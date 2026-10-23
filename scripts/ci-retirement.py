#!/usr/bin/env python3
"""Exercise online retirement only on this run's disposable Linux installation."""

import argparse
import contextlib
import importlib.util
import http.cookiejar
import json
import os
from pathlib import Path
import re
import sqlite3
import subprocess
import sys
import tempfile
import time
import tomllib
import urllib.error
import urllib.parse
import urllib.request

SPEC = importlib.util.spec_from_file_location("e2e_driver", Path(__file__).with_name("e2e-driver.py"))
DRIVER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(DRIVER)
ORIGIN = "http://127.0.0.1:18080"
AGENT = Path("/usr/local/bin/apsp-agent")
DATABASE = Path("/var/lib/apsp/core/state.db")
IDENTITY = Path("/etc/apsp/identity")
RUNTIME_DIRECTORY = Path("/var/lib/apsp/plugins/sing-box@main")
AGENT_UNIT = "apsp-agent.service"
RUNTIME_UNIT = "apsp-singbox@main.service"


class Failure(Exception):
    pass


class LocalPanel(DRIVER.Panel):
    def __init__(self, password):
        self.base = ORIGIN
        self.client = urllib.request.build_opener(
            urllib.request.ProxyHandler({}), DRIVER.NoRedirect(),
            urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()),
        )
        self.request("/api/login", {"password": password})


def require(condition, message):
    if not condition:
        raise Failure(message)


def command(arguments, timeout=20):
    return subprocess.run(arguments, check=False, capture_output=True, text=True, timeout=timeout)


def service(unit):
    properties = ("LoadState", "ActiveState", "SubState", "MainPID", "ControlPID", "ExecMainStatus", "Result",
                  "ExecMainStartTimestampMonotonic", "ConditionResult", "ConditionTimestampMonotonic")
    result = command(["systemctl", "show", unit, "--property=" + ",".join(properties)])
    require(result.returncode == 0 and len(result.stdout) <= 4096, "cannot inspect disposable systemd service")
    lines = result.stdout.splitlines()
    values = dict(line.split("=", 1) for line in lines if "=" in line)
    require(len(lines) == len(properties) and set(values) == set(properties), "systemd service status is incomplete")
    require(values["LoadState"] == "loaded", "disposable systemd service is not loaded")
    require(all(re.fullmatch(r"0|[1-9][0-9]*", values[key]) for key in
                ("MainPID", "ControlPID", "ExecMainStartTimestampMonotonic", "ConditionTimestampMonotonic"))
            and values["ConditionResult"] in ("yes", "no"), "systemd service status is invalid")
    return values


def runtime_restart_refused(before, after, returncode):
    require((after["ActiveState"], after["SubState"]) in (("inactive", "dead"), ("failed", "failed"))
            and after["MainPID"] == "0" and after["ControlPID"] == "0"
            and after["ExecMainStartTimestampMonotonic"] in ("0", before["ExecMainStartTimestampMonotonic"]),
            "retired runtime start was not rejected before runtime execution")
    # A failed ConditionPathExists makes systemctl start succeed without running
    # ExecStartPre/ExecStart. Require a fresh failed condition evaluation, not only
    # a zero exit code or a stale inactive service left by an earlier attempt.
    condition_skipped = (after["ActiveState"] == "inactive" and after["ConditionResult"] == "no"
                         and int(after["ConditionTimestampMonotonic"]) > int(before["ConditionTimestampMonotonic"]))
    require(returncode > 0 or (returncode == 0 and condition_skipped),
            "retired runtime start neither failed nor confirmed a condition skip")


def wait_retired_agent(after_start=None):
    deadline = time.monotonic() + 45
    while time.monotonic() < deadline:
        value = service(AGENT_UNIT)
        if (value["ActiveState"] == "inactive" and value["SubState"] == "dead"
                and value["MainPID"] == "0" and value["ExecMainStatus"] == "78"
                and value["Result"] == "success"
                and (after_start is None or value["ExecMainStartTimestampMonotonic"] != after_start)):
            return value
        time.sleep(0.2)
    raise Failure("retired Agent did not stop cleanly with exit status 78")


def ledger():
    require(DATABASE.is_file() and not DATABASE.is_symlink(), "retirement must preserve the ordinary ledger database")
    metadata = DATABASE.stat()
    with contextlib.closing(sqlite3.connect(f"file:{DATABASE}?mode=ro", uri=True, timeout=10)) as connection:
        connection.execute("BEGIN")
        baseline = connection.execute("SELECT module,stat_name,epoch,uplink,downlink FROM usage_baselines ORDER BY module,stat_name").fetchall()
        outbox = connection.execute("SELECT epoch,seq,batch,acknowledged FROM usage_outbox ORDER BY length(seq),seq").fetchall()
        sequence = connection.execute("SELECT value FROM kv WHERE key='usage:last_seq'").fetchone()
        clocks = connection.execute("SELECT key,value FROM kv WHERE key LIKE 'usage:module:%' ORDER BY key").fetchall()
        record = connection.execute("SELECT value FROM kv WHERE key='retirement'").fetchone()
        applications = connection.execute("SELECT COUNT(*) FROM kv WHERE key NOT LIKE 'usage:%' AND key<>'retirement'").fetchone()[0]
        intents = connection.execute("SELECT COUNT(*) FROM intents").fetchone()[0]
        connection.execute("ROLLBACK")
    require(all(row[3] in (0, 1) for row in outbox), "invalid persisted usage acknowledgement flag")
    return {"file": (metadata.st_dev, metadata.st_ino), "baseline": baseline, "outbox": outbox,
            "sequence": sequence, "clocks": {key: json.loads(value) for key, value in clocks},
            "retirement": json.loads(record[0]) if record else None,
            "applications": applications, "intents": intents}


