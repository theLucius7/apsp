import importlib.util
from pathlib import Path
import tempfile
import textwrap
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "tools/check-feature-boundaries.py"
spec = importlib.util.spec_from_file_location("feature_boundaries", SCRIPT)
boundaries = importlib.util.module_from_spec(spec)
spec.loader.exec_module(boundaries)

CONFIG = textwrap.dedent('''
    [domains]
    order = [["platform"], ["governance"], ["resources"], ["networking", "naming"], ["traffic"],
             ["access"], ["operations"], ["app"]]
    published = ["servers", "users"]

    [tables]
    platform = ["notification_outbox"]
    governance = ["management_audit"]
    resources = ["servers"]
    naming = ["dns_*"]
    traffic = ["singbox_*"]
    access = ["users"]
    operations = ["operations_*"]

    [rust]
    "crates/panel-host/src" = "platform"
    "crates/panel-host/src/servers.rs" = "resources"
    "crates/panel-host/src/operations" = "operations"
    "crates/naming" = "naming"
    "crates/panel/src" = "app"
    "plugins/singbox" = "traffic"
    "plugins/singbox/panel/src/users.rs" = "access"

    [rust_exempt]
    paths = ["crates/panel/tests"]

    [web]
    "web/src" = "platform"
    "web/src/app" = "app"
    "web/src/plugins/singbox" = "traffic"
    "web/src/plugins/ddns" = "naming"
''')
SCHEMA = textwrap.dedent('''
    CREATE TABLE public.notification_outbox (id bigint);
    CREATE TABLE public.management_audit (id bigint);
    CREATE TABLE public.servers (id bigint);
    CREATE TABLE public.dns_records (id bigint);
    CREATE TABLE public.singbox_nodes (id bigint);
    CREATE TABLE public.users (id bigint);
    CREATE TABLE public.operations_jobs (id bigint);
    CREATE VIEW public.singbox_live AS SELECT 1;
''')


class OwnershipTests(unittest.TestCase):
    def test_exact_names_win_over_prefixes_and_the_longest_prefix_wins(self):
        rules = {"governance": ["operations_backup_records"], "operations": ["operations_*"],
                 "resources": ["operations_hetzner_*"]}
        self.assertEqual(boundaries.table_owner("operations_backup_records", rules), "governance")
        self.assertEqual(boundaries.table_owner("operations_hetzner_accounts", rules), "resources")
        self.assertEqual(boundaries.table_owner("operations_jobs", rules), "operations")
        self.assertIsNone(boundaries.table_owner("servers", rules))

    def test_equal_claims_are_rejected(self):
        with self.assertRaises(ValueError):
            boundaries.table_owner("dns_records", {"naming": ["dns_*"], "networking": ["dns_*"]})

    def test_longest_path_prefix_decides_the_owner(self):
        mapping = {"crates/panel-host/src": "platform", "crates/panel-host/src/operations": "operations",
                   "crates/panel-host/src/operations/backups.rs": "governance"}
        self.assertEqual(boundaries.path_owner("crates/panel-host/src/operations/backups.rs", mapping), "governance")
        self.assertEqual(boundaries.path_owner("crates/panel-host/src/operations/jobs.rs", mapping), "operations")
        self.assertEqual(boundaries.path_owner("crates/panel-host/src/servers.rs", mapping), "platform")
        self.assertIsNone(boundaries.path_owner("web/src/App.tsx", mapping))

    def test_dependency_groups_rank_owners_once(self):
        config = boundaries.tomllib.loads(CONFIG)
        rank = boundaries.ranks(config)
        self.assertEqual(rank["platform"], 0)
        self.assertEqual(rank["networking"], rank["naming"])
        self.assertLess(rank["traffic"], rank["access"])
        config["domains"]["order"].append(["naming"])
        with self.assertRaises(ValueError):
            boundaries.ranks(config)

    def test_published_reads_follow_the_dependency_order(self):
        rank = boundaries.ranks(boundaries.tomllib.loads(CONFIG))
        published = {"servers", "users"}
        allowed = boundaries.sql_allowed
        self.assertTrue(allowed("write", "servers", "resources", "resources", rank, published))
        self.assertTrue(allowed("read", "servers", "traffic", "resources", rank, published))
        self.assertFalse(allowed("write", "servers", "traffic", "resources", rank, published))
        # Reading a later owner's identity, or a peer's, is a dependency in the wrong direction.
        self.assertFalse(allowed("read", "users", "traffic", "access", rank, published))
        self.assertFalse(allowed("read", "servers", "platform", "resources", rank, published))
        self.assertFalse(allowed("read", "dns_records", "networking", "naming", rank, published | {"dns_records"}))
        # Private tables and platform tables are never read directly by another owner.
        self.assertFalse(allowed("read", "dns_records", "operations", "naming", rank, published))
        self.assertFalse(allowed("read", "notification_outbox", "naming", "platform", rank, published))


