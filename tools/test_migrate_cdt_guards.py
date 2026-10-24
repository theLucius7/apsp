"""Offline import regression tests; no shell config or cloud calls are executed."""
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("migrate_cdt", Path(__file__).with_name("migrate-cdt-guards.py"))
m = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = m
spec.loader.exec_module(m)


class FakePanel:
    origin = "http://127.0.0.1:18090"

    def __init__(self):
        self.overview = {"accounts": [], "resources": []}
        self.writes = []

    def request(self, path, method="GET", body=None):
        if method == "GET":
            return self.overview if path == "/api/plugins/alicloud" else {"encryption_ready": True}
        self.writes.append((path, body))
        if path.endswith("/credentials"):
            return {"id": "cred-" + str(len(self.writes))}
        if path.endswith("/accounts"):
            account = dict(body, id="account-" + str(len(self.writes)), revision=1)
            self.overview["accounts"].append(account)
            return account
        if path.endswith("/resources"):
            resource = dict(body, id="resource-" + str(len(self.writes)), revision=1, power_policy={"enabled": False})
            self.overview["resources"].append(resource)
            return resource
        resource = next(r for r in self.overview["resources"] if r["id"] == path.split("/")[-2])
        assert resource["revision"] == body["revision"]
        resource["revision"] += 1
        if path.endswith("/power-policy"):
            resource["power_policy"] = body["policy"]


class ImportTests(unittest.TestCase):
    def fixture(self, root):
        (root / "state").mkdir()
        for index, (filename, _, _) in enumerate(m.PROFILES):
            instance = f"i-test{index}"
            (root / filename).write_text(f"ALIYUN_ACCESS_KEY_ID='TEST_ONLY_KEY_{index}'\nALIYUN_ACCESS_KEY_SECRET='TEST_ONLY_SECRET'\nALIYUN_REGION_ID='cn-hongkong'\nALIYUN_ECS_INSTANCE_ID='{instance}'\nTRAFFIC_THRESHOLD_GB='150'\n")
            (root / "state" / f"{instance}.json").write_text(json.dumps({"version": 1,
                "identity": {"instance_id": instance, "region_id": "cn-hongkong", "quota_pool": "non-mainland"},
                "billing_cycle": "2026-10", "max_usage_bytes": str(110 * m.GIB), "max_jump_bytes": str(45 * m.GIB),
                "observed_at": "2026-10-01T08:00:00+08:00"}))

    def test_import_keeps_two_credentials_separate_pauses_both_and_resumes_without_duplicates(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixture(root)
            profiles = m.read_profiles(root)
            self.assertEqual(profiles[0].threshold, 150 * m.GIB)
            self.assertEqual(profiles[0].jump, 20 * m.GIB)
            self.assertNotIn("TEST_ONLY", repr(profiles))
            panel, journal = FakePanel(), root / "journal.json"
            m.prepare(panel, profiles, journal)
            m.prepare(panel, profiles, journal)
            self.assertEqual(len(panel.overview["accounts"]), 2)
            self.assertEqual(len(panel.overview["resources"]), 2)
            for account in panel.overview["accounts"]:
                self.assertFalse(account["enabled"])
                self.assertIn("credential_id", account)
                self.assertNotIn("access_key_secret", account)
            for resource in panel.overview["resources"]:
                self.assertFalse(resource["power_policy"]["enabled"])
                self.assertEqual(resource["power_policy"]["cdt_guard"]["threshold_bytes"], 150 * m.GIB)
            self.assertNotIn("TEST_ONLY", journal.read_text())
            panel.overview["accounts"][0]["enabled"] = True
            with self.assertRaises(m.MigrationError):
                m.prepare(panel, profiles, journal)
            panel.overview["accounts"][0]["enabled"] = False
            panel.overview["resources"][0]["cloud_id"] = "i-other"
            writes = len(panel.writes)
            with self.assertRaises(m.MigrationError):
                m.prepare(panel, profiles, journal)
            self.assertEqual(len(panel.writes), writes)

    def test_config_is_never_executed_and_missing_history_is_not_zero(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            self.fixture(root)
            (root / "config.env").write_text("ALIYUN_ACCESS_KEY_ID=$(touch forbidden)\n")
            with self.assertRaises(m.MigrationError):
                m.read_profiles(root)
            self.assertFalse((root / "forbidden").exists())
            (root / "state" / "i-test1.json").unlink()
            (root / "config.env").write_text((root / "config-hk-cdt-200.env").read_text())
            with self.assertRaises(FileNotFoundError):
                m.read_profiles(root)

    def test_thresholds_preserve_binary_units_and_reject_nonfinite_values(self):
        self.assertEqual(m.gib_bytes("150"), 161061273600)
        self.assertEqual(m.gib_bytes("0.5"), 536870912)
        for value in ["NaN", "Infinity", "-1", "0", "garbage", "0.0000000000001"]:
            with self.assertRaises(m.MigrationError):
                m.gib_bytes(value)


if __name__ == "__main__":
    unittest.main()
