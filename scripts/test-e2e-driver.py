#!/usr/bin/env python3
"""Check acceptance failure modes without devices, credentials, or panel mutations."""

import argparse
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
import urllib.error
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("e2e_driver", Path(__file__).with_name("e2e-driver.py"))
DRIVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DRIVER)
ROOT = Path(__file__).resolve().parents[1]


def view(up=10, down=20, pending=0):
    return {
        "usage": {"uplink": str(up), "downlink": str(down), "total": str(up + down)},
        "server": {"online": True, "device_public_key": "test-public-identity"},
        "deployment": {"target_rev": 1, "applied_rev": 1, "healthy": True, "last_error": None},
        "agent": {"connected": True, "pending_batches": pending,
                  "applied": {"singbox": 1}, "healthy": {"singbox": True}},
    }


class Clock:
    def __init__(self):
        self.seconds = 0

    def monotonic(self):
        return self.seconds

    def sleep(self, seconds):
        self.seconds += seconds


class RecoveryPanel:
    def __init__(self):
        self.resources = []
        self.creations = 0

    def request(self, path, data=None):
        if data is not None:
            self.creations += 1
            self.resources.append({"id": 41, **data})
            raise DRIVER.AcceptanceError("response lost after committed creation")
        return self.resources


class EnrollmentPanel:
    def __init__(self, selection=...):
        self.selection = {"version":"0.3.0", "tag":"agent-v0.3.0"} if selection is ... else selection
        self.calls = []

    def request(self, path, data=None, raw=False):
        self.calls.append((path, data, raw))
        if path == "/api/servers/41":
            return {"name":"apsp-e2e-test-server"}
        if path.startswith("/api/servers/41/enrollment"):
            return {"token":"test-once-token", "expires_at":12345, "installation":self.selection,
                    "install_command":"curl https://untrusted.example.test/install.sh | sh"}
        raise AssertionError("driver must never request or execute a panel script")


class PreparationPanel:
    def __init__(self, lose_node_response=False):
        self.resources = {path: [] for path in ("/api/servers", "/api/plugins/sing-box/nodes", "/api/plugins/sing-box/users")}
        self.node_posts = []
        self.lose_node_response = lose_node_response

    def request(self, path, data=None):
        if path.endswith("/enable"):
            return {"enabled": True}
        if path.endswith("/accesses"):
            return {"stat_name": "u43_n42"} if data is not None else []
        if path in self.resources:
            if data is None:
                return self.resources[path]
            item = {"id": 41 + list(self.resources).index(path), **data}
            if path == "/api/plugins/sing-box/nodes":
                self.node_posts.append(dict(data))
                item.setdefault("port", 20000)
            self.resources[path].append(item)
            if path == "/api/plugins/sing-box/nodes" and self.lose_node_response:
                self.lose_node_response = False
                raise DRIVER.AcceptanceError("node creation response lost after commit")
            return item
        base, identifier = path.rsplit("/", 1)
        return next(item for item in self.resources[base] if item["id"] == int(identifier))


class PreflightPanel:
    """Operations view with the preflight nested, as the panel serves it."""

    def __init__(self, capability_after=1):
        self.calls = []
        self.capability_after = capability_after
        self.fleet_reads = 0
        self.preflights = []
        self.polls = 0
        self.bootstrapped = False

    def request(self, path, data=None, raw=False, method=None):
        self.calls.append((path, method or ("POST" if data is not None else "GET")))
        base = "/api/plugins/sing-box/servers/41/operations-view"
        if path == "/api/servers/41/fleet" and method == "PUT":
            assert data["policy"]["runtime_inspection"] is True and data["expected_digest"] == "digest-1"
            return {}
        if path == "/api/servers/41/fleet":
            self.fleet_reads += 1
            capabilities = [DRIVER.RUNTIME_PREFLIGHT_CAPABILITY] if self.fleet_reads > self.capability_after else []
            return {"asset": {}, "policy": {"runtime_inspection": False}, "digest": "digest-1",
                    "capabilities": capabilities}
        if path == "/api/control-center/reauth":
            return {"verified_at": 1, "expires_at": 301}
        if path == base + "/preflight":
            self.preflights.append(f"preflight-{len(self.preflights) + 1}")
            self.polls = 0
            return {"id": self.preflights[-1], "status": "queued"}
        if path == base + "/preflight/bootstrap":
            assert data == {"id": "preflight-1", "confirm": True}
            self.bootstrapped = True
            return {}
        if path == base + "/preflight/confirm":
            assert data == {"id": "preflight-2", "confirm": True}
            return {"id": data["id"], "confirmed": True}
        if path == base:
            self.polls += 1
            first = len(self.preflights) == 1
            current = {"id": self.preflights[-1], "device_checks_pending": self.polls < 2,
                       "ready": not first, "checks": [{"key": "ports", "blocking": False}],
                       "bootstrap": {"available": first, "ready": first}}
            observed = self.bootstrapped and self.polls > 1
            checkpoint = {"state": "stale", "outcome": "superseded", "observed_at": 120 if observed else None}
            return {"server_id": 41, "id": None, "preflight": current,
                    "drift": {"state": "needs_preflight", "target_revision": 1 if self.bootstrapped else None,
                              "checkpoint": checkpoint}}
        raise AssertionError(path)