class SqlTests(unittest.TestCase):
    KNOWN = {"servers", "management_audit", "singbox_nodes", "users", "dns_records"}

    def test_reads_and_writes_are_told_apart(self):
        found = boundaries.sql_accesses(
            "INSERT INTO management_audit(id) SELECT n.id FROM singbox_nodes n JOIN servers s ON s.id=n.id", self.KNOWN)
        self.assertEqual(found, {("write", "management_audit"), ("read", "singbox_nodes"), ("read", "servers")})
        self.assertEqual(boundaries.sql_accesses("DELETE FROM dns_records WHERE id=$1", self.KNOWN), {("write", "dns_records")})
        self.assertEqual(boundaries.sql_accesses("UPDATE users u SET name=$1 FROM servers s", self.KNOWN),
                         {("write", "users"), ("read", "servers")})

    def test_prose_and_unknown_names_are_not_sql(self):
        self.assertEqual(boundaries.sql_accesses("failed to update servers", self.KNOWN), set())
        self.assertEqual(boundaries.sql_accesses("SELECT extract(epoch FROM now())", self.KNOWN), set())
        self.assertEqual(boundaries.sql_accesses("data from servers", self.KNOWN), set())

    def test_strings_come_from_literals_only(self):
        source = textwrap.dedent('''
            // SELECT * FROM dns_records in a comment
            /* block /* nested */ SELECT * FROM dns_records */
            let a = "SELECT id FROM users";
            let b = r#"DELETE FROM "quoted" dns_records"#;
            let c = '"'; let d = b"bytes";
            fn f<'a>(x: &'a str) {}
        ''')
        self.assertEqual(boundaries.rust_strings(source), ["SELECT id FROM users", 'DELETE FROM "quoted" dns_records', "bytes"])


class ModuleRefTests(unittest.TestCase):
    def test_code_keeps_offsets_and_drops_comments_and_literals(self):
        source = 'let a = "crate::x"; // crate::y\n/* crate::z */ crate::w::f(\'c\');\n'
        code = boundaries.rust_code(source)
        self.assertEqual(len(code), len(source))
        self.assertEqual(code.count("\n"), 2)
        self.assertNotIn("crate::x", code)
        self.assertNotIn("crate::y", code)
        self.assertNotIn("crate::z", code)
        self.assertIn("crate::w::f(", code)

    def test_use_trees_expand_to_paths(self):
        code = "use crate::{a, b::{c, d}, e::*, F};"
        start = code.index("{")
        self.assertEqual(boundaries.use_paths(code, start), [["a"], ["b", "c"], ["b", "d"], ["e"], ["F"]])
        call = "crate::g::h(1)::<T>"
        self.assertEqual(boundaries.use_paths(call, len("crate::")), [["g", "h"]])
        renamed, renames = "use other::{a as b, c::{d as e, f}};", []
        self.assertEqual(boundaries.use_paths(renamed, len("use "), renames),
                         [["other", "a"], ["other", "c", "d"], ["other", "c", "f"]])
        self.assertEqual(renames, [(["other", "a"], "b"), (["other", "c", "d"], "e")])

    def test_crate_and_super_paths_resolve_to_source_files(self):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        crate = Path(temp.name)
        files = {"Cargo.toml": '[package]\nname = "fixture"\n', "src/lib.rs": "", "src/a.rs": "",
                 "src/b/mod.rs": "", "src/b/c.rs": "", "src/b/d.rs": ""}
        for relative, text in files.items():
            (crate / relative).parent.mkdir(parents=True, exist_ok=True)
            (crate / relative).write_text(text)
        root_file = boundaries.crate_source(crate)
        self.assertEqual(root_file, crate / "src/lib.rs")
        code = "use super::d::Item; fn f() { crate::a::run(); super::super::a::run(); }"
        self.assertEqual(boundaries.module_refs(crate / "src/b/c.rs", code, root_file),
                         {crate / "src/b/d.rs", crate / "src/a.rs"})
        # `super` in a mod.rs names the parent module; the crate root has no parent.
        self.assertEqual(boundaries.module_refs(crate / "src/b/mod.rs", "super::a::run()", root_file),
                         {crate / "src/a.rs"})
        self.assertEqual(boundaries.module_refs(crate / "src/lib.rs", "super::a::run()", root_file), set())

    def test_reference_direction_follows_the_order(self):
        rank = boundaries.ranks(boundaries.tomllib.loads(CONFIG))
        self.assertTrue(boundaries.ref_allowed("resources", "governance", rank))
        self.assertTrue(boundaries.ref_allowed("operations", "platform", rank))
        self.assertTrue(boundaries.ref_allowed("app", "platform", rank))
        self.assertFalse(boundaries.ref_allowed("platform", "resources", rank))
        self.assertFalse(boundaries.ref_allowed("governance", "resources", rank))
        self.assertFalse(boundaries.ref_allowed("naming", "networking", rank))


