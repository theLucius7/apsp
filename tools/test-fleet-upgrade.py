#!/usr/bin/env python3
"""Offline fleet orchestration failures; no panel, Agent, SSH, or cloud access."""

import contextlib
import copy
import importlib.util
import io
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

SOURCE = Path(__file__).with_name("fleet-upgrade.py")
spec = importlib.util.spec_from_file_location("fleet_upgrade", SOURCE)
fleet = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fleet)
VERSION = "0.4.0"
RUNTIME = "1.14.4-apsp.2"


class Clock:
    def __init__(self):
        self.value = 0

    def now(self):
        return self.value

    def sleep(self, seconds):
        self.value += seconds


class Panel:
    def __init__(self, ids=(1,), runtime=False):
        self.values = {
            identifier: {
                "id": identifier,
                "name": "TEST_ONLY",
                "online": True,
                "static_info": {
                    "os": "linux",
                    "arch": "x86_64",
                    "libc": "musl",
                    "agent_version": "0.3.9",
                    "runtime_version": RUNTIME,
                },
                "agent_settings": {"auto_update": False},
            }
            for identifier in ids
        }
        self.has_runtime = runtime
        self.versions = [
            {"version": VERSION, "tag": "agent-v" + VERSION, "targets": ["amd64"]}
        ]
        self.calls = []
        self.enabled = set()
        self.upgrade = True
        self.runtime_error = None
        self.restore_error = False
        self.checkpoint_current = True
        self.runtime_state = "ready"
        self.preconditions = True
        self.precondition_checks = None
        self.changed_candidate = False
        self.panel_time = 1_700_000_000
        self.checkpoint_seq = 0
        self.deployment_healthy = True
        self.active_checkpoint = None
        self.checkpoint_outcomes = []
        self.upgraded_singbox_component = RUNTIME
        self.omit_upgraded_components = False
        self.expected_binding = {
            "deployment_id": "00000000-0000-4000-8000-000000000005",
            "module": "singbox",
            "revision": 5,
            "bundle_sha256": "a" * 64,
            "binding_digest": "b" * 64,
        }

    def call(self, path, method="GET", payload=None, timeout=45):
        self.calls.append((path, method, copy.deepcopy(payload)))
        if path == "/api/system/agent-releases/targets":
            return {
                "supported_targets": [
                    "linux-gnu-amd64",
                    "linux-gnu-arm64",
                    "linux-musl-amd64",
                ]
            }
        if path == "/api/system/agent-releases/import":
            return {"signature_verified": True}
        if path == "/api/system/agent-releases":
            versions = copy.deepcopy(self.versions)
            if self.changed_candidate and self.enabled:
                versions.append(
                    {"version": "0.4.1", "tag": "agent-v0.4.1", "targets": ["amd64"]}
                )
            return {"versions": versions}
        pieces = path.split("/")
        identifier = int(pieces[3] if pieces[2] == "servers" else pieces[4])
        value = self.values[identifier]
        if path == f"/api/servers/{identifier}":
            if method == "PATCH":
                if self.restore_error and payload["auto_update"] is False:
                    raise fleet.UpgradeError("panel_http_503")
                value["agent_settings"]["auto_update"] = payload["auto_update"]
                if payload["auto_update"]:
                    self.enabled.add(identifier)
                    if self.upgrade:
                        value["static_info"]["agent_version"] = VERSION
                        if not self.omit_upgraded_components:
                            value["static_info"]["components"] = {
                                "sing-box": self.upgraded_singbox_component
                            }
                        if self.runtime_state == "upgrade_required":
                            self.runtime_state = "ready"
                else:
                    self.enabled.discard(identifier)
            return copy.deepcopy(value)
        if path == f"/api/traffic/servers/{identifier}":
            if self.runtime_error is not None and self.enabled:
                error, self.runtime_error = self.runtime_error, None
                raise error
            state = self.runtime_state if self.has_runtime else "unconfigured"
            return {
                "id": identifier,
                "configured": self.has_runtime,
                "runtime_version": value["static_info"].get("runtime_version"),
                "status": {"state": state},
            }
        if path.endswith("/deployments/check"):
            reply = {"ready": self.preconditions}
            if self.precondition_checks is not None:
                reply["checks"] = copy.deepcopy(self.precondition_checks)
            return reply
        if path.endswith("/deployments"):
            return {
                "pending": False,
                "status": {
                    "healthy": self.deployment_healthy,
                    "target_rev": 5,
                    "applied_rev": 5,
                    "last_error": None,
                },
            }
        if path.endswith("/runtime-control/checkpoint"):
            self.checkpoint_seq += 1
            request_id = f"00000000-0000-4000-8000-{self.checkpoint_seq:012d}"
            created_at = self.panel_time + self.checkpoint_seq * 10
            expected = copy.deepcopy(self.expected_binding)
            self.active_checkpoint = {
                "request_id": request_id,
                "expected": expected,
                "created_at": created_at,
                "expires_at": created_at + 120,
            }
            self.deployment_healthy = False
            if self.runtime_state == "ready":
                self.runtime_state = "pending"
            return {
                "request": {
                    "request_id": request_id,
                    "expected": copy.deepcopy(expected),
                    "expires_at": created_at + 120,
                }
            }
        if path.endswith("/runtime-control"):
            active = self.active_checkpoint
            request_id = active["request_id"]
            created_at = active["created_at"]
            expected = copy.deepcopy(active["expected"])
            mode = self.checkpoint_outcomes.pop(0) if self.checkpoint_outcomes else "default"
            if mode == "expired":
                return {
                    "checkpoint_required": True,
                    "checkpoints": [],
                    "requests": [{
                        "request_id": request_id,
                        "module": "singbox",
                        "kind": "checkpoint",
                        "request_digest": "c" * 64,
                        "created_at": created_at,
                        "expires_at": created_at + 120,
                        "state": "expired",
                        "result": None,
                        "received_at": None,
                        "outcome": None,
                    }],
                }
            if mode == "displaced":
                return {
                    "checkpoint_required": True,
                    "checkpoints": [{
                        "module": "singbox",
                        "checkpoint": {
                            "binding": copy.deepcopy(expected),
                            "activation_id": "act-old",
                            "instance_id": "inst-old",
                            "healthy": True,
                        },
                        "current": True,
                        "verified_at": created_at - 30,
                        "minimum_revision": 0,
                        "barrier_request_id": None,
                        "barrier_current": False,
                    }],
                    "requests": [{
                        "request_id": "00000000-0000-4000-8000-999999999999",
                        "module": "singbox",
                        "kind": "checkpoint",
                        "request_digest": "d" * 64,
                        "created_at": created_at + 1,
                        "expires_at": created_at + 121,
                        "state": "pending",
                        "result": None,
                        "received_at": None,
                        "outcome": None,
                    }],
                }
            if mode in ("failed", "rejected", "mismatch"):
                return {
                    "checkpoint_required": True,
                    "checkpoints": [],
                    "requests": [{
                        "request_id": request_id,
                        "module": "singbox",
                        "kind": "checkpoint",
                        "request_digest": "c" * 64,
                        "created_at": created_at,
                        "expires_at": created_at + 120,
                        "state": "received",
                        "result": {
                            "request_id": request_id,
                            "request_digest": "c" * 64,
                            "observed": None,
                            "success": False,
                            "error": "checkpoint failed",
                        },
                        "received_at": created_at + 2,
                        "outcome": mode,
                    }],
                }
            observed_binding = copy.deepcopy(expected)
            if mode == "wrong_binding":
                observed_binding["revision"] = 4
            received_at = (created_at + 2) if self.checkpoint_current else (created_at - 10)
            if self.checkpoint_current and mode == "default":
                self.deployment_healthy = True
                if self.runtime_state == "pending":
                    self.runtime_state = "ready"
            observed = {
                "binding": observed_binding,
                "activation_id": "act-1",
                "instance_id": "inst-1",
                "healthy": True,
            }
            return {
                "checkpoint_required": True,
                "checkpoints": [{
                    "module": "singbox",
                    "checkpoint": copy.deepcopy(observed),
                    "current": True,
                    "verified_at": received_at,
                    "minimum_revision": 0,
                    "barrier_request_id": None,
                    "barrier_current": False,
                }],
                "requests": [{
                    "request_id": request_id,
                    "module": "singbox",
                    "kind": "checkpoint",
                    "request_digest": "c" * 64,
                    "created_at": created_at,
                    "expires_at": created_at + 120,
                    "state": "received",
                    "result": {
                        "request_id": request_id,
                        "request_digest": "c" * 64,
                        "observed": observed,
                        "success": True,
                        "error": None,
                    },
                    "received_at": received_at,
                    "outcome": "verified",
                }],
            }
        raise AssertionError("unexpected TEST_ONLY route")


class FleetTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.path = Path(self.directory.name) / "receipt.json"
        self.clock = Clock()

    def receipt(self, ids=(1,)):
        return fleet.Receipt(self.path, list(ids), VERSION)

    def upgrade(self, panel, receipt, **kwargs):
        kwargs.setdefault("client_check", "/TEST_ONLY/client-check")
        return fleet.upgrade_one(
            panel,
            1,
            VERSION,
            receipt,
            3,
            1,
            clock=self.clock.now,
            sleep=self.clock.sleep,
            **kwargs,
        )

    def test_cli_does_not_accept_separate_runtime_version_flag(self):
        args = fleet.arguments(["--servers", "1", "--agent-version", VERSION])
        self.assertEqual(args.agent_version, VERSION)
        self.assertFalse(hasattr(args, "runtime_version"))
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            fleet.arguments(["--servers", "1", "--agent-version", VERSION, "--runtime-version", RUNTIME])

    def test_plan_never_changes_agent_settings_or_requests_checkpoint(self):
        panel = Panel((1, 2), runtime=True)
        result = fleet.plan(panel, [1, 2], VERSION)
        self.assertTrue(all(item["ready"] for item in result))
        self.assertEqual(result[0]["target_runtime_source"], "target_signed_agent_report")
        self.assertFalse(
            any(method == "PATCH" or "checkpoint" in path for path, method, _ in panel.calls)
        )

    def test_signed_catalog_embedded_components_drive_plan_and_reject_mismatched_agent_report(self):
        panel = Panel(runtime=True)
        panel.versions[0]["target_components"] = {
            "amd64": {"sing-box": {"version": RUNTIME, "sha256": "1" * 64}}
        }
        panel.versions[0]["target_digests"] = {"amd64": "9" * 64}
        planned = fleet.plan(panel, [1], VERSION)
        self.assertTrue(planned[0]["ready"])
        self.assertEqual(planned[0]["target_runtime_version"], RUNTIME)
        self.assertEqual(planned[0]["target_runtime_source"], "signed_release_manifest")

        # Mismatched embedded sing-box version vs catalog manifest fails closed immediately,
        # even if runtime_version == components["sing-box"].
        panel.upgraded_singbox_component = "1.14.4-apsp.1"
        panel.values[1]["static_info"]["runtime_version"] = "1.14.4-apsp.1"
        panel.values[1]["static_info"]["component_digests"] = {
            "agent": "9" * 64,
            "sing-box": "1" * 64,
        }
        with self.receipt() as receipt, self.assertRaisesRegex(
            fleet.UpgradeError, "embedded_runtime_differs_from_signed_release"
        ):
            self.upgrade(panel, receipt)

    def test_missing_embedded_component_or_conflicting_agent_digest_never_fakes_success(self):
        panel = Panel(runtime=True)
        panel.omit_upgraded_components = True
        with self.receipt() as receipt, self.assertRaisesRegex(fleet.UpgradeError, "timed_out"):
            self.upgrade(panel, receipt)

        self.clock = Clock()
        panel2 = Panel(runtime=True)
        panel2.values[1]["static_info"]["agent_sha256"] = "a" * 64
        panel2.values[1]["static_info"]["component_digests"] = {"agent": "b" * 64}
        with fleet.Receipt(self.path.with_name("r2.json"), [1], VERSION) as receipt2:
            with self.assertRaisesRegex(fleet.UpgradeError, "digest_conflict"):
                self.upgrade(panel2, receipt2)

    def test_legacy_agent_upgrade_required_state_plans_and_upgrades_to_self_contained(self):
        panel = Panel(runtime=True)
        panel.runtime_state = "upgrade_required"
        panel.preconditions = False
        panel.precondition_checks = [
            {"name": "Agent 接入", "passed": True, "detail": "已接入"},
            {"name": "设备在线", "passed": True, "detail": "在线"},
            {"name": "内置运行时能力", "passed": False, "detail": "待升级一体化 Agent"},
        ]
        planned = fleet.plan(panel, [1], VERSION)
        self.assertTrue(planned[0]["ready"])
        ok_proc = type("Result", (), {"returncode": 0})()
        with self.receipt() as receipt, mock.patch.object(
            fleet.subprocess, "run", return_value=ok_proc
        ) as runner:
            result = self.upgrade(panel, receipt)
        self.assertEqual(result["state"], "control_plane_verified")
        self.assertEqual(result["target_runtime_version"], RUNTIME)
        self.assertEqual(result["external_client_check"], "passed")
        runner.assert_called_once_with(
            ["/TEST_ONLY/client-check", "1", RUNTIME],
            stdin=fleet.subprocess.DEVNULL,
            stdout=fleet.subprocess.DEVNULL,
            stderr=fleet.subprocess.DEVNULL,
            timeout=mock.ANY,
            check=False,
        )

    def test_runtime_artifact_precondition_failure_is_not_ready(self):
        panel = Panel(runtime=True)
        panel.preconditions = False
        result = fleet.plan(panel, [1], VERSION)
        self.assertFalse(result[0]["ready"])
        self.assertEqual(result[0]["error"], "runtime_preconditions_or_signed_artifact_missing")
        self.assertFalse(panel.enabled)

    def test_monitor_verification_does_not_require_a_runtime(self):
        panel = Panel()
        with self.receipt() as receipt:
            result = self.upgrade(panel, receipt)
            self.assertEqual(result["mode"], "monitor")
            self.assertEqual(result["external_client_check"], "not_applicable")
            self.assertEqual(result["state"], "control_plane_verified")
        self.assertFalse(
            any("deployments" in path or "runtime-control" in path for path, _, _ in panel.calls)
        )
        self.assertFalse(panel.values[1]["agent_settings"]["auto_update"])

    def test_latest_signed_candidate_must_equal_the_fixed_target(self):
        panel = Panel()
        panel.versions.append({"version": "0.4.1", "tag": "agent-v0.4.1", "targets": ["amd64"]})
        with self.receipt() as receipt, self.assertRaisesRegex(
            fleet.UpgradeError, "candidate_changed"
        ):
            self.upgrade(panel, receipt)
        self.assertFalse(any(method == "PATCH" for _, method, _ in panel.calls))

    def test_candidate_drift_after_enable_aborts_and_restores(self):
        panel = Panel()
        panel.changed_candidate = True
        with self.receipt() as receipt, self.assertRaisesRegex(
            fleet.UpgradeError, "candidate_changed"
        ):
            self.upgrade(panel, receipt)
        self.assertFalse(panel.values[1]["agent_settings"]["auto_update"])
        self.assertFalse(json.loads(self.path.read_text())["members"]["1"]["restore_required"])

    def test_timeout_cannot_report_success_on_the_old_agent(self):
        panel = Panel()
        panel.upgrade = False
        with self.receipt() as receipt, self.assertRaisesRegex(fleet.UpgradeError, "timed_out"):
            self.upgrade(panel, receipt)
        self.assertFalse(panel.values[1]["agent_settings"]["auto_update"])
        self.assertEqual(json.loads(self.path.read_text())["members"]["1"]["state"], "failed")

    def test_keyboard_interrupt_restores_the_previous_setting(self):
        panel = Panel(runtime=True)
        panel.runtime_error = KeyboardInterrupt()
        with self.receipt() as receipt, self.assertRaises(KeyboardInterrupt):
            self.upgrade(panel, receipt)
        self.assertFalse(panel.values[1]["agent_settings"]["auto_update"])

    def test_restore_failure_is_persisted_for_recovery_and_is_non_success(self):
        panel = Panel()
        panel.upgrade = False
        panel.restore_error = True
        with self.receipt() as receipt, self.assertRaisesRegex(
            fleet.UpgradeError, "restore_failed"
        ):
            self.upgrade(panel, receipt)
        saved = json.loads(self.path.read_text())
        self.assertTrue(saved["members"]["1"]["restore_required"])
        panel.restore_error = False
        with self.receipt() as receipt:
            fleet.recover_settings(panel, receipt)
            self.assertFalse(receipt.value["members"]["1"]["restore_required"])
        self.assertFalse(panel.values[1]["agent_settings"]["auto_update"])

    def test_originally_enabled_setting_is_restored_to_true(self):
        panel = Panel()
        panel.values[1]["agent_settings"]["auto_update"] = True
        with self.receipt() as receipt:
            self.upgrade(panel, receipt)
        self.assertTrue(panel.values[1]["agent_settings"]["auto_update"])

    def test_healthy_equal_revisions_do_not_hide_the_wrong_runtime(self):
        panel = Panel(runtime=True)
        panel.values[1]["static_info"]["runtime_version"] = "1.14.4-apsp.1"
        with self.receipt() as receipt, self.assertRaisesRegex(fleet.UpgradeError, "timed_out"):
            self.upgrade(panel, receipt)

    def test_mismatched_embedded_singbox_component_version_is_rejected(self):
        panel = Panel(runtime=True)
        panel.upgraded_singbox_component = "1.14.4-apsp.1"
        with self.receipt() as receipt, self.assertRaisesRegex(fleet.UpgradeError, "timed_out"):
            self.upgrade(panel, receipt)

    def test_manifest_error_cannot_be_overridden_by_old_healthy_runtime(self):
        panel = Panel(runtime=True)
        panel.runtime_state = "failed"
        with self.receipt() as receipt, self.assertRaisesRegex(fleet.UpgradeError, "timed_out"):
            self.upgrade(panel, receipt)

    def test_old_checkpoint_is_not_new_acceptance(self):
        panel = Panel(runtime=True)
        panel.checkpoint_current = False
        with self.receipt() as receipt, self.assertRaisesRegex(fleet.UpgradeError, "timed_out"):
            self.upgrade(panel, receipt)

    def test_operator_clock_skew_ahead_of_panel_uses_server_request_created_at(self):
        panel = Panel(runtime=True)
        skewed_operator_now = panel.panel_time + 3600
        ok_proc = type("Result", (), {"returncode": 0})()
        with (
            self.receipt() as receipt,
            mock.patch.object(fleet.time, "time", return_value=skewed_operator_now),
            mock.patch.object(fleet.subprocess, "run", return_value=ok_proc),
        ):
            post_reply = panel.call(
                "/api/servers/1/runtime-control/checkpoint", "POST", {"module": "singbox"}
            )
            self.assertEqual(set(post_reply["request"]), {"request_id", "expected", "expires_at"})
            panel.calls.clear()
            panel.deployment_healthy = True
            panel.runtime_state = "ready"
            result = self.upgrade(panel, receipt)
        self.assertEqual(result["state"], "control_plane_verified")
        self.assertEqual(result["external_client_check"], "passed")
        self.assertFalse(panel.values[1]["agent_settings"]["auto_update"])

    def test_expired_and_displaced_checkpoints_retry_cleanly_within_budget(self):
        for transient in ("expired", "displaced"):
            with self.subTest(transient=transient):
                self.clock = Clock()
                panel = Panel(runtime=True)
                panel.checkpoint_outcomes = [transient]
                ok_proc = type("Result", (), {"returncode": 0})()
                with self.receipt() as receipt, mock.patch.object(
                    fleet.subprocess, "run", return_value=ok_proc
                ):
                    result = self.upgrade(panel, receipt)
                self.assertEqual(result["state"], "control_plane_verified")
                self.assertEqual(result["external_client_check"], "passed")
                self.assertEqual(panel.runtime_state, "ready")
                self.assertTrue(panel.deployment_healthy)
                posts = [
                    path
                    for path, method, _ in panel.calls
                    if method == "POST" and path.endswith("/runtime-control/checkpoint")
                ]
                self.assertEqual(len(posts), 2)
                self.assertEqual(panel.checkpoint_seq, 2)

    def test_checkpoint_retry_window_never_relaxes_blocked_or_failed_runtime_states(self):
        for bad_state in ("sync_blocked", "failed", "upgrade_required"):
            with self.subTest(bad_state=bad_state):
                self.clock = Clock()
                panel = Panel(runtime=True)

                def expire_then_block(orig_call=panel.call):
                    def wrapped(path, method="GET", payload=None, timeout=45):
                        res = orig_call(path, method, payload, timeout)
                        if path.endswith("/runtime-control") and panel.checkpoint_seq == 1:
                            panel.runtime_state = bad_state
                        return res

                    return wrapped

                panel.checkpoint_outcomes = ["expired"]
                panel.call = expire_then_block()
                with (
                    self.receipt() as receipt,
                    mock.patch.object(fleet.subprocess, "run") as client_runner,
                    self.assertRaisesRegex(fleet.UpgradeError, "timed_out"),
                ):
                    self.upgrade(panel, receipt)
                client_runner.assert_not_called()
                self.assertEqual(panel.checkpoint_seq, 1)

    def test_rejected_failed_or_mismatched_checkpoint_aborts_immediately(self):
        for outcome in ("failed", "rejected", "mismatch", "wrong_binding"):
            with self.subTest(outcome=outcome):
                self.clock = Clock()
                panel = Panel(runtime=True)
                panel.checkpoint_outcomes = [outcome]
                with (
                    self.receipt() as receipt,
                    mock.patch.object(fleet.subprocess, "run") as client_runner,
                    self.assertRaisesRegex(fleet.UpgradeError, "runtime_checkpoint_rejected"),
                ):
                    self.upgrade(panel, receipt)
                client_runner.assert_not_called()
                self.assertEqual(self.clock.now(), 0)
                self.assertFalse(panel.values[1]["agent_settings"]["auto_update"])

    def test_failed_external_client_stops_the_member(self):
        panel = Panel(runtime=True)
        with (
            self.receipt() as receipt,
            mock.patch.object(
                fleet.subprocess, "run", return_value=type("Result", (), {"returncode": 1})()
            ),
            self.assertRaisesRegex(fleet.UpgradeError, "external_client_check_failed"),
        ):
            self.upgrade(panel, receipt, client_check="/TEST_ONLY/client-check")
        self.assertFalse(panel.values[1]["agent_settings"]["auto_update"])

    def test_missing_client_check_prevents_runtime_upgrade_before_mutation(self):
        panel = Panel(runtime=True)
        with self.receipt() as receipt, contextlib.redirect_stdout(io.StringIO()) as output:
            status = fleet.execute(panel, [1], VERSION, receipt, 3, 1)
        self.assertEqual(status, 1)
        summary = json.loads(output.getvalue().splitlines()[-1])
        self.assertFalse(summary["control_plane_complete"])
        self.assertFalse(summary["complete"])
        self.assertIn("external_client_check_required", summary["members"][0]["error"])
        self.assertFalse(any(method == "PATCH" for _, method, _ in panel.calls))

    def test_missing_client_check_also_prevents_release_import(self):
        panel = Panel(runtime=True)
        with self.assertRaisesRegex(fleet.UpgradeError, "external_client_check_required"):
            fleet.prepare_apply(panel, [1], VERSION, ["linux-gnu-amd64"], None)
        self.assertFalse(
            any(path == "/api/system/agent-releases/import" for path, _, _ in panel.calls)
        )

    def test_import_is_scoped_to_the_explicit_signed_version_and_targets(self):
        panel = Panel()
        fleet.prepare_apply(panel, [1], VERSION, ["linux-gnu-amd64", "linux-gnu-arm64"], None)
        imports = [
            payload
            for path, _, payload in panel.calls
            if path == "/api/system/agent-releases/import"
        ]
        self.assertEqual(
            imports,
            [{"tag": "agent-v0.4.0", "targets": ["linux-gnu-amd64", "linux-gnu-arm64"]}],
        )

    def test_explicit_targets_run_sequentially_and_failure_marks_rest_not_started(self):
        panel = Panel((1, 2, 3))
        panel.values[2]["online"] = False
        with self.receipt((1, 2, 3)) as receipt, contextlib.redirect_stdout(
            io.StringIO()
        ) as output:
            status = fleet.execute(panel, [1, 2, 3], VERSION, receipt, 3, 1)
        self.assertEqual(status, 1)
        summary = json.loads(output.getvalue().splitlines()[-1])
        self.assertFalse(summary["complete"])
        self.assertEqual(
            [item["state"] for item in summary["members"]],
            ["control_plane_verified", "failed", "not_started"],
        )
        self.assertFalse(panel.enabled)
        self.assertFalse(any(path == "/api/servers/3" for path, _, _ in panel.calls))

    def test_private_credentials_reject_broad_permissions_and_symlinks(self):
        self.path.write_text(json.dumps({"password": "TEST_ONLY_PASSWORD"}))
        self.path.chmod(0o644)
        with self.assertRaisesRegex(fleet.UpgradeError, "mode_0600"):
            fleet.private_json(self.path)
        self.path.chmod(0o600)
        link = self.path.with_name("link.json")
        link.symlink_to(self.path)
        with self.assertRaises(OSError):
            fleet.private_json(link)

    def test_remote_plain_http_and_credentialed_origins_are_rejected(self):
        for value in (
            "http://panel.example.invalid",
            "https://secret@panel.example.invalid",
            "https://panel.example.invalid/api",
        ):
            with self.subTest(value=value), self.assertRaises(fleet.UpgradeError):
                fleet.origin(value)


if __name__ == "__main__":
    unittest.main()
