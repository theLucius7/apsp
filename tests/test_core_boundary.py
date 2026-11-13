import importlib.util
from pathlib import Path
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "tools/check-core-boundary.py"
spec = importlib.util.spec_from_file_location("core_boundary", SCRIPT)
boundary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(boundary)


class CoreBoundaryTests(unittest.TestCase):
    def test_business_identifiers_sql_routes_and_comments_are_rejected(self):
        for source in (
            "struct ProxyUser { user_id: i64 }", "SELECT users.id FROM users",
            '"/api/users"', "loadSubscription()", "subscription_token",
            "quota_bytes", "AccountQuota", "// user traffic", "USER_ID",
            "SubscriptionURL", "monthly_quotas", "userId",
        ):
            with self.subTest(source=source):
                self.assertTrue(boundary.violations("src/transport.rs", source))

    def test_runtime_name_remains_forbidden_in_every_file(self):
        for relative in ("Cargo.toml", "README.md", "src/system/windows.rs"):
            for source in ("singbox", "SING-BOX"):
                self.assertTrue(boundary.violations(relative, source))

    def test_native_exceptions_are_expression_and_file_scoped(self):
        fixtures = {
            "src/system/services.rs": '"--property=LoadState,MainPID,User,Group,SupplementaryGroups"; properties.get("User")',
            "src/system/jobs.rs": '"--property=CPUQuota=20%"; "--property=CPUQuotaPeriodSec=100ms"',
            "src/system/budgets.rs": '("CPUQuota", format!("{}%", limit)); ("CPUQuotaPeriodSec", "100ms".into())',
            "src/system/tests.rs": '"--property=CPUWeight,CPUQuotaPerSecUSec,IOWeight,OOMScoreAdjust"; ("CPUQuotaPerSecUSec", "1s".into())',
            "src/system_network/firewall.rs": '"WantedBy=multi-user.target\\n"',
            "src/system_network/tunnel.rs": 'format!("UserKnownHostsFile={}",known.display())',
            "src/system/windows.rs": "[Security.Principal.WindowsIdentity]::GetCurrent().User.Value",
            "src/system/deploy/native/windows.rs": "Get-LocalUser -Name $account; -UserId 'SYSTEM'; USER_RIGHTS",
            "src/system/deploy/native/unix.rs": '"/Users/example"; "UserShell"; <key>UserName</key>',
            "tests/usage_bounds.rs": '.pragma_update(None, "user_version", 1); "PRAGMA user_version"',
        }
        for relative, source in fixtures.items():
            with self.subTest(relative=relative):
                self.assertFalse(boundary.violations(relative, source))
                self.assertTrue(boundary.violations("src/transport.rs", source))
                self.assertTrue(boundary.violations(relative, source + "; user_id=1"))

    def test_api_names_and_existing_account_terms_do_not_match_business(self):
        self.assertFalse(boundary.violations("src/config.rs", "url.username(); account_name; usershow; useradd"))

    def test_network_native_exception_cannot_mask_repurposed_business_terms(self):
        for source in ('"multi-user.target"', '"WantedBy=multi-user.target/proxy"',
                       '"WantedBy=multi-user.target\\n"; user_id=1'):
            self.assertTrue(boundary.violations("src/system_network/firewall.rs", source))
        for source in ('UserKnownHostsFile', 'format!("UserKnownHostsFile=proxy-user")',
                       'format!("UserKnownHostsFile={}", user_id)'):
            self.assertTrue(boundary.violations("src/system_network/tunnel.rs", source))

    def test_formatted_native_expression_preserves_lines_and_adjacent_business_rejection(self):
        native = 'let account = properties\n    .get("User")\n    .map(String::as_str);'
        relative = "src/system_network/certificates.rs"
        self.assertFalse(boundary.violations(relative, native))
        self.assertTrue(boundary.violations("src/transport.rs", native))
        findings = boundary.violations(relative, native + '\nlet user_id = 1;')
        self.assertEqual(findings, [(4, 'user')])
        findings = boundary.violations(relative, native.replace('.get("User")', '.get("User"); user_id=1'))
        self.assertEqual(findings, [(2, 'user')])

    def test_certificate_native_account_fixture_exception_is_exact_and_file_scoped(self):
        relative = "src/system_network/certificates.rs"
        for native in ('loaded.insert("User".into(), "operator".into());',
                       'loaded.insert("User".into(), String::new());'):
            with self.subTest(native=native):
                self.assertFalse(boundary.violations(relative, native))
                self.assertTrue(boundary.violations("src/transport.rs", native))
                self.assertEqual(boundary.violations(relative, native + " user_id=1;"),
                                 [(1, 'user')])
        for native in ('other.insert("User".into(), String::new());',
                       'loaded.insert("User", String::new());'):
            self.assertTrue(boundary.violations(relative, native))

    def test_macos_account_path_exception_does_not_hide_business_routes(self):
        relative = "src/system/deploy/native/unix.rs"
        for source in ('"/Users".into()', 'format!("/Users/{name}")'):
            with self.subTest(source=source):
                self.assertFalse(boundary.violations(relative, source))
        for source in ('let route = "/api/Users";', 'let path = "/proxy/Users/example";',
                       'let path = format!("{root}/Users");', 'let path = "/Users/user_id";'):
            with self.subTest(source=source):
                self.assertTrue(boundary.violations(relative, source))

    def test_new_file_is_scanned_and_current_core_passes(self):
        self.assertTrue(boundary.check(SCRIPT.parents[1] / "crates/agent-core"))
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "new.sql").write_text("SELECT quota FROM state")
            self.assertFalse(boundary.check(root))

    def test_panel_host_passes_and_its_exception_stays_scoped(self):
        root = SCRIPT.parents[1]
        self.assertTrue(boundary.check(root / "crates/panel-host", boundary.HOST_EXCEPTIONS))
        relative, source = "src/config.rs", '"https://user:secret@example.invalid"'
        self.assertFalse(boundary.violations(relative, source, boundary.HOST_EXCEPTIONS))
        self.assertTrue(boundary.violations(relative, source))
        self.assertTrue(boundary.violations("src/settings.rs", source, boundary.HOST_EXCEPTIONS))
        self.assertTrue(boundary.violations(relative, source + "; user_id", boundary.HOST_EXCEPTIONS))
        self.assertTrue(boundary.violations("src/lib.rs", "pub mod singbox;", boundary.HOST_EXCEPTIONS))
        # Governance, resources and the other domains left the platform crate, so their
        # former exceptions no longer exist there.
        self.assertTrue(boundary.violations("src/releases/network.rs", '.user_agent("apsp-panel")',
                                            boundary.HOST_EXCEPTIONS))
        self.assertTrue(boundary.violations("src/network_workbench/reports.rs", 'r.get("daily_quota")',
                                            boundary.HOST_EXCEPTIONS))

    def test_cloud_api_passes_and_its_header_exception_stays_scoped(self):
        root = SCRIPT.parents[1]
        exceptions = boundary.CORES["crates/cloud-api"]
        self.assertTrue(boundary.check(root / "crates/cloud-api", exceptions))
        relative = "src/globalping.rs"
        native = 'const USER_AGENT: &str = "apsp-panel";\n.header("user-agent", USER_AGENT)'
        self.assertFalse(boundary.violations(relative, native, exceptions))
        self.assertTrue(boundary.violations("src/aliyun.rs", native, exceptions))
        self.assertTrue(boundary.violations(relative, native + "; user_id", exceptions))
        self.assertTrue(boundary.violations(relative, '.header("x-user", USER_AGENT)', exceptions))

    def test_layers_depend_in_the_documented_direction(self):
        self.assertTrue(boundary.check_dependencies(SCRIPT.parents[1]))
        protocol = "[dependencies]\napsp-protocol.workspace = true\n"
        manifests = {
            "crates/protocol/Cargo.toml": "[package]\n",
            "crates/adapter-sdk/Cargo.toml": "[package]\n",
            "crates/tcp-probe/Cargo.toml": "[package]\n",
            "crates/adapter-singbox/Cargo.toml": "[dependencies]\napsp-adapter-sdk.workspace = true\n",
            "crates/agent-core/Cargo.toml": protocol,
            "crates/agent/Cargo.toml": "[dependencies]\napsp-agent-core.workspace = true\napsp-adapter-singbox.workspace = true\n",
            "crates/panel-host/Cargo.toml": protocol,
            "crates/cloud-api/Cargo.toml": protocol,
            "crates/compiler/Cargo.toml": "[package]\n",
            "crates/governance/Cargo.toml": "[dependencies]\napsp-panel-host.workspace = true\n",
            "crates/resources/Cargo.toml": "[dependencies]\napsp-panel-host.workspace = true\napsp-governance.workspace = true\napsp-cloud-api.workspace = true\n",
            "crates/networking/Cargo.toml": "[dependencies]\napsp-panel-host.workspace = true\napsp-resources.workspace = true\n",
            # Names and certificates build on basic networking, which keeps their documents.
            "crates/naming/Cargo.toml": "[dependencies]\napsp-panel-host.workspace = true\napsp-cloud-api.workspace = true\napsp-networking.workspace = true\n",
            "crates/costs/Cargo.toml": "[dependencies]\napsp-panel-host.workspace = true\napsp-naming.workspace = true\n",
            "crates/operations/Cargo.toml": "[dependencies]\napsp-panel-host.workspace = true\napsp-costs.workspace = true\napsp-observability.workspace = true\napsp-traffic.workspace = true\n",
            "crates/observability/Cargo.toml": "[dependencies]\napsp-panel-host.workspace = true\napsp-traffic.workspace = true\napsp-access.workspace = true\n",
            # Traffic builds on its own configuration compiler; access builds on traffic.
            "crates/traffic/Cargo.toml": "[dependencies]\napsp-panel-host.workspace = true\napsp-compiler.workspace = true\n",
            "crates/access/Cargo.toml": "[dependencies]\napsp-panel-host.workspace = true\napsp-traffic.workspace = true\n",
            "crates/panel/Cargo.toml": "[dependencies]\napsp-costs.workspace = true\napsp-traffic.workspace = true\napsp-access.workspace = true\n",
        }
        broken = {
            # The platform never depends on a domain or on a crate that holds domains.
            "crates/panel-host/Cargo.toml": "[dev-dependencies]\napsp-traffic.workspace = true\n",
            "crates/cloud-api/Cargo.toml": "[dependencies]\napsp-naming.workspace = true\n",
            # A domain depends only on earlier groups, never on a later domain or a peer.
            "crates/naming/Cargo.toml": "[dependencies]\napsp-costs.workspace = true\n",
            "crates/compiler/Cargo.toml": "[dependencies]\napsp-costs.workspace = true\n",
            "crates/costs/Cargo.toml": "[dependencies]\napsp-operations.workspace = true\n",
            "crates/resources/Cargo.toml": "[dev-dependencies]\napsp-networking.workspace = true\n",
            # Networking comes before naming, so it cannot depend on it.
            "crates/networking/Cargo.toml": "[dependencies]\napsp-naming.workspace = true\n",
            # Peers in one group do not depend on each other.
            "crates/observability/Cargo.toml": "[dependencies]\napsp-costs.workspace = true\n",
            # Traffic comes before access, so it cannot depend on it.
            "crates/traffic/Cargo.toml": "[dependencies]\napsp-access.workspace = true\n",
            "crates/protocol/Cargo.toml": "[dependencies]\napsp-compiler.workspace = true\n",
            "crates/agent-core/Cargo.toml": "[target.'cfg(unix)'.dependencies]\napsp-adapter-singbox.workspace = true\n",
            # Governance comes first among the domains, so it cannot depend on a later one.
            "crates/governance/Cargo.toml": "[dependencies]\napsp-resources.workspace = true\n",
            # Every workspace crate needs a layer.
            "crates/billing/Cargo.toml": "[package]\n",
        }

        def tree(directory, files):
            root = Path(directory)
            for relative, text in files.items():
                (root / relative).parent.mkdir(parents=True, exist_ok=True)
                (root / relative).write_text(text)
            return root

        for path, manifest in broken.items():
            with self.subTest(path=path), tempfile.TemporaryDirectory() as directory:
                self.assertFalse(boundary.check_dependencies(tree(directory, {**manifests, path: manifest})))
        # A layered crate that moves away without updating the rules fails.
        with tempfile.TemporaryDirectory() as directory:
            root = tree(directory, {k: v for k, v in manifests.items() if k != "crates/naming/Cargo.toml"})
            self.assertFalse(boundary.check_dependencies(root))
        with tempfile.TemporaryDirectory() as directory:
            self.assertTrue(boundary.check_dependencies(tree(directory, manifests)))
        platform = {"apsp-protocol", "apsp-panel-host", "apsp-cloud-api"}
        self.assertEqual(boundary.domain_dependencies("governance"), platform)
        base = platform | {"apsp-governance"}
        self.assertEqual(boundary.domain_dependencies("resources"), base)
        self.assertEqual(boundary.domain_dependencies("networking"), base | {"apsp-resources"})
        self.assertEqual(boundary.domain_dependencies("naming"), base | {"apsp-resources", "apsp-networking"})
        before_costs = base | {"apsp-resources", "apsp-networking", "apsp-naming", "apsp-compiler",
                               "apsp-traffic", "apsp-access"}
        self.assertEqual(boundary.domain_dependencies("costs"), before_costs)
        self.assertEqual(boundary.domain_dependencies("observability"), before_costs)
        self.assertEqual(boundary.domain_dependencies("operations"),
                         before_costs | {"apsp-costs", "apsp-observability"})
        # Another crate of the same domain is allowed, the crate itself is not.
        self.assertEqual(boundary.domain_dependencies("traffic", "apsp-traffic"),
                         base | {"apsp-resources", "apsp-networking", "apsp-naming", "apsp-compiler"})
        self.assertEqual(boundary.domain_dependencies("access", "apsp-access"),
                         base | {"apsp-resources", "apsp-networking", "apsp-naming", "apsp-compiler",
                                 "apsp-traffic"})

if __name__ == "__main__":
    unittest.main()