class TreeTests(unittest.TestCase):
    def tree(self, files, config=CONFIG):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        (root / "tools").mkdir()
        (root / "tools/feature-boundaries.toml").write_text(config)
        (root / "crates/panel/migrations").mkdir(parents=True)
        (root / "crates/panel/migrations/0001_current.sql").write_text(SCHEMA)
        for relative, text in files.items():
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(textwrap.dedent(text))
        return root

    def crossings(self, root):
        config = boundaries.tomllib.loads((root / "tools/feature-boundaries.toml").read_text())
        owners, problems = boundaries.ownership(root, config)
        self.assertEqual(problems, [])
        return boundaries.rust_crossings(root, config, owners) | boundaries.web_crossings(root, config)

    def test_sql_rules(self):
        root = self.tree({
            "crates/panel-host/Cargo.toml": "",
            "crates/panel-host/src/search.rs": 'const Q: &str = "SELECT id FROM dns_records";',
            "crates/panel-host/src/agent.rs": 'const Q: &str = "SELECT id FROM servers";',
            "crates/panel-host/src/servers.rs": 'const Q: &str = "UPDATE servers SET id=1";',
            "crates/panel-host/src/operations/jobs.rs": 'const Q: &str = "SELECT id FROM operations_jobs JOIN servers ON true";',
            "crates/naming/Cargo.toml": "",
            "crates/naming/src/outbox.rs": 'const Q: &str = "INSERT INTO notification_outbox(id) VALUES(1)";',
            "crates/panel/Cargo.toml": "",
            "crates/panel/src/main.rs": 'const A: &str = "SELECT id FROM users"; const B: &str = "DELETE FROM users";',
            "plugins/singbox/panel/Cargo.toml": "",
            "plugins/singbox/panel/src/publish.rs": 'const A: &str = "SELECT id FROM servers"; const B: &str = "INSERT INTO management_audit(id) VALUES(1)";',
            "plugins/singbox/panel/src/hosts.rs": 'const A: &str = "SELECT id FROM dns_records JOIN users ON true";',
            "plugins/singbox/panel/src/users.rs": 'const A: &str = "UPDATE users SET id=1 FROM singbox_nodes, servers";',
            "plugins/singbox/panel/src/tests.rs": 'const A: &str = "INSERT INTO servers(id) VALUES(1)";',
            "crates/panel/tests/all.rs": 'const A: &str = "DELETE FROM dns_records";',
        })
        self.assertEqual(self.crossings(root), {
            "sql\tcrates/panel-host/src/search.rs\tread\tdns_records\tplatform->naming",
            "sql\tcrates/panel-host/src/agent.rs\tread\tservers\tplatform->resources",
            "sql\tcrates/naming/src/outbox.rs\twrite\tnotification_outbox\tnaming->platform",
            "sql\tcrates/panel/src/main.rs\twrite\tusers\tapp->access",
            "sql\tplugins/singbox/panel/src/publish.rs\twrite\tmanagement_audit\ttraffic->governance",
            "sql\tplugins/singbox/panel/src/hosts.rs\tread\tdns_records\ttraffic->naming",
            "sql\tplugins/singbox/panel/src/hosts.rs\tread\tusers\ttraffic->access",
            "sql\tplugins/singbox/panel/src/users.rs\tread\tsingbox_nodes\taccess->traffic",
        })

    def test_module_references_against_the_order_are_reported(self):
        config = CONFIG.replace('"crates/panel-host/src/servers.rs" = "resources"',
                                '"crates/panel-host/src/servers.rs" = "resources"\n'
                                '"crates/panel-host/src/auth.rs" = "governance"')
        root = self.tree({
            "crates/panel-host/Cargo.toml": '[package]\nname = "fixture"\n',
            "crates/panel-host/src/lib.rs": "pub mod auth; pub mod operations; pub mod router; pub mod servers;",
            "crates/panel-host/src/auth.rs": "pub fn check() {}",
            "crates/panel-host/src/router.rs": "use crate::{servers, auth::check};\n// crate::operations is prose\n",
            "crates/panel-host/src/servers.rs": "fn f() { crate::auth::check(); crate::operations::jobs::run(); }",
            "crates/panel-host/src/operations/mod.rs": "pub mod jobs; pub mod queue;",
            "crates/panel-host/src/operations/jobs.rs": "use crate::servers; use super::queue::Item;",
            "crates/panel-host/src/operations/queue.rs": "",
            "crates/panel-host/src/operations/tests.rs": "use crate::router;",
        }, config)
        self.assertEqual(self.crossings(root), {
            "ref\tcrates/panel-host/src/router.rs\tcrates/panel-host/src/servers.rs\tplatform->resources",
            "ref\tcrates/panel-host/src/router.rs\tcrates/panel-host/src/auth.rs\tplatform->governance",
            "ref\tcrates/panel-host/src/servers.rs\tcrates/panel-host/src/operations/jobs.rs\tresources->operations",
        })

    def test_references_through_other_crates_are_reported(self):
        config = CONFIG.replace('"crates/panel-host/src/servers.rs" = "resources"',
                                '"crates/panel-host/src/servers.rs" = "resources"\n'
                                '"crates/panel-host/src/auth.rs" = "governance"')
        root = self.tree({
            "crates/panel-host/Cargo.toml": '[package]\nname = "apsp-panel-host"\n',
            "crates/panel-host/src/lib.rs": "pub mod auth; pub mod operations; pub mod servers;",
            "crates/panel-host/src/auth.rs": "pub fn check() {}",
            "crates/panel-host/src/servers.rs": "",
            "crates/panel-host/src/operations/mod.rs": "pub fn enqueue() {}",
            "plugins/singbox/panel/Cargo.toml": '[package]\nname = "apsp-plugin-singbox"\n\n[lib]\npath = "mod.rs"\n',
            "plugins/singbox/panel/mod.rs": "pub mod deploy; pub mod local; pub mod users;\n"
                                            "pub(crate) use apsp_panel_host::{auth, servers};\n",
            "plugins/singbox/panel/local.rs": "pub fn enqueue() {}",
            # `crate::servers` names the host through the root's re-export; a local module wins.
            "plugins/singbox/panel/deploy.rs": "fn f() { crate::auth::check(); crate::servers::x(); crate::local::enqueue(); }",
            "plugins/singbox/panel/users.rs": "use apsp_panel_host::{auth::check, operations::enqueue};",
            "crates/panel/Cargo.toml": '[package]\nname = "apsp-panel"\n',
            "crates/panel/src/lib.rs": "pub use apsp_panel_host::operations;\nfn f() { apsp_plugin_singbox::deploy::f(); }",
        }, config)
        self.assertEqual(self.crossings(root), {
            "ref\tplugins/singbox/panel/users.rs\tcrates/panel-host/src/operations/mod.rs\ttraffic->operations",
        })
        # A re-exported later module is a crossing where the root names it and where it is used,
        # under its own name or a new one.
        (root / "plugins/singbox/panel/mod.rs").write_text(
            "pub mod deploy; pub mod local; pub mod users;\npub(crate) use apsp_panel_host::{auth, operations as jobs, servers};\n")
        (root / "plugins/singbox/panel/deploy.rs").write_text("fn f() { crate::jobs::enqueue(); }")
        self.assertEqual(self.crossings(root), {
            "ref\tplugins/singbox/panel/mod.rs\tcrates/panel-host/src/operations/mod.rs\ttraffic->operations",
            "ref\tplugins/singbox/panel/deploy.rs\tcrates/panel-host/src/operations/mod.rs\ttraffic->operations",
            "ref\tplugins/singbox/panel/users.rs\tcrates/panel-host/src/operations/mod.rs\ttraffic->operations",
        })

    def test_inline_test_modules_are_fixtures(self):
        source = textwrap.dedent('''
            const Q: &str = "SELECT id FROM users";
            #[cfg(test)]
            mod tests {
                fn seed() { let s = "{"; let c = '}'; /* } */ sqlx::query("INSERT INTO servers(id) VALUES(1)"); }
                mod inner { fn f() {} }
            }
            const R: &str = "SELECT id FROM dns_records";
        ''')
        self.assertEqual(boundaries.rust_strings(boundaries.without_test_modules(source)),
                         ["SELECT id FROM users", "SELECT id FROM dns_records"])

    def test_path_includes_across_crates_are_reported(self):
        root = self.tree({
            "crates/panel-host/Cargo.toml": "",
            "crates/panel-host/src/diagnostics.rs": '#[path = "../../naming/src/lib.rs"]\nmod naming;\n#[path = "local.rs"]\nmod local;',
            "crates/panel-host/src/local.rs": "",
            "crates/naming/Cargo.toml": "",
            "crates/naming/src/lib.rs": "",
        })
        self.assertEqual(self.crossings(root), {
            "path\tcrates/panel-host/src/diagnostics.rs\tcrates/naming/src/lib.rs\tplatform->naming"})

    def test_web_imports_between_domains_and_into_the_shell_are_reported(self):
        root = self.tree({
            "web/src/components.tsx": "import { tiers } from './app/tiers'\nimport { Node } from './plugins/singbox/types'",
            "web/src/app/tiers.ts": "export const tiers = 1",
            "web/src/app/routes.ts": "import { Node } from '../plugins/singbox/types'",
            "web/src/plugins/singbox/types.ts": "export type Node = 1",
            "web/src/plugins/singbox/Domain.tsx": "import { Account } from '../ddns/Accounts'\nimport { Panel } from '@/components'\nimport { tiers } from '../../app/tiers'",
            "web/src/plugins/ddns/Accounts.tsx": "export type Account = 1",
        })
        self.assertEqual(self.crossings(root), {
            "import\tweb/src/components.tsx\tweb/src/app/tiers.ts\tplatform->app",
            "import\tweb/src/components.tsx\tweb/src/plugins/singbox/types.ts\tplatform->traffic",
            "import\tweb/src/plugins/singbox/Domain.tsx\tweb/src/plugins/ddns/Accounts.tsx\ttraffic->naming",
            "import\tweb/src/plugins/singbox/Domain.tsx\tweb/src/app/tiers.ts\ttraffic->app",
        })

    def test_tables_without_an_owner_fail(self):
        root = self.tree({})
        (root / "crates/panel/migrations/0001_current.sql").write_text(SCHEMA + "CREATE TABLE public.orphans (id bigint);\n")
        config = boundaries.tomllib.loads((root / "tools/feature-boundaries.toml").read_text())
        owners, problems = boundaries.ownership(root, config)
        self.assertEqual(problems, ["table orphans has no owner in tools/feature-boundaries.toml"])

    def test_unknown_owners_and_published_tables_fail(self):
        config = CONFIG.replace('"crates/naming" = "naming"', '"crates/naming" = "nameing"')
        config = config.replace('published = ["servers", "users"]', 'published = ["servers", "accounts"]')
        root = self.tree({}, config)
        owners, problems = boundaries.ownership(root, boundaries.tomllib.loads(config))
        self.assertEqual(problems, ["owner nameing is not in the dependency order in tools/feature-boundaries.toml",
                                    "published table accounts is not in the schema"])
        broken = boundaries.tomllib.loads(CONFIG)
        broken["domains"]["order"].append(["access"])
        owners, problems = boundaries.ownership(root, broken)
        self.assertEqual(len(problems), 1)
        self.assertIn("appears twice", problems[0])

    def test_the_baseline_only_shrinks(self):
        root = self.tree({
            "plugins/singbox/panel/Cargo.toml": "",
            "plugins/singbox/panel/src/hosts.rs": 'const A: &str = "SELECT id FROM dns_records";',
        })
        baseline = root / "tools/feature-boundaries.baseline"
        self.assertEqual(boundaries.main(["--root", str(root)]), 0)
        self.assertIn("traffic->naming", baseline.read_text())
        # A new crossing fails and is not added, even with --update.
        (root / "plugins/singbox/panel/src/more.rs").write_text('const B: &str = "DELETE FROM dns_records";')
        self.assertEqual(boundaries.main(["--root", str(root)]), 1)
        self.assertEqual(boundaries.main(["--root", str(root), "--update"]), 1)
        self.assertNotIn("more.rs", baseline.read_text())
        # Fixing both crossings passes, and --update removes them from the list.
        for name in ("hosts.rs", "more.rs"):
            (root / "plugins/singbox/panel/src" / name).write_text("")
        self.assertEqual(boundaries.main(["--root", str(root)]), 0)
        self.assertEqual(boundaries.main(["--root", str(root), "--update"]), 0)
        self.assertNotIn("traffic->naming", baseline.read_text())


if __name__ == "__main__":
    unittest.main()
