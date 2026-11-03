"""Node import identity, privacy and retry behavior."""
import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import nexus_node_inventory as migration
from test_nexus_agent_migration import fixture


def receipt(envelope):
    return {"digest": envelope["digest"], "source_id": "fixture", "complete": True,
            "servers": [{"legacy_host_id": 1, "apsp_server_id": 20}]}


class Panel:
    password = "fixture-password"

    def __init__(self):
        self.requests, self.nodes = [], []
        self.fail_after_commit = False
        self.conflicts = []

    def call(self, path, method="GET", payload=None):
        self.requests.append((path, method))
        if path.endswith("/preview"):
            assert set(payload) == {"payload"}, "Raw source must use the audit-redacted envelope"
            return {"digest": "reviewed-digest", "conflicts": self.conflicts}
        if path.endswith("/reauth"):
            return {}
        if path.endswith("/import"):
            assert set(payload) == {"payload", "expected_digest", "confirm_staged"}
            already = bool(self.nodes)
            self.nodes = [{"id": 90, "legacy_node_id": 1, "source_id": "fixture", "state": "pending_agent",
                           "legacy_machine_id": 2, "legacy_host_id": 1, "server_id": 20}]
            if self.fail_after_commit:
                self.fail_after_commit = False
                raise ValueError("Lost response after atomic commit")
            return {"already_imported": already}
        if path == migration.ROOT:
            return self.nodes
        raise AssertionError("Unexpected API, possibly a runtime mutation")


class NodeInventoryTests(unittest.TestCase):
    def snapshot(self):
        envelope = fixture()
        with patch.object(migration, "snapshot", return_value=envelope), patch.object(migration, "rows", return_value=[]):
            return migration.node_snapshot(None, envelope, receipt(envelope))

    def test_original_node_and_machine_mapping_survive_export(self):
        value = self.snapshot()
        self.assertEqual(value["nodes"], fixture()["snapshot"]["nodes"])
        self.assertIn("PRIVATE_NODE_KEY", json.dumps(value))
        self.assertNotIn("private note", json.dumps(value))
        self.assertEqual(value["hosts"], [{"legacy_id": 1, "server_id": 20}])
        self.assertEqual([m["host_id"] for m in value["machines"]], [1, 1])
        self.assertEqual(value["format"], "apsp.nexus.nodes.v1")

    def test_changed_infrastructure_and_incomplete_receipt_fail_before_export(self):
        envelope = fixture()
        changed = {**envelope, "digest": "changed"}
        with patch.object(migration, "snapshot", return_value=changed):
            with self.assertRaisesRegex(ValueError, "Source infrastructure changed"):
                migration.node_snapshot(None, envelope, receipt(envelope))
        invalid = receipt(envelope)
        invalid["complete"] = False
        with self.assertRaisesRegex(ValueError, "complete host receipt"):
            migration.node_snapshot(None, envelope, invalid)

    def test_duplicate_and_missing_host_receipts_are_rejected(self):
        envelope = fixture()
        for servers in [[], [{"legacy_host_id": 1, "apsp_server_id": 20}] * 2]:
            invalid = receipt(envelope)
            invalid["servers"] = servers
            with patch.object(migration, "snapshot", return_value=envelope):
                with self.assertRaisesRegex(ValueError, "duplicate, missing or invalid"):
                    migration.node_snapshot(None, envelope, invalid)

    def test_lost_commit_response_retries_without_duplicate_or_secret_receipt(self):
        panel, value = Panel(), self.snapshot()
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "receipt.json"
            panel.fail_after_commit = True
            with self.assertRaisesRegex(ValueError, "Lost response"):
                migration.import_nodes(panel, value, "reviewed-digest", path)
            self.assertFalse(path.exists())
            result = migration.import_nodes(panel, value, "reviewed-digest", path)
            self.assertTrue(result["already_imported"])
            self.assertEqual(result["node_information_imported"], 1)
            self.assertFalse(result["runtime_ownership_changed"])
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertNotIn("PRIVATE_NODE_KEY", path.read_text())
            self.assertNotIn("fixture-password", path.read_text())
            self.assertEqual(len(panel.nodes), 1)

    def test_changed_preview_and_conflicts_never_import(self):
        panel, value = Panel(), self.snapshot()
        with self.assertRaisesRegex(ValueError, "exact node preview"):
            migration.import_nodes(panel, value, "wrong", "/unused")
        panel.conflicts = ["mapping conflict"]
        with self.assertRaisesRegex(ValueError, "preview has conflicts"):
            migration.import_nodes(panel, value, "reviewed-digest", "/unused")
        self.assertFalse(any(path.endswith("/import") for path, _ in panel.requests))

    def test_server_mismatch_is_not_recorded_as_success(self):
        panel, value = Panel(), copy.deepcopy(self.snapshot())
        value["hosts"][0]["server_id"] = 99
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "receipt.json"
            with self.assertRaisesRegex(ValueError, "host mapping or state differs"):
                migration.import_nodes(panel, value, "reviewed-digest", path)
            self.assertFalse(path.exists())