def preserved_ledger(before, after):
    require(after["file"] == before["file"], "retirement replaced the ledger database")
    require(after["baseline"] == before["baseline"] and after["sequence"] == before["sequence"],
            "retirement changed acknowledged usage baselines or sequence")
    require(after["clocks"].keys() == before["clocks"].keys(), "retirement removed usage clocks")
    for key, previous in before["clocks"].items():
        current = after["clocks"][key]
        require(current["epoch"] == previous["epoch"] and current["timestamp"] >= previous["timestamp"],
                "retirement reset a usage epoch or moved its clock backwards")
    # The normal sampler may clean acknowledged rows before the request acquires its
    # operation gate. Retained rows must remain immutable; pending work may not vanish.
    rows = {(row[0], row[1]): row for row in before["outbox"]}
    require(all(row[3] == 1 for row in after["outbox"]), "retirement left unacknowledged usage")
    require(all(rows.get((row[0], row[1])) == row for row in after["outbox"]),
            "retirement changed an existing batch or introduced new traffic after the stable checkpoint")


def cleared_configuration(state):
    require(all(not os.path.lexists(IDENTITY / name) for name in ("device.key", "server_id", "panel_origin")),
            "retirement left enrollment credentials on disk")
    require(not os.path.lexists(RUNTIME_DIRECTORY), "retirement left managed runtime configuration on disk")
    current = ledger()
    record = current["retirement"]
    require(isinstance(record, dict) and record.get("phase") == "acknowledged"
            and record.get("server_id") == state["server_id"] and record.get("panel_origin") == ORIGIN,
            "retirement receipt was not durably acknowledged for this server")
    require(current["applications"] == 0 and current["intents"] == 0,
            "retirement retained configuration snapshots in the ledger")
    return current


def usage(panel, state, expected):
    query = urllib.parse.urlencode({"user_id": state["user_id"], "node_id": state["node_id"]})
    view = panel.request("/api/plugins/sing-box/usage?" + query)
    require(DRIVER.usage_totals(view) == expected, "retirement changed panel user/node usage totals")
    require(len(view["by_user"]) == 1 and view["by_user"][0]["user_id"] == state["user_id"]
            and len(view["by_node"]) == 1 and view["by_node"][0]["node_id"] == state["node_id"],
            "retirement removed the historical user/node accounting views")
    for group in (view["by_user"], view["by_node"]):
        require(tuple(int(group[0][key]) for key in ("uplink", "downlink")) == expected[:2],
                "historical accounting grouping differs from the stable checkpoint")
    return {key: str(number) for key, number in zip(("uplink", "downlink", "total"), expected)}


def http_status(panel, path, method):
    request = urllib.request.Request(panel.base + path, method=method)
    try:
        with panel.client.open(request, timeout=30) as response:
            return response.status
    except urllib.error.HTTPError as error:
        # Never print response bodies or URLs from private acceptance state.
        try:
            return error.code
        finally:
            error.close()


def preflight(args):
    require(sys.platform == "linux" and os.geteuid() == 0 and os.environ.get("APSP_E2E_DISPOSABLE_HOST") == "1",
            "retirement acceptance requires an explicitly disposable Linux host and root")
    require(os.environ.get("APSP_E2E_ADMIN_PASSWORD"), "CI administrator password must be supplied through the environment")
    require(args.state.is_file() and not args.state.is_symlink() and not args.state.parent.is_symlink(),
            "acceptance state must be an ordinary private file")
    require(args.state.stat().st_mode & 0o077 == 0 and args.state.parent.stat().st_mode & 0o077 == 0,
            "acceptance state and directory must be private")
    require(args.summary.parent.resolve() == args.state.parent.resolve()
            and args.summary.name == "retirement.json" and not os.path.lexists(args.summary),
            "retirement summary must be a new file in this acceptance directory")
    state = json.loads(args.state.read_text())
    require(state.get("schema") == 1 and state.get("origin") == ORIGIN
            and re.fullmatch(r"apsp-e2e-[0-9a-f]{32}", state.get("prefix", "")),
            "retirement is restricted to the isolated local CI panel")
    require(all(type(state.get(key)) is int and state[key] > 0 for key in ("server_id", "user_id", "node_id")),
            "acceptance resource identifiers are invalid")
    config = tomllib.loads(Path("/etc/apsp/agent.toml").read_text())
    require(config.get("panel_url") == ORIGIN and config.get("state_db") == str(DATABASE)
            and config.get("identity_dir") == str(IDENTITY)
            and config.get("runtime_root") == str(RUNTIME_DIRECTORY.parent),
            "installed Agent does not belong to the disposable CI environment")
    require(int((IDENTITY / "server_id").read_text()) == state["server_id"],
            "installed Agent identity differs from the acceptance server")
    expected = DRIVER.usage_totals(state["checkpoints"]["after-reinstall"]["usage"])
    require(expected[0] > 0 and expected[1] > 0, "retirement must follow real bidirectional traffic")
    return state, expected