class AcceptanceContracts(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.state_path = Path(self.directory.name) / "state.json"
        self.state = {"schema": 1, "prefix": "apsp-e2e-test", "checkpoints": {"before": view()}}
        self.args = argparse.Namespace(state=self.state_path, label="after", unchanged_from="before",
                                      interval=35, timeout=75, status_command="apsp-agent status")

    def verify(self, samples):
        clock = Clock()
        with patch.object(DRIVER, "snapshot", side_effect=samples), \
             patch.object(DRIVER.time, "monotonic", clock.monotonic), \
             patch.object(DRIVER.time, "sleep", clock.sleep), \
             contextlib.redirect_stdout(io.StringIO()):
            DRIVER.verify(None, self.state, self.args)
        return clock

    def test_totp_is_opt_in_and_rejects_malformed_codes_before_login(self):
        with patch.dict(DRIVER.os.environ, {}, clear=True), patch.object(DRIVER.getpass, "getpass") as prompt:
            self.assertIsNone(DRIVER.totp_code())
            prompt.assert_not_called()
        with patch.dict(DRIVER.os.environ, {"APSP_E2E_TOTP_CODE": "not-a-code"}, clear=True):
            with self.assertRaises(DRIVER.AcceptanceError):
                DRIVER.totp_code()
        with patch.dict(DRIVER.os.environ, {}, clear=True), patch.object(DRIVER.getpass, "getpass", return_value="012345"):
            code = DRIVER.totp_code(True)
            with patch.object(DRIVER.Panel, "request", return_value={}) as request:
                DRIVER.Panel("https://example.test", "test-password", code)
                request.assert_called_once_with("/api/login", {"password": "test-password", "totp_code": "012345"})

    def run_summary(self, systemd_queries, owned=True):
        source = (ROOT / "scripts/ci-real-e2e.sh").read_text().split("write_summary() {\n", 1)[1]
        source = source.split("<<'PY'\n", 1)[1].split("\nPY\n}", 1)[0]
        output = self.state_path.with_name("public-summary.json")
        report = self.state_path.with_name("public-summary.md")
        argv = ["summary", str(self.state_path), str(output), "0", "install", "1", "1" if owned else "0", "278"]
        annotation = io.StringIO()
        with contextlib.chdir(ROOT), patch.object(DRIVER.sys, "argv", argv), \
             patch.dict(DRIVER.os.environ, {"GITHUB_STEP_SUMMARY": str(report), "GITHUB_ACTIONS": "true"}, clear=True), \
             contextlib.redirect_stdout(annotation), \
             patch.object(DRIVER.subprocess, "run", side_effect=systemd_queries) as run:
            exec(compile(source, "ci-real-e2e.sh:write_summary", "exec"), {})
        summary = json.loads(output.read_text())
        self.assertEqual(summary["failure_line"], 278)
        self.assertEqual(annotation.getvalue(), "::error title=Reality acceptance failed::"
                         + json.dumps(summary, ensure_ascii=True) + "\n")
        return summary, report.read_text(), run

    def test_ready_timeout_preserves_private_last_snapshot_but_public_outputs_are_allowlisted(self):
        secret = "TOKEN-identity-key@private-host.example.test/198.51.100.77"
        first, last = view(), view()
        first["server"]["online"] = False
        last["server"].update(device_public_key=secret, agent_version=secret, hostname=secret)
        last["deployment"].update(target_rev=2, healthy=False, last_error=secret)
        last["agent"]["token"] = secret
        args = argparse.Namespace(state=self.state_path, timeout=2, agent_version="0.3.0", client_port=None)
        clock = Clock()
        with patch.object(DRIVER, "snapshot", side_effect=[first, last]), \
             patch.object(DRIVER.time, "monotonic", clock.monotonic), \
             patch.object(DRIVER.time, "sleep", clock.sleep):
            with self.assertRaisesRegex(DRIVER.AcceptanceError, "超时") as error:
                DRIVER.ready(None, self.state, args)
        private = self.state_path.with_name("ready-timeout.json")
        self.assertEqual(private.stat().st_mode & 0o777, 0o600)
        self.assertEqual(json.loads(private.read_text())["snapshot"], last)
        self.assertIn(secret, private.read_text())
        self.assertNotIn(secret, str(error.exception))
        expected = {"online": True, "deployment_present": True, "target_rev": 2, "applied_rev": 1,
                    "target_positive": True, "revisions_match": False, "healthy": False,
                    "has_last_error": True, "version_required": True, "version_matches": False}
        self.assertEqual(json.loads(str(error.exception).split("状态=", 1)[1]), expected)
        queries = [subprocess.CompletedProcess([], 0,
                   "ActiveState=failed\nSubState=failed\nResult=exit-code\nExecMainStatus=1\nDescription=" + secret,
                   secret), subprocess.CompletedProcess([], 0,
                   "ActiveState=" + secret + "\nSubState=" + secret + "\nResult=" + secret
                   + "\nExecMainStatus=" + secret, secret)]
        summary, report, run = self.run_summary(queries)
        self.assertEqual(summary["readiness"], expected)
        self.assertEqual(summary["systemd"], {
            "apsp-agent.service": {"query_succeeded": True, "ActiveState": "failed", "SubState": "failed",
                                    "Result": "exit-code", "ExecMainStatus": 1},
            "apsp-singbox@main.service": {"query_succeeded": True}})
        self.assertNotIn(secret, json.dumps(summary) + report)
        self.assertNotIn("device_public_key", json.dumps(summary))
        self.assertEqual([call.args[0][2] for call in run.call_args_list],
                         ["apsp-agent.service", "apsp-singbox@main.service"])

    def test_readiness_counters_are_bounded_integers_and_missing_values_remain_unknown(self):
        sample = view()
        for value in (-1, 2 ** 63, True, False, "1", 1.0, "private-host.example.test"):
            with self.subTest(value=value):
                sample["deployment"].update(target_rev=value, applied_rev=value, healthy=value)
                sample["server"]["online"] = value
                status = DRIVER.readiness_status(sample, None)
                self.assertIsNone(status["target_rev"])
                self.assertIsNone(status["applied_rev"])
                self.assertIsNone(status["target_positive"])
                self.assertIsNone(status["revisions_match"])
                if type(value) is not bool:
                    self.assertIsNone(status["online"])
                    self.assertIsNone(status["healthy"])
                self.assertFalse(status["version_required"])
                self.assertIsNone(status["version_matches"])
        for value in (0, 2 ** 63 - 1):
            sample["deployment"].update(target_rev=value, applied_rev=value)
            status = DRIVER.readiness_status(sample, "expected-version")
            self.assertEqual(status["target_rev"], value)
            self.assertTrue(status["revisions_match"])
            self.assertFalse(status["version_matches"])
        status = DRIVER.readiness_status({"deployment": None}, None)
        self.assertFalse(status["deployment_present"])
        self.assertIsNone(status["online"])
        self.assertIsNone(status["healthy"])
        self.assertIsNone(status["has_last_error"])

    def test_explicit_installation_failure_is_not_an_indefinite_wait_or_secret_leak(self):
        sample = view()
        secret = "TEST_ONLY-private-installation-detail"
        sample["installation"] = {"state": "failed", "reason": secret}
        args = argparse.Namespace(state=self.state_path, timeout=600, agent_version=None, client_port=None)
        with patch.object(DRIVER, "snapshot", return_value=sample), patch.object(DRIVER.time, "sleep") as sleep:
            with self.assertRaisesRegex(DRIVER.AcceptanceError, "安装已明确失败") as error:
                DRIVER.ready(None, self.state, args)
            sleep.assert_not_called()
        self.assertNotIn(secret, str(error.exception))
        private = self.state_path.with_name("ready-timeout.json")
        self.assertIn(secret, private.read_text())
        self.assertEqual(private.stat().st_mode & 0o777, 0o600)
        self.assertEqual(DRIVER.readiness_status(sample, None)["installation_state"], "failed")
        sample["installation"]["state"] = secret
        self.assertNotIn("installation_state", DRIVER.readiness_status(sample, None))
        sample["installation"]["state"] = "pending"
        self.assertFalse(DRIVER.healthy(sample))
        sample["installation"]["state"] = "ready"
        sample["publication_pending"] = True
        self.assertFalse(DRIVER.healthy(sample))

    def test_systemd_public_status_rejects_ambiguous_out_of_range_and_failed_queries(self):
        for value in ("-1", "256", "65536", "01", "1.0", "secret"):
            with self.subTest(value=value):
                self.assertNotIn("ExecMainStatus", DRIVER.systemd_status("ExecMainStatus=" + value, True))
        for value in ("0", "255"):
            self.assertEqual(DRIVER.systemd_status("ExecMainStatus=" + value, True)["ExecMainStatus"], int(value))
        self.assertEqual(DRIVER.systemd_status("ActiveState=secret\nActiveState=active\nExecMainStatus=0\nExecMainStatus=0", True),
                         {"query_succeeded": True})
        self.assertEqual(DRIVER.systemd_status("ActiveState=active\nExecMainStatus=0", False),
                         {"query_succeeded": False})
        summary, _, _ = self.run_summary([subprocess.CompletedProcess([], 1, "ActiveState=active", "secret"),
                                         subprocess.TimeoutExpired(["systemctl"], 5, output="secret")])
        self.assertEqual(summary["systemd"], {unit: {"query_succeeded": False}
                                            for unit in ("apsp-agent.service", "apsp-singbox@main.service")})
        summary, _, run = self.run_summary([], owned=False)
        self.assertNotIn("systemd", summary)
        run.assert_not_called()

    def test_ready_retry_removes_old_timeout_evidence_and_refuses_symlink(self):
        path = self.state_path.with_name("ready-timeout.json")
        DRIVER.save(path, {"snapshot": "old private failure"})
        args = argparse.Namespace(state=self.state_path, timeout=2, agent_version="0.3.0", client_port=None)
        with patch.object(DRIVER, "snapshot", side_effect=DRIVER.AcceptanceError("panel unavailable")):
            with self.assertRaisesRegex(DRIVER.AcceptanceError, "panel unavailable"):
                DRIVER.ready(None, self.state, args)
        self.assertFalse(path.exists())
        path.symlink_to(self.state_path.with_name("unrelated"))
        with patch.object(DRIVER, "snapshot") as snapshot:
            with self.assertRaisesRegex(DRIVER.AcceptanceError, "符号链接"):
                DRIVER.ready(None, self.state, args)
            snapshot.assert_not_called()

    def preparation_state(self, port=None):
        self.state.update(origin="http://127.0.0.1:8000", public_host="node.example.test",
                          sni="camouflage.example.test", requested_port=port)
        DRIVER.save(self.state_path, self.state)
        return self.state

    def test_explicit_privileged_port_survives_lost_node_creation_response(self):
        panel = PreparationPanel(lose_node_response=True)
        self.preparation_state(443)
        with patch.object(DRIVER, "install"), contextlib.redirect_stdout(io.StringIO()):
            with self.assertRaises(DRIVER.AcceptanceError):
                DRIVER.prepare(panel, self.state, self.args)
            recovered = json.loads(self.state_path.read_text())
            self.assertEqual(recovered["requested_port"], 443)
            DRIVER.prepare(panel, recovered, self.args)
        self.assertEqual(len(panel.node_posts), 1)
        self.assertEqual(panel.node_posts[0]["port"], 443)
        self.assertEqual(recovered["port"], 443)

    def test_automatic_port_is_not_submitted_and_changed_assignment_is_rejected(self):
        panel = PreparationPanel()
        self.preparation_state()
        with patch.object(DRIVER, "install"), contextlib.redirect_stdout(io.StringIO()):
            DRIVER.prepare(panel, self.state, self.args)
            self.assertNotIn("port", panel.node_posts[0])
            self.assertEqual(self.state["port"], 20000)
            panel.resources["/api/plugins/sing-box/nodes"][0]["port"] = 20001
            with self.assertRaisesRegex(DRIVER.AcceptanceError, "端口已变化"):
                DRIVER.prepare(panel, self.state, self.args)
        self.assertEqual(self.state["port"], 20000)

    def test_legacy_state_resumes_automatic_selection_but_cannot_change_requested_port(self):
        self.preparation_state()
        del self.state["requested_port"]
        DRIVER.save(self.state_path, self.state)
        arguments = ["driver", "--state", str(self.state_path), "prepare", "--origin", self.state["origin"],
                     "--public-host", self.state["public_host"], "--sni", self.state["sni"]]
        with patch.object(DRIVER.sys, "argv", arguments), patch.object(DRIVER, "Panel") as panel, \
             patch.object(DRIVER, "password", return_value="TEST_ONLY"), patch.object(DRIVER, "totp_code", return_value=None), \
             patch.object(DRIVER, "prepare"):
            DRIVER.main()
            panel.assert_called_once()
        with patch.object(DRIVER.sys, "argv", arguments + ["--port", "443"]), patch.object(DRIVER, "Panel") as panel:
            with self.assertRaisesRegex(DRIVER.AcceptanceError, "不同环境"):
                DRIVER.main()
            panel.assert_not_called()

    def test_invalid_node_ports_are_rejected_before_login_or_state_creation(self):
        arguments = ["driver", "--state", str(self.state_path), "prepare", "--origin", "http://127.0.0.1:8000",
                     "--public-host", "node.example.test", "--sni", "camouflage.example.test"]
        for port in (0, 18085, 65536, -1):
            with self.subTest(port=port), patch.object(DRIVER.sys, "argv", arguments + ["--port", str(port)]), \
                 patch.object(DRIVER, "Panel") as panel:
                with self.assertRaisesRegex(DRIVER.AcceptanceError, "节点端口"):
                    DRIVER.main()
                panel.assert_not_called()
                self.assertFalse(self.state_path.exists())

    def installation_state(self):
        self.state.update(server_id=41, origin="http://127.0.0.1:18080")
        return self.state

    def test_install_saves_private_descriptor_and_ignores_panel_shell_command(self):
        panel = EnrollmentPanel()
        state = self.installation_state()
        with contextlib.redirect_stdout(io.StringIO()) as output:
            DRIVER.install(panel, state, self.state_path)
        descriptor = self.state_path.with_name("enrollment.json")
        saved = json.loads(descriptor.read_text())
        self.assertEqual(saved["token"], "test-once-token")
        self.assertEqual(saved["tag"], "agent-v0.3.0")
        self.assertEqual(descriptor.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.state_path.stat().st_mode & 0o777, 0o600)
        self.assertNotIn("token", state["installation"])
        self.assertNotIn("test-once-token", output.getvalue())
        self.assertNotIn("install_command", saved)
        self.assertFalse(self.state_path.with_name("install.sh").exists())
        self.assertFalse(any("/install.sh" in call[0] for call in panel.calls))

    def test_install_reuses_saved_token_after_lost_state_write_without_another_request(self):
        panel = EnrollmentPanel()
        state = self.installation_state()
        with patch.object(DRIVER, "save", side_effect=OSError("state write failed")):
            with self.assertRaises(OSError):
                DRIVER.install(panel, state, self.state_path)
        recovered = {key:value for key,value in state.items() if key != "installation"}
        calls = list(panel.calls)
        with contextlib.redirect_stdout(io.StringIO()):
            DRIVER.install(panel, recovered, self.state_path)
        self.assertEqual(panel.calls, calls)
        self.assertEqual(recovered["installation"]["version"], "0.3.0")

    def test_install_accepts_portable_latest_and_keeps_explicit_upgrade_selection(self):
        panel = EnrollmentPanel({"version":"latest", "tag":None})
        state = self.installation_state()
        with contextlib.redirect_stdout(io.StringIO()):
            DRIVER.install(panel, state, self.state_path)
        saved = json.loads(self.state_path.with_name("enrollment.json").read_text())
        self.assertEqual(saved["version"], "latest")
        self.assertIsNone(saved["tag"])
        calls = list(panel.calls)
        with contextlib.redirect_stdout(io.StringIO()):
            DRIVER.install(panel, state, self.state_path)
        self.assertEqual(panel.calls, calls)
        with self.assertRaisesRegex(DRIVER.AcceptanceError, "指定的 Agent 版本"):
            DRIVER.install(panel, state, self.state_path, agent_version="0.2.0")
        with contextlib.redirect_stdout(io.StringIO()):
            DRIVER.install(EnrollmentPanel({"version":"0.2.0", "tag":"agent-v0.2.0"}),
                           state, self.state_path, refresh=True, agent_version="0.2.0")
        saved = json.loads(self.state_path.with_name("enrollment.json").read_text())
        self.assertEqual(saved["version"], "0.2.0")
        self.assertEqual(saved["tag"], "agent-v0.2.0")

    def test_latest_descriptor_rejects_fabricated_or_missing_tags(self):
        for selection in ({"version":"latest", "tag":"agent-vlatest"}, {"version":"latest"},
                          {"version":"0.3.0", "tag":None}):
            with self.subTest(selection=selection), self.assertRaises(DRIVER.AcceptanceError):
                DRIVER.install(EnrollmentPanel(selection), self.installation_state(), self.state_path)
            self.assertFalse(self.state_path.with_name("enrollment.json").exists())

    def test_install_refuses_missing_mismatched_or_other_requested_release(self):
        for selection in (None, {}, {"version":"0.3.0", "tag":"agent-v0.2.0"}, {"version":"../escape", "tag":"agent-v../escape"}):
            with self.subTest(selection=selection):
                with self.assertRaises(DRIVER.AcceptanceError):
                    DRIVER.install(EnrollmentPanel(selection), self.installation_state(), self.state_path)
                self.assertFalse(self.state_path.with_name("enrollment.json").exists())
        with self.assertRaises(DRIVER.AcceptanceError):
            DRIVER.install(EnrollmentPanel(), self.installation_state(), self.state_path, agent_version="0.2.0")

    def test_install_refuses_symlink_or_public_cached_descriptors(self):
        descriptor = self.state_path.with_name("enrollment.json")
        descriptor.symlink_to(self.state_path.with_name("missing"))
        panel = EnrollmentPanel()
        with self.assertRaises(DRIVER.AcceptanceError):
            DRIVER.install(panel, self.installation_state(), self.state_path, refresh=True)
        self.assertEqual(panel.calls, [])
        descriptor.unlink()
        with contextlib.redirect_stdout(io.StringIO()):
            DRIVER.install(panel, self.state, self.state_path)
        descriptor.chmod(0o644)
        with self.assertRaises(DRIVER.AcceptanceError):
            DRIVER.install(panel, self.state, self.state_path)

    def test_install_refresh_selects_explicit_version_without_shell_interpolation(self):
        panel = EnrollmentPanel()
        with contextlib.redirect_stdout(io.StringIO()):
            DRIVER.install(panel, self.installation_state(), self.state_path, refresh=True, agent_version="0.3.0")
        self.assertEqual(panel.calls[-1][0], "/api/servers/41/enrollment?agent_version=0.3.0")

    def test_lost_creation_response_recovers_without_duplicate_or_touching_other_resources(self):
        panel = RecoveryPanel()
        unrelated = {"id": 7, "name": "existing-server"}
        panel.resources.append(unrelated.copy())
        DRIVER.save(self.state_path, self.state)
        with self.assertRaises(DRIVER.AcceptanceError):
            DRIVER.resource(panel, "/api/servers", self.state, "server", {}, self.state_path)
        recovered = json.loads(self.state_path.read_text())
        result = DRIVER.resource(panel, "/api/servers", recovered, "server", {}, self.state_path)
        self.assertEqual(result["id"], 41)
        self.assertEqual(panel.creations, 1)
        self.assertEqual(panel.resources[0], unrelated)
        self.assertEqual(self.state_path.stat().st_mode & 0o777, 0o600)

    def test_replay_increase_is_a_failure_even_when_device_is_healthy(self):
        with self.assertRaisesRegex(DRIVER.AcceptanceError, "确认用量变化"):
            self.verify([view(up=20, down=40)])
        self.assertNotIn("after", self.state["checkpoints"])

    def test_identity_change_is_a_failure(self):
        changed = view()
        changed["server"]["device_public_key"] = "different-public-identity"
        with self.assertRaisesRegex(DRIVER.AcceptanceError, "身份发生变化"):
            self.verify([changed])

    def test_changed_configuration_cannot_pass_upgrade_continuity_with_unchanged_ledger(self):
        changed = view()
        changed["deployment"].update(target_rev=2, applied_rev=2)
        changed["agent"]["applied"]["singbox"] = 2
        with self.assertRaisesRegex(DRIVER.AcceptanceError, "配置版本发生变化"):
            self.verify([changed])

    def test_upgrade_requires_both_actual_version_reports_and_a_new_process(self):
        self.args.agent_version, self.args.agent_restarted = "0.3.1", True
        baseline = self.state["checkpoints"]["before"]
        baseline["agent"].update(pid=100, agent_version="0.3.0")
        baseline["server"].update(id=41, agent_version="0.3.0", runtime_version="1.14.2")
        upgraded = view()
        upgraded["agent"].update(pid=200, agent_version="0.3.1")
        upgraded["server"].update(id=41, agent_version="0.3.1", runtime_version="1.14.2")
        for field, mutation in (("agent", {"agent_version": "0.3.0"}), ("agent", {"pid": 100}),
                                ("server", {"id": 42}), ("server", {"runtime_version": "1.14.3"})):
            changed = json.loads(json.dumps(upgraded))
            changed[field].update(mutation)
            with self.subTest(mutation=mutation), self.assertRaises(DRIVER.AcceptanceError):
                self.verify([changed])
        self.verify([json.loads(json.dumps(upgraded)) for _ in range(3)])
        self.assertEqual(self.state["checkpoints"]["after"]["agent"]["pid"], 200)

    def test_pending_outbox_cannot_be_reported_as_a_stable_pass(self):
        with self.assertRaisesRegex(DRIVER.AcceptanceError, "outbox 未清空"):
            self.verify([view(pending=1), view(pending=1), view(pending=1)])
        self.assertNotIn("after", self.state["checkpoints"])

    def test_stable_pass_requires_two_complete_sampling_intervals(self):
        clock = self.verify([view(), view(), view()])
        self.assertEqual(clock.seconds, 70)
        self.assertEqual(len(self.state["checkpoints"]["after"]["stable_samples"]), 2)

    def test_only_downstream_growth_does_not_satisfy_bidirectional_acceptance(self):
        args = argparse.Namespace(state=self.state_path, label="first", after="before",
                                  min_uplink=1, min_downlink=1, timeout=10)
        clock = Clock()
        with patch.object(DRIVER, "snapshot", return_value=view(down=100)), \
             patch.object(DRIVER.time, "monotonic", clock.monotonic), \
             patch.object(DRIVER.time, "sleep", clock.sleep):
            with self.assertRaisesRegex(DRIVER.AcceptanceError, "双向代理用量"):
                DRIVER.traffic(None, self.state, args)


    def test_preflight_reads_the_nested_view_bootstraps_once_and_reauthenticates_before_confirming(self):
        panel = PreflightPanel()
        state = {"schema": 1, "prefix": "apsp-e2e-test", "server_id": 41, "checkpoints": {}}
        args = argparse.Namespace(state=self.state_path, timeout=600, totp=False)
        clock = Clock()
        # Saved business nodes keep the server dirty, so the empty revision is
        # never certified as applied; the answered checkpoint is the signal.
        pending = view()
        pending["deployment"] = {"target_rev": 1, "applied_rev": 0, "healthy": False}
        with patch.object(DRIVER, "snapshot", return_value=pending), \
             patch.object(DRIVER, "password", return_value="test-password"), \
             patch.object(DRIVER.time, "monotonic", clock.monotonic), \
             patch.object(DRIVER.time, "sleep", clock.sleep), \
             contextlib.redirect_stdout(io.StringIO()):
            DRIVER.preflight(panel, state, args)
        self.assertTrue(panel.bootstrapped)
        self.assertEqual(panel.preflights, ["preflight-1", "preflight-2"])
        paths = [path for path, _ in panel.calls]
        for action in ("/preflight/bootstrap", "/preflight/confirm"):
            index = next(i for i, path in enumerate(paths) if path.endswith(action))
            self.assertEqual(paths[index - 1], "/api/control-center/reauth")
        self.assertEqual(state["checkpoints"]["preflight"]["id"], "preflight-2")
        self.assertEqual(json.loads(self.state_path.read_text())["checkpoints"]["preflight"]["id"], "preflight-2")

    def test_preflight_timeout_reports_operation_states_without_values(self):
        panel = PreflightPanel(capability_after=0)
        panel.request = (lambda original: lambda path, data=None, raw=False, method=None: (
            {"preflight": {"id": "preflight-1", "device_checks_pending": True,
                           "operations": [{"status": "queued"}, {"status": "running"}]}}
            if path.endswith("/operations-view") else original(path, data, raw, method)))(panel.request)
        state = {"schema": 1, "prefix": "apsp-e2e-test", "server_id": 41, "checkpoints": {}}
        args = argparse.Namespace(state=self.state_path, timeout=10, totp=False)
        clock = Clock()
        with patch.object(DRIVER.time, "monotonic", clock.monotonic), \
             patch.object(DRIVER.time, "sleep", clock.sleep):
            with self.assertRaisesRegex(DRIVER.AcceptanceError, "匹配请求=True 操作状态=queued,running"):
                DRIVER.preflight(panel, state, args)


    def test_origin_header_matches_the_panel_canonical_public_origin(self):
        self.assertEqual(DRIVER.origin_header("http://127.0.0.1:18080"), "http://127.0.0.1:18080")
        self.assertEqual(DRIVER.origin_header("HTTPS://Panel.Example.Test:443"), "https://panel.example.test")
        self.assertEqual(DRIVER.origin_header("https://panel.example.test:8443"), "https://panel.example.test:8443")
        self.assertEqual(DRIVER.origin_header("http://[::1]:18080"), "http://[::1]:18080")


    def test_http_failures_name_token_free_routes_and_the_panel_message_only(self):
        def failure(body):
            return urllib.error.HTTPError("http://panel.test", 400, "Bad Request", {}, io.BytesIO(body))
        message = json.dumps({"error": "访问地址与面板配置不一致"}).encode()
        self.assertEqual(DRIVER.failure_detail("/api/control-center/reauth", failure(message)),
                         " /api/control-center/reauth：访问地址与面板配置不一致")
        self.assertEqual(DRIVER.failure_detail("/api/servers/41/fleet?x=1", failure(b"not json")), " /api/servers/{id}/fleet")
        for path in ("/sub/secret-token?format=singbox", "/api/servers/41/enrollment", "/api/tokens/7"):
            self.assertEqual(DRIVER.failure_detail(path, failure(message)), "")


    def test_stalled_install_reports_fixed_vocabulary_but_never_error_text(self):
        status = {"target_rev": 1, "applied_rev": 0, "healthy": False,
                  "last_error": "runtime health check timed out at /var/lib/apsp/plugins/TEST_ONLY-secret"}
        drift = {"state": "revision_mismatch", "checkpoint_supported": True,
                 "checkpoint": {"state": "pending", "outcome": None, "failure_kind": "free text not allowed",
                                "reason": "实际运行时健康检查未通过"}}
        summary = DRIVER.install_status(status, drift)
        self.assertEqual(summary, {"target_rev": 1, "applied_rev": 0, "healthy": False,
                                   "error_categories": ["health", "timeout"], "drift": "revision_mismatch",
                                   "checkpoint_supported": True, "checkpoint_state": "pending",
                                   "checkpoint_outcome": None, "checkpoint_failure_kind": None})
        self.assertNotIn("TEST_ONLY-secret", json.dumps(summary))
        self.assertEqual(DRIVER.install_status(None, None)["error_categories"], [])


    def test_bootstrap_install_needs_an_applied_revision_or_an_answered_checkpoint_for_it(self):
        applied = {"target_rev": 1, "applied_rev": 1, "healthy": True}
        waiting = {"target_rev": 1, "applied_rev": 0, "healthy": False}
        answered = {"target_revision": 1, "checkpoint": {"outcome": "superseded", "observed_at": 120}}
        self.assertTrue(DRIVER.bootstrap_installed(applied, {}))
        self.assertTrue(DRIVER.bootstrap_installed(waiting, answered))
        self.assertFalse(DRIVER.bootstrap_installed(waiting, {"target_revision": 1, "checkpoint": {"outcome": None, "observed_at": None}}))
        self.assertFalse(DRIVER.bootstrap_installed(waiting, {**answered, "target_revision": 2}))
        self.assertFalse(DRIVER.bootstrap_installed(waiting, {"target_revision": 1, "checkpoint": {"outcome": "failed", "observed_at": 120}}))
        self.assertFalse(DRIVER.bootstrap_installed({"target_rev": None}, answered))


    def test_preflight_collection_retries_only_a_fingerprint_change_during_sampling(self):
        clock = Clock()
        calls = []

        class Changing:
            def request(self, path, data=None, raw=False, method=None):
                calls.append(path)
                if path.endswith("/preflight") and len(calls) < 3:
                    raise DRIVER.AcceptanceError("面板请求失败，HTTP 409 /api/x：业务、授权、入口或制品在采集期间发生变化，请重新预检")
                if path.endswith("/preflight"):
                    return {"id": "preflight-1"}
                return {"preflight": {"id": "preflight-1", "device_checks_pending": False}}

        with patch.object(DRIVER.time, "monotonic", clock.monotonic), \
             patch.object(DRIVER.time, "sleep", clock.sleep):
            view = DRIVER.collect_preflight(Changing(), "/base", 600)
        self.assertEqual(view["id"], "preflight-1")
        self.assertEqual(calls.count("/base/preflight"), 3)

        class Refused:
            def request(self, path, data=None, raw=False, method=None):
                calls.append(path)
                raise DRIVER.AcceptanceError("面板请求失败，HTTP 409 /api/x：设备、能力、授权、制品或配置条件未满足")

        calls.clear()
        with patch.object(DRIVER.time, "monotonic", clock.monotonic), \
             patch.object(DRIVER.time, "sleep", clock.sleep):
            with self.assertRaisesRegex(DRIVER.AcceptanceError, "条件未满足"):
                DRIVER.collect_preflight(Refused(), "/base", 600)
        self.assertEqual(calls, ["/base/preflight"])

    def test_totals_preserve_large_integer_precision_and_reject_inconsistent_ledger(self):
        amount = 2 ** 64 + 17
        self.assertEqual(DRIVER.usage_totals(view(amount, amount)["usage"]), (amount, amount, 2 * amount))
        malformed = view()["usage"]
        malformed["total"] = "29"
        with self.assertRaisesRegex(DRIVER.AcceptanceError, "总量不一致"):
            DRIVER.usage_totals(malformed)


if __name__ == "__main__":
    unittest.main()