if __name__ == "__main__":
    unittest.main()


class AppendTests(unittest.TestCase):
    """Appending later nodes reads only their own slice of the source."""

    TABLES = {
        "v2_server": [
            {"id": 52, "machine_id": 34, "parent_id": None, "type": "vless", "protocol_settings": "{}"},
            {"id": 54, "machine_id": None, "parent_id": 52, "type": "vless", "protocol_settings": "{}"},
            {"id": 66, "machine_id": 40, "parent_id": None, "type": "vless", "protocol_settings": "{}"},
            {"id": 70, "machine_id": 41, "parent_id": None, "type": "vless", "protocol_settings": "{}"},
        ],
        "v2_infra_machine": [{"machine_id": 34, "host_id": 19}, {"machine_id": 40, "host_id": 20}],
        "v2_server_machine": [{"id": 34, "name": "hk5", "is_active": 1}, {"id": 40, "name": "alice-hk-cf", "is_active": 1},
                              {"id": 41, "name": "unlinked", "is_active": 1}],
        "v2_infra_host": [{"id": 19, "name": "alibaba-hk5", "address": "a", "notes": None, "metadata": "{}"},
                          {"id": 20, "name": "alice-hk", "address": "b", "notes": None, "metadata": "{}"}],
        "v2_server_group": [{"id": 2, "name": "group"}],
        "v2_server_route": [],
    }

    def rows(self, connection, table, columns, required=True):
        return copy.deepcopy(self.TABLES[table])

    def servers(self, *hosts):
        return [{"id": 100 + host, "asset_settings": {"tags": [migration.source_tag({"source_id": "fixture"}, host)]}}
                for host in hosts]

    def test_only_requested_nodes_and_their_hosts_are_exported(self):
        with patch.object(migration, "rows", self.rows):
            value = migration.append_snapshot(None, "fixture", [52, 66], self.servers(19, 20), now=7)
        self.assertEqual([n["id"] for n in value["nodes"]], [52, 66])
        self.assertEqual(value["hosts"], [{"legacy_id": 19, "server_id": 119}, {"legacy_id": 20, "server_id": 120}])
        self.assertEqual([(m["id"], m["host_id"]) for m in value["machines"]], [(34, 19), (40, 20)])
        self.assertEqual((value["format"], value["exported_at"]), ("apsp.nexus.nodes.v1", 7))
        self.assertEqual(len(value["infrastructure_digest"]), 64)

    def test_aliases_unlinked_machines_untagged_hosts_and_duplicates_are_refused(self):
        with patch.object(migration, "rows", self.rows):
            for nodes, servers in ([[54], self.servers(19)], [[70], self.servers(19)], [[66], self.servers(19)],
                                   [[52, 52], self.servers(19)], [[99], self.servers(19)], [[52], self.servers(19) * 2]):
                with self.assertRaises(ValueError):
                    migration.append_snapshot(None, "fixture", nodes, servers)

    def test_link_tags_once_and_refuses_another_owner(self):
        class LinkPanel:
            def __init__(self, servers):
                self.servers, self.put = servers, None

            def call(self, path, method="GET", payload=None):
                if path == "/api/servers":
                    return copy.deepcopy(self.servers)
                if method == "PATCH":
                    self.put = payload
                    return {}
                return {"id": 19, "name": "alice-hk", "asset_settings": {"tags": ["keep"], "region": "HK"}}

        panel = LinkPanel([{"id": 19, "asset_settings": {"tags": ["keep"]}}])
        self.assertTrue(migration.link_server(panel, "fixture", 20, 19))
        self.assertEqual(panel.put["asset_settings"]["tags"], ["keep", migration.source_tag({"source_id": "fixture"}, 20)])
        self.assertEqual(panel.put["asset_settings"]["region"], "HK")
        with self.assertRaises(ValueError):
            migration.link_server(LinkPanel(self.servers(20)), "fixture", 20, 19)
