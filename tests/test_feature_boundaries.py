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
    [tables]
    kernel = ["servers", "management_audit"]
    proxy = ["singbox_*", "users"]
    dns = ["dns_*"]
    lab = ["operations_*"]

    [rust]
    "crates/panel-host/src" = "kernel"
    "crates/panel-host/src/operations" = "lab"
    "plugins/singbox" = "proxy"
    "plugins/ddns" = "dns"

    [rust_exempt]
    paths = ["crates/panel/tests"]

    [web]
    "web/src" = "shared"
    "web/src/app" = "shell"
    "web/src/plugins/singbox" = "proxy"
    "web/src/plugins/ddns" = "dns"
''')
SCHEMA = textwrap.dedent('''
    CREATE TABLE public.servers (id bigint);
    CREATE TABLE public.management_audit (id bigint);
    CREATE TABLE public.singbox_nodes (id bigint);
    CREATE TABLE public.users (id bigint);
    CREATE TABLE public.dns_records (id bigint);
    CREATE TABLE public.operations_jobs (id bigint);
    CREATE VIEW public.singbox_live AS SELECT 1;
''')


class OwnershipTests(unittest.TestCase):
    def test_exact_names_win_over_prefixes_and_the_longest_prefix_wins(self):
        rules = {"kernel": ["operations_backup_records"], "lab": ["operations_*"], "cloud": ["operations_hetzner_*"]}
        self.assertEqual(boundaries.table_owner("operations_backup_records", rules), "kernel")
        self.assertEqual(boundaries.table_owner("operations_hetzner_accounts", rules), "cloud")
        self.assertEqual(boundaries.table_owner("operations_jobs", rules), "lab")
        self.assertIsNone(boundaries.table_owner("servers", rules))

    def test_equal_claims_are_rejected(self):
        with self.assertRaises(ValueError):
            boundaries.table_owner("dns_records", {"dns": ["dns_*"], "lab": ["dns_*"]})

    def test_longest_path_prefix_decides_the_owner(self):
        mapping = {"crates/panel-host/src": "kernel", "crates/panel-host/src/operations": "lab",
                   "crates/panel-host/src/operations/backups.rs": "kernel"}
        self.assertEqual(boundaries.path_owner("crates/panel-host/src/operations/backups.rs", mapping), "kernel")
        self.assertEqual(boundaries.path_owner("crates/panel-host/src/operations/jobs.rs", mapping), "lab")
        self.assertEqual(boundaries.path_owner("crates/panel-host/src/servers.rs", mapping), "kernel")
        self.assertIsNone(boundaries.path_owner("web/src/App.tsx", mapping))


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


class TreeTests(unittest.TestCase):
    def tree(self, files):
        temp = tempfile.TemporaryDirectory()
        self.addCleanup(temp.cleanup)
        root = Path(temp.name)
        (root / "tools").mkdir()
        (root / "tools/feature-boundaries.toml").write_text(CONFIG)
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
            "crates/panel-host/src/servers.rs": 'const Q: &str = "UPDATE servers SET id=1";',
            "crates/panel-host/src/operations/jobs.rs": 'const Q: &str = "SELECT id FROM operations_jobs JOIN servers ON true";',
            "plugins/singbox/panel/Cargo.toml": "",
            "plugins/singbox/panel/src/publish.rs": 'const A: &str = "SELECT id FROM servers"; const B: &str = "INSERT INTO management_audit(id) VALUES(1)";',
            "plugins/singbox/panel/src/hosts.rs": 'const A: &str = "SELECT id FROM dns_records";',
            "plugins/singbox/panel/src/tests.rs": 'const A: &str = "INSERT INTO servers(id) VALUES(1)";',
            "crates/panel/tests/all.rs": 'const A: &str = "DELETE FROM dns_records";',
        })
        self.assertEqual(self.crossings(root), {
            "sql\tcrates/panel-host/src/search.rs\tread\tdns_records\tkernel->dns",
            "sql\tplugins/singbox/panel/src/publish.rs\twrite\tmanagement_audit\tproxy->kernel",
            "sql\tplugins/singbox/panel/src/hosts.rs\tread\tdns_records\tproxy->dns",
        })

    def test_path_includes_across_crates_are_reported(self):
        root = self.tree({
            "crates/panel-host/Cargo.toml": "",
            "crates/panel-host/src/diagnostics.rs": '#[path = "../../../plugins/ddns/panel/mod.rs"]\nmod ddns;\n#[path = "local.rs"]\nmod local;',
            "crates/panel-host/src/local.rs": "",
            "plugins/ddns/panel/mod.rs": "",
        })
        self.assertEqual(self.crossings(root), {
            "path\tcrates/panel-host/src/diagnostics.rs\tplugins/ddns/panel/mod.rs\tkernel->dns"})

    def test_web_imports_between_features_and_into_the_shell_are_reported(self):
        root = self.tree({
            "web/src/components.tsx": "import { tiers } from './app/tiers'\nimport { Node } from './plugins/singbox/types'",
            "web/src/app/tiers.ts": "export const tiers = 1",
            "web/src/app/routes.ts": "import { Node } from '../plugins/singbox/types'",
            "web/src/plugins/singbox/types.ts": "export type Node = 1",
            "web/src/plugins/singbox/Domain.tsx": "import { Account } from '../ddns/Accounts'\nimport { Panel } from '@/components'",
            "web/src/plugins/ddns/Accounts.tsx": "export type Account = 1",
        })
        self.assertEqual(self.crossings(root), {
            "import\tweb/src/components.tsx\tweb/src/app/tiers.ts\tshared->shell",
            "import\tweb/src/components.tsx\tweb/src/plugins/singbox/types.ts\tshared->proxy",
            "import\tweb/src/plugins/singbox/Domain.tsx\tweb/src/plugins/ddns/Accounts.tsx\tproxy->dns",
        })

    def test_tables_without_an_owner_fail(self):
        root = self.tree({})
        (root / "crates/panel/migrations/0001_current.sql").write_text(SCHEMA + "CREATE TABLE public.orphans (id bigint);\n")
        config = boundaries.tomllib.loads((root / "tools/feature-boundaries.toml").read_text())
        owners, problems = boundaries.ownership(root, config)
        self.assertEqual(problems, ["table orphans has no owner in tools/feature-boundaries.toml"])

    def test_the_baseline_only_shrinks(self):
        root = self.tree({
            "plugins/singbox/panel/Cargo.toml": "",
            "plugins/singbox/panel/src/hosts.rs": 'const A: &str = "SELECT id FROM dns_records";',
        })
        baseline = root / "tools/feature-boundaries.baseline"
        self.assertEqual(boundaries.main(["--root", str(root)]), 0)
        self.assertIn("proxy->dns", baseline.read_text())
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
        self.assertNotIn("proxy->dns", baseline.read_text())


if __name__ == "__main__":
    unittest.main()