def exercise(args):
    state, expected = preflight(args)
    panel = LocalPanel(os.environ["APSP_E2E_ADMIN_PASSWORD"])
    try:
        server_path = f"/api/servers/{state['server_id']}"
        server = panel.request(server_path)
        require(server["name"] == state["prefix"] + "-server" and server["online"] is True,
                "retirement target must be this run's online server")
        require("server:retire-v1" in server.get("capabilities", []), "online Agent does not advertise retirement support")
        version = state["installation"]["version"]
        version_output = command([str(AGENT), "--version"])
        require(version_output.returncode == 0 and version_output.stdout.strip() == "apsp-agent " + version
                and server["static_info"].get("agent_version") == version,
                "retirement target is not the currently installed CI Agent version")
        status_output = command([str(AGENT), "status"])
        require(status_output.returncode == 0 and len(status_output.stdout) <= 65536,
                "online Agent status is unavailable")
        status = json.loads(status_output.stdout)
        require(status.get("connected") is True and status.get("pending_batches") == 0
                and status.get("healthy", {}).get("singbox") is True,
                "retirement requires a connected healthy Agent with an acknowledged ledger")
        for unit in (AGENT_UNIT, RUNTIME_UNIT):
            value = service(unit)
            require(value["ActiveState"] == "active" and value["MainPID"].isdigit() and int(value["MainPID"]) > 0,
                    "retirement requires both real systemd services to be running")
        baseline = ledger()
        require(baseline["retirement"] is None and baseline["baseline"]
                and all(row[3] == 1 for row in baseline["outbox"]),
                "retirement requires a fresh request and a persisted acknowledged baseline")
        usage(panel, state, expected)
        # A second DELETE could take the offline fallback branch. This check never retries it.
        require(http_status(panel, server_path, "DELETE") == 204, "online retirement DELETE did not complete; no offline retry was attempted")
        require(http_status(panel, server_path, "GET") == 404, "retired server remains visible in the panel")
        agent = wait_retired_agent()
        runtime = service(RUNTIME_UNIT)
        require(runtime["ActiveState"] == "inactive" and runtime["SubState"] == "dead"
                and runtime["MainPID"] == "0" and runtime["ControlPID"] == "0",
                "retirement left the external runtime running")
        retired = cleared_configuration(state)
        preserved_ledger(baseline, retired)
        totals = usage(panel, state, expected)
        started = command(["systemctl", "start", AGENT_UNIT], timeout=30)
        require(started.returncode == 0, "explicit retired Agent start could not reach its terminal guard")
        wait_retired_agent(after_start=agent["ExecMainStartTimestampMonotonic"])
        resumed = cleared_configuration(state)
        preserved_ledger(retired, resumed)
        denied = command(["systemctl", "start", RUNTIME_UNIT], timeout=30)
        runtime_after = service(RUNTIME_UNIT)
        runtime_restart_refused(runtime, runtime_after, denied.returncode)
        cleared_configuration(state)
        usage(panel, state, expected)
        return {"passed": True, "online_delete_confirmed": True, "server_removed": True,
                "receipt_acknowledged": True, "credentials_removed": True, "configuration_removed": True,
                "ledger_preserved": True, "outbox_drained": True, "agent_stopped": True,
                "runtime_stopped": True, "agent_restart_refused": True, "runtime_restart_refused": True,
                "usage": totals}
    finally:
        panel.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--summary", type=Path, required=True)
    args = parser.parse_args()
    os.umask(0o077)
    summary = exercise(args)
    descriptor, temporary = tempfile.mkstemp(prefix=".retirement-", dir=args.summary.parent)
    try:
        with os.fdopen(descriptor, "w") as output:
            json.dump(summary, output, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, args.summary)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    print("Online retirement, credential cleanup, ledger preservation, and restart rejection verified.")


if __name__ == "__main__":
    try:
        main()
    except Failure as error:
        raise SystemExit("Retirement acceptance failed: " + str(error)) from None
    except (DRIVER.AcceptanceError, OSError, ValueError, KeyError, TypeError, sqlite3.Error, subprocess.SubprocessError):
        raise SystemExit("Retirement acceptance failed; private state and response bodies were not printed") from None
