#!/usr/bin/env python3
"""Private SQLite and stopped-manager fixtures; never install or execute a payload."""

import contextlib
import json
from pathlib import Path
import socket
import sqlite3
import subprocess
import shlex
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import legacy_agent_checkpoint as guard
import release

ID = "00000000-0000-4000-8000-000000000001"
VERSION = "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r2"


def preparing(mode="full"):
    return {"Preparing": {"id": ID, "plugin": "nodequality", "version": VERSION,
                          "artifact": {"url": "https://panel.example.com/artifact", "sha256": "a" * 64,
                                       "proof": {"metadata_json": "TEST_ONLY", "checksums": "TEST_ONLY", "signature": "TEST_ONLY"}},
                          "timeout_secs": 60, "expires_at": None, "options": {"mode": mode}}}


def started():
    directory = "/opt/apsp/diagnostics/" + ID
    binary = "/opt/apsp/plugins/nodequality/" + VERSION + "/nodequality"
    return {"Started": {"spec": {"id": ID, "version": VERSION, "binary_path": binary, "job_dir": directory,
                                  "timeout_secs": 60, "options": {}},
                        "service": {"unit": "apsp-diagnostic-" + ID + ".service", "program": binary,
                                    "args": [], "working_directory": directory, "timeout_secs": 60},
                        "plugin": "nodequality", "started_at": 1}}


class LegacyCheckpointTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="apsp-private-legacy-checkpoint-")
        self.root = Path(self.temporary.name)
        self.configuration = self.root / "agent.toml"
        self.database = self.root / "state.db"
        self.configuration.write_text('state_db = "' + str(self.database) + '"\n')

    def tearDown(self):
        self.temporary.cleanup()

    def checkpoint(self, encoded, wal=False):
        writer = sqlite3.connect(self.database)
        if wal:
            self.assertEqual(writer.execute("PRAGMA journal_mode=WAL").fetchone()[0], "wal")
            writer.execute("PRAGMA wal_autocheckpoint=0")
        writer.execute("CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY,value TEXT)")
        writer.execute("INSERT OR REPLACE INTO kv VALUES ('diagnostics:active',?)", (encoded,))
        writer.commit()
        if wal:
            return writer
        writer.close()
        return None

    def inspect(self, refused=False, reason=None):
        before = {path.name: path.read_bytes() for path in self.root.iterdir()
                  if path.name == "agent.toml" or path.name == "state.db" or path.name.endswith("-wal")}
        with patch.object(guard, "protected"), patch.object(guard, "quiescent") as quiet:
            if refused:
                with self.assertRaisesRegex(ValueError, reason or ".*"):
                    guard.preflight("0.3.0", self.configuration)
            else:
                guard.preflight("0.3.0", self.configuration)
            quiet.assert_called_once()
        self.assertEqual(before, {name: (self.root / name).read_bytes() for name in before})

    def test_preparing_full_and_missing_legacy_fields_are_refused_without_writes(self):
        variants = [preparing(), preparing()]
        variants[1]["Preparing"].pop("plugin")
        for remove_options in (False, True):
            value = preparing()
            if remove_options:
                value["Preparing"].pop("options")
            else:
                value["Preparing"]["options"].pop("mode")
            variants.append(value)
        for value in variants:
            with self.subTest(value=value):
                self.checkpoint(json.dumps(value, indent=2))
                self.inspect(refused=True, reason="完整验机")

    def test_stable_live_wal_is_read_not_ignored_and_original_json_is_unchanged(self):
        encoded = json.dumps(preparing(), indent=4)
        writer = self.checkpoint(encoded, wal=True)
        try:
            self.assertGreater(Path(str(self.database) + "-wal").stat().st_size, 0)
            self.inspect(refused=True, reason="完整验机")
            self.assertEqual(writer.execute("SELECT value FROM kv").fetchone()[0], encoded)
        finally:
            writer.close()

    def test_daily_and_exact_started_checkpoint_are_preserved(self):
        for value in (preparing("daily"), started(), None):
            with self.subTest(value=value):
                encoded = json.dumps(value, indent=4)
                self.checkpoint(encoded)
                self.inspect()
                with contextlib.closing(sqlite3.connect(self.database)) as reader:
                    self.assertEqual(reader.execute("SELECT value FROM kv").fetchone()[0], encoded)

    def test_unknown_corrupt_or_duplicate_checkpoint_is_refused(self):
        invalid = ["[]", "{}", '{"Unknown":{}}', '{"Preparing":null}', '{"Preparing":{},"Preparing":{}}',
                   '{"Started":{"spec":{},"service":{},"plugin":"nodequality","started_at":1}}', "NaN", "{broken"]
        for mutation in (lambda value: value["Preparing"].update(plugin="unknown"),
                         lambda value: value["Preparing"].update(timeout_secs=True),
                         lambda value: value["Preparing"].update(options={"mode": "unknown"}),
                         lambda value: value["Preparing"].update(artifact={}),
                         lambda value: value["Preparing"].update(resource_budget="bad")):
            value = preparing("daily")
            mutation(value)
            invalid.append(json.dumps(value))
        for encoded in invalid:
            with self.subTest(encoded=encoded):
                self.checkpoint(encoded)
                self.inspect(refused=True)

    def test_newer_started_history_is_kept_for_a_compatible_agent_without_downgrade(self):
        for mutate in (lambda value: value["Started"]["spec"].update(version=VERSION[:-1] + "4"),
                       lambda value: value["Started"]["spec"].update(options={"mode": "full"})):
            value = started()
            mutate(value)
            self.checkpoint(json.dumps(value, indent=4))
            self.inspect(refused=True, reason="原版本回收")

    def test_missing_wal_shared_memory_is_refused_without_creating_it(self):
        writer = self.checkpoint(json.dumps(preparing()), wal=True)
        try:
            # Preserve live files: model a missing sidecar by changing only the lookup result.
            real_exists = Path.exists
            shared = Path(str(self.database) + "-shm")
            with patch.object(Path, "exists", lambda path: False if path == shared else real_exists(path)), \
                    patch.object(guard, "protected"), patch.object(guard, "quiescent"), \
                    self.assertRaisesRegex(ValueError, "WAL"):
                guard.preflight("0.3.0", self.configuration)
        finally:
            writer.close()

    def test_old_default_state_path_is_read_when_not_explicitly_configured(self):
        self.checkpoint(json.dumps(preparing()))
        self.configuration.write_text('panel_url="https://panel.example.com"\n')
        real_path = Path
        mapped = lambda value: self.database if str(value) == "/var/lib/apsp/core/state.db" else real_path(value)
        with patch.object(guard, "Path", side_effect=mapped), patch.object(guard, "protected"), \
                patch.object(guard, "quiescent"), self.assertRaisesRegex(ValueError, "完整验机"):
            guard.preflight("0.3.0", self.configuration)

    def test_a_writer_change_during_read_is_refused_instead_of_claiming_atomicity(self):
        writer = self.checkpoint("null", wal=True)
        real_connect = sqlite3.connect
        changed = json.dumps(preparing())

        class ConcurrentRead:
            def __init__(self, *args, **kwargs):
                self.connection = real_connect(*args, **kwargs)

            def set_progress_handler(self, *args):
                self.connection.set_progress_handler(*args)

            def execute(self, *args):
                cursor = self.connection.execute(*args)
                writer.execute("UPDATE kv SET value=? WHERE key='diagnostics:active'", (changed,))
                writer.commit()
                return cursor

            def close(self):
                self.connection.close()

        try:
            with patch.object(guard, "protected"), patch.object(guard, "quiescent"), \
                    patch.object(sqlite3, "connect", side_effect=ConcurrentRead), \
                    self.assertRaisesRegex(ValueError, "预检期间变化"):
                guard.preflight("0.3.0", self.configuration)
            self.assertEqual(writer.execute("SELECT value FROM kv").fetchone()[0], changed)
        finally:
            writer.close()

    def test_running_or_uncertain_manager_and_live_status_socket_are_refused(self):
        manager = self.root / "manager"
        manager.mkdir()
        real_path = Path
        mapped = lambda value: manager if str(value) == "/run/systemd/system" else real_path(value)
        for output in (b"LoadState=loaded\nActiveState=active\nSubState=running\nMainPID=123\n",
                       b"LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\nMainPID=0\n"):
            with self.subTest(output=output), patch.object(guard, "Path", side_effect=mapped), \
                    patch.object(guard.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, output, b"")), \
                    self.assertRaises(ValueError):
                guard.quiescent({})
        endpoint = self.root / "agent.sock"
        listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        listener.bind(str(endpoint))
        listener.listen()
        try:
            stopped = b"LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\n"
            with patch.object(guard, "Path", side_effect=mapped), \
                    patch.object(guard.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, stopped, b"")), \
                    self.assertRaisesRegex(ValueError, "仍在运行"):
                guard.quiescent({"status_socket": str(endpoint)})
        finally:
            listener.close()

    def test_stopped_and_fresh_managers_have_explicit_positive_cases(self):
        manager = self.root / "manager"
        manager.mkdir()
        real_path = Path
        mapped = lambda value: manager if str(value) == "/run/systemd/system" else real_path(value)
        for load, allowed in (("loaded", False), ("not-found", True)):
            output = f"LoadState={load}\nActiveState=inactive\nSubState=dead\nMainPID=0\n".encode()
            with patch.object(guard, "Path", side_effect=mapped), \
                    patch.object(guard.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, output, b"")):
                guard.quiescent({"status_socket": str(self.root / "absent.sock")}, allow_missing=allowed)
        softlevel = self.root / "softlevel"
        softlevel.write_text("TEST_ONLY")
        mapped = lambda value: (softlevel if str(value) == "/run/openrc/softlevel" else
                                self.root / "no-systemd" if str(value) == "/run/systemd/system" else real_path(value))
        for exists, status, missing in ((0, 3, False), (1, 1, True)):
            with patch.object(guard, "Path", side_effect=mapped), \
                    patch.object(guard.subprocess, "run", side_effect=[subprocess.CompletedProcess([], exists, b"", b""),
                                                                     subprocess.CompletedProcess([], status, b"", b"")]):
                guard.quiescent({"status_socket": str(self.root / "absent.sock")}, allow_missing=missing)

    def test_signed_linux_executor_runs_the_same_gate_before_enrollment_and_activation(self):
        template = (ROOT / "deploy/install.sh.tmpl").read_text()
        self.assertEqual(template.count("@@LEGACY_CHECKPOINT_PREFLIGHT@@"), 2)
        rendered = release.installer_source(ROOT / "deploy/install.sh.tmpl", ROOT / "deploy/apsp-agent.service",
                                           ROOT / "plugins/sing-box/apsp-singbox@.service", source_root=ROOT)
        self.assertEqual(rendered.count((ROOT / "tools/legacy_agent_checkpoint.py").read_text().rstrip()), 2)
        first, last = rendered.index('python3 -I - "$VERSION"'), rendered.rindex('python3 -I - "$VERSION"')
        enroll = rendered.index('"/opt/apsp/core/$VERSION/apsp-agent" enroll')
        self.assertLess(first, enroll)
        self.assertLess(enroll, last)
        self.assertLess(last, rendered.index("ACTIVATING=1"))

    def test_final_preflight_failure_restores_old_config_without_starting_a_service(self):
        template = (ROOT / "deploy/install.sh.tmpl").read_text()
        cleanup = template.split("cleanup() {", 1)[1].split("\ntrap cleanup EXIT", 1)[0]
        download = self.root / "download"
        download.mkdir()
        previous = b'TEST_ONLY original configuration\n'
        (download / "agent.toml.previous").write_bytes(previous)
        configuration = self.root / "restored.toml"
        configuration.write_bytes(b'TEST_ONLY changed during enrollment\n')
        cleanup = cleanup.replace("/etc/apsp/agent.toml", str(configuration))
        script = ("DOWNLOAD=" + shlex.quote(str(download)) + "; STAGE=; ACTIVATING=0; COMPLETED=0; CONFIGURATION_CHANGED=1;\n"
                  + "cleanup() {" + cleanup + "\ntrap cleanup EXIT\nexit 7\n")
        result = subprocess.run(["/bin/sh", "-c", script], capture_output=True, timeout=3)
        self.assertEqual(result.returncode, 7)
        self.assertEqual(configuration.read_bytes(), previous)
        self.assertFalse(download.exists())


if __name__ == "__main__":
    unittest.main()
