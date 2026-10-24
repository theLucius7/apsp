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
