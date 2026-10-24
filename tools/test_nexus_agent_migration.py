"""Failure recovery and confidentiality checks for host-only staging."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

import nexus_agent_migration as migration


def fixture():
    value = {"format": migration.FORMAT, "source_id": "fixture",
             "hosts": [{"id": 1, "name": "测试服务器", "address": "host.example.invalid", "notes": "private note", "metadata": "{}"}],
             "machines": [{"id": 1, "name": "instance-1", "is_active": 1}, {"id": 2, "name": "instance-2", "is_active": 1}],
             "machine_hosts": [{"machine_id": 1, "host_id": 1}, {"machine_id": 2, "host_id": 1}],
             "nodes": [{"id": 1, "machine_id": 2, "type": "vless", "protocol_settings": json.dumps({"tls": 2, "network": "tcp", "reality_settings": {"short_id": "0123456789abcdef", "private_key": "PRIVATE_NODE_KEY"}}),
                        "rate": "2.5", "rate_time_enable": 0, "custom_routes": "[]", "custom_outbounds": "[]", "port": "443", "server_port": 443}],
             "connections": [{"id": 1, "host_id": 1, "host_keys": "[]"}]}
    return {"snapshot": value, "digest": migration.digest(value), "exported_at": 1}


class FakePanel:
    def __init__(self):
        self.servers, self.requests = [], []
        self.fail_after_create = False

    def call(self, path, method="GET", payload=None):
        self.requests.append((path, method))
        if path != "/api/servers":
            raise AssertionError("Staging must only use the host inventory API")
        if method == "GET":
            return copy.deepcopy(self.servers)
        server = {"id": len(self.servers) + 20, **copy.deepcopy(payload)}
        self.servers.append(server)
        if self.fail_after_create:
            self.fail_after_create = False
            raise ValueError("Lost response after creation")
        return server


class MigrationTests(unittest.TestCase):
    def test_physical_hosts_merge_machine_records_and_preview_hides_secrets(self):
        report = migration.readiness(fixture())
        self.assertEqual((report["host_count"], report["machine_count"], report["node_count"]), (1, 2, 1))
        self.assertEqual(report["hosts"][0]["machine_ids"], [1, 2])
        rendered = json.dumps(report)
        for secret in ["PRIVATE_NODE_KEY", "private note", "host.example.invalid"]:
            self.assertNotIn(secret, rendered)
        self.assertFalse(report["ready_for_cutover"])
        self.assertIn("traffic_multiplier_mapping_required", report["nodes"][0]["reasons"])
        self.assertIn("existing_reality_short_id_not_accepted", report["nodes"][0]["reasons"])

    def test_signed_old_name_is_not_rewritten_into_current_identity(self):
        release = {"source_repo": "theLucius7/sinan", "tag": "agent-v0.3.0", "artifacts": [{"name": "agent", "binary_name": "sinan-agent"}]}
        original = copy.deepcopy(release)
        report = migration.readiness(fixture(), release)
        self.assertEqual(release, original)
        self.assertEqual(report["release"]["status"], "incompatible_identity")
        self.assertFalse(report["release"]["signature_verified"])

    def test_digest_and_mapping_conflicts_fail_before_any_panel_write(self):
        with tempfile.TemporaryDirectory() as temporary:
            receipt = Path(temporary) / "receipt.json"
            panel, envelope = FakePanel(), fixture()
            envelope["snapshot"]["nodes"][0]["rate"] = "10"
            with self.assertRaisesRegex(ValueError, "digest mismatch"):
                migration.stage(panel, envelope, envelope["digest"], receipt)
            self.assertEqual(panel.requests, [])
            envelope = fixture()
            server = {"id": 20, **migration.server_request(envelope["snapshot"], envelope["snapshot"]["hosts"][0])}
            panel.servers = [server, {**server, "id": 21}]
            with self.assertRaisesRegex(ValueError, "Duplicate source tags"):
                migration.stage(panel, envelope, envelope["digest"], receipt)
            self.assertFalse(any(method == "POST" for _, method in panel.requests))

    def test_lost_creation_response_recovers_without_duplicates_or_agent_changes(self):
        with tempfile.TemporaryDirectory() as temporary:
            receipt = Path(temporary) / "receipt.json"
            panel, envelope = FakePanel(), fixture()
            panel.fail_after_create = True
            with self.assertRaisesRegex(ValueError, "Lost response"):
                migration.stage(panel, envelope, envelope["digest"], receipt)
            self.assertFalse(json.loads(receipt.read_text())["complete"])
            result = migration.stage(panel, envelope, envelope["digest"], receipt)
            self.assertTrue(result["complete"])
            self.assertTrue(result["servers"][0]["reused"])
            self.assertEqual(len(panel.servers), 1)
            self.assertFalse(result["agents_installed"])
            self.assertFalse(result["nodes_transferred"])
            self.assertEqual(receipt.stat().st_mode & 0o777, 0o600)
            self.assertFalse(panel.servers[0]["asset_settings"]["offline_notify"])
            self.assertFalse(panel.servers[0]["agent_settings"]["auto_update"])
            self.assertNotIn("static_info", panel.servers[0])

    def test_unlinked_existing_host_is_not_duplicated_by_name(self):
        with tempfile.TemporaryDirectory() as temporary:
            panel, envelope = FakePanel(), fixture()
            panel.servers.append({"id": 17, "name": "测试服务器", "asset_settings": {"tags": []}})
            with self.assertRaisesRegex(ValueError, "existing unlinked server"):
                migration.stage(panel, envelope, envelope["digest"], Path(temporary) / "receipt.json")
            self.assertFalse(any(method == "POST" for _, method in panel.requests))

    def test_existing_operator_edits_survive_and_deleted_mapping_is_not_recreated(self):
        with tempfile.TemporaryDirectory() as temporary:
            receipt = Path(temporary) / "receipt.json"
            panel, envelope = FakePanel(), fixture()
            migration.stage(panel, envelope, envelope["digest"], receipt)
            panel.servers[0]["name"] = "管理员新名称"
            panel.servers[0]["asset_settings"]["price"] = "12.34"
            migration.stage(panel, envelope, envelope["digest"], receipt)
            self.assertEqual(panel.servers[0]["name"], "管理员新名称")
            self.assertEqual(panel.servers[0]["asset_settings"]["price"], "12.34")
            panel.servers.clear()
            with self.assertRaisesRegex(ValueError, "missing or has changed identity"):
                migration.stage(panel, envelope, envelope["digest"], receipt)
            self.assertEqual(panel.servers, [])

    def test_source_quota_semantics_are_not_guessed_and_money_is_not_rounded(self):
        value = fixture()["snapshot"]
        host = value["hosts"][0]
        host["metadata"] = {"asset": {"renewal_price": "15.00", "currency": "USD", "monthly_traffic_gb": 180, "expires_on": "2027-01-27"}}
        result = migration.server_request(value, host)["asset_settings"]
        self.assertEqual(result["price"], "15.00")
        self.assertEqual(result["currency"], "USD")
        self.assertNotIn("traffic_limit", result)
        self.assertFalse(result["auto_renewal"])
        host["metadata"]["asset"]["renewal_price"] = "15.001"
        with self.assertRaisesRegex(ValueError, "renewal price"):
            migration.server_request(value, host)

    def test_hysteria_port_range_and_certificate_require_explicit_handoff(self):
        envelope = fixture()
        node = envelope["snapshot"]["nodes"][0]
        node.update(type="hysteria", port="21000-21020", server_port=21000)
        envelope["digest"] = migration.digest(envelope["snapshot"])
        reasons = migration.readiness(envelope)["nodes"][0]["reasons"]
        self.assertIn("public_port_or_port_hopping_mapping_required", reasons)
        self.assertIn("certificate_and_renewal_handoff_required", reasons)
        self.assertIn("existing_protocol_credentials_mapping_required", reasons)


if __name__ == "__main__":
    unittest.main()
