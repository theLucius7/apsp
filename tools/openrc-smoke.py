#!/usr/bin/env python3
"""Verify the installer and real OpenRC supervision with disposable process fixtures."""

import hashlib
import http.server
import os
from pathlib import Path
import pwd
import signal
import socket
import struct
import json
import runpy
import tempfile
import io
import tarfile
import sqlite3
import subprocess
import threading
import time
import tomllib


ROOT = Path(__file__).resolve().parents[1]
RUNTIME_SERVICE = "apsp-singbox@main"
AGENT_STATE = Path("/var/lib/apsp/core/test-agent.pid")
RUNTIME_STATE = Path("/var/lib/apsp/plugins/sing-box@main/data/test-runtime.pid")
RELOAD_STATE = RUNTIME_STATE.with_suffix(".reloads")
TOKEN = "openrc-smoke-fixture-token"
RELEASE = runpy.run_path(str(ROOT / "tools/ci-release-fixture.py"))
BUNDLE = Path("/tmp/apsp-openrc-TEST_ONLY-release")
TRUST_DIRECTORY = Path("/etc/apsp-openrc-TEST_ONLY-trust")
TRUST_FILE = TRUST_DIRECTORY / "public-keys.json"
AGENT_VERSION = tomllib.loads((ROOT / "crates/agent/Cargo.toml").read_text())["package"]["version"]

AGENT = b'''#!/usr/bin/python3
from pathlib import Path
import os
import sys
import time
import hashlib
import importlib.util
import json

arguments = sys.argv[1:]
if arguments[:1] == ["--config"]:
    arguments = arguments[2:]
if arguments[0] == "verify-installed":
    # This is an explicitly public process fixture verifier, not the production Agent.
    binary = Path(arguments[arguments.index("--binary") + 1])
    name = arguments[arguments.index("--name") + 1]
    format = arguments[arguments.index("--format") + 1]
    proof = binary.parent
    module_spec = importlib.util.spec_from_file_location("test_release", "/src/tools/release.py")
    release = importlib.util.module_from_spec(module_spec)
    module_spec.loader.exec_module(release)
    roots = release.load_roots(Path("@TEST_ONLY_TRUST_FILE@"), require_protected=True)
    release.verify_signature(proof, roots, "minisign")
    data = binary.read_bytes()
    metadata_bytes = (proof / "release.json").read_bytes()
    rows = dict(line.split("  ")[::-1] for line in (proof / "SHA256SUMS").read_text().splitlines())
    assert rows["release.json"] == hashlib.sha256(metadata_bytes).hexdigest()
    metadata = json.loads(metadata_bytes)
    entry, = [item for item in metadata["artifacts"] if item["name"] == name]
    assert entry["format"] == format and entry["binary_name"] == binary.name
    assert len(data) == entry["binary_size"] and hashlib.sha256(data).hexdigest() == entry["binary_sha256"]
    print("TEST_ONLY process fixture signature and binary checked")
elif arguments[0] == "verify-cache":
    # Actual cache preflight is separately exercised by Rust and real-e2e checks.
    print("TEST_ONLY process fixture cache stub")
elif arguments[0] == "--version":
    print("apsp-agent @AGENT_VERSION@")
elif arguments[0] == "runtime-log":
    if os.environ.get("APSP_TEST_NO_MEMORY_LOG") == "1":
        raise SystemExit(2)
    binary = os.environ.get("APSP_TEST_AGENT") or @TEST_ONLY_REAL_AGENT@
    if binary:
        os.execv(binary, [binary, *arguments])
    if "--help" not in arguments:
        while sys.stdin.buffer.read(4096):
            pass
elif arguments[0] == "status":
    if os.environ.get("APSP_TEST_STATUS_FAIL") == "1":
        raise SystemExit("fixture startup check failed")
    print('{"fixture":true,"connected":true,"pending_batches":0}')
elif arguments[0] == "enroll":
    if os.environ.get("APSP_TEST_ENROLL_FAIL") == "1":
        raise SystemExit("fixture enrollment rejected")
    config = Path("/etc/apsp/agent.toml")
    if not config.exists():
        config.write_text('panel_url = "http://panel.example.com"\\n')
    print("fixture enrollment accepted", flush=True)
else:
    Path("/var/lib/apsp/core/test-agent.pid").write_text(str(os.getpid()))
    print("fixture agent started", flush=True)
    while True:
        time.sleep(0.1)
'''.replace(b"@AGENT_VERSION@", AGENT_VERSION.encode()).replace(
    b"@TEST_ONLY_TRUST_FILE@", str(TRUST_FILE).encode()).replace(
    b"@TEST_ONLY_REAL_AGENT@", repr(os.environ.get("APSP_TEST_AGENT", "")).encode())

RUNTIME = b'''#!/usr/bin/python3
from pathlib import Path
import os
import signal
import socket
import sys
import time

if sys.argv[1] == "version":
    print("sing-box version 1.14.2 (OpenRC process fixture)")
    raise SystemExit(0)
root = Path("/var/lib/apsp/plugins/sing-box@main/data")
listener = socket.socket()
listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
listener.bind(("127.0.0.1", 80))
listener.listen()
reloads = 0
def reload(signum, frame):
    global reloads
    reloads += 1
    (root / "test-runtime.reloads").write_text(str(reloads))
    print("fixture runtime reloaded", flush=True)
signal.signal(signal.SIGHUP, reload)
(root / "test-runtime.reloads").write_text("0")
(root / "test-runtime.pid").write_text(str(os.getpid()))
print("fixture runtime started", flush=True)
while True:
    time.sleep(0.1)
'''


def run(*args, **kwargs):
    result = subprocess.run(args, capture_output=True, text=True, timeout=90, **kwargs)
    if result.returncode:
        raise RuntimeError(f"{args!r}:\n{result.stdout}\n{result.stderr}")
    return result.stdout.strip()


def wait_for(check, description):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            value = check()
            if value:
                return value
        except (OSError, ValueError, RuntimeError):
            pass
        time.sleep(0.1)
    raise AssertionError(f"Timed out waiting for {description}")


def pid(path):
    number = int(path.read_text())
    return number if Path(f"/proc/{number}").exists() else None


def service(name, action):
    return run("rc-service", "--", name, action)


def memory_logs():
    """Read the actual collector IPC, including its nonprivileged peer identity."""
    lines, processes = [], []
    account = pwd.getpwnam("apsp-singbox")
    directory = Path(f"/run/apsp/runtime-logs/{RUNTIME_SERVICE}.service")
    assert directory.stat().st_mode & 0o777 == 0o700
    assert directory.stat().st_uid == account.pw_uid
    for stream in ("stdout", "stderr"):
        path = directory / f"{stream}.sock"
        assert path.stat().st_mode & 0o777 == 0o600
        with socket.socket(socket.AF_UNIX) as peer:
            peer.settimeout(5)
            peer.connect(str(path))
            process, uid, _ = struct.unpack("3i", peer.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            assert uid == account.pw_uid and uid != 0
            limits = Path(f"/proc/{process}/limits").read_text().splitlines()
            core, = [line for line in limits if line.startswith("Max core file size")]
            assert core.split()[4:6] == ["0", "0"]
            processes.append(process)
            peer.sendall(b'{"since":0,"follow":false}\n')
            with peer.makefile("rb") as reader:
                assert reader.readline(64) == b"APSP_RUNTIME_LOG_V1\n"
                while wire := reader.readline(100_000):
                    assert wire.endswith(b"\n") and len(wire) < 100_000
                    lines.append(json.loads(wire)["text"])
    assert not Path("/var/log/apsp/runtime.log").exists()
    assert {entry.name for entry in directory.iterdir()} == {"stdout.sock", "stderr.sock"}
    return lines, processes


def expect_memory_log(message, processes=None):
    if not os.environ.get("APSP_TEST_AGENT"):
        return None
    def ready():
        result = memory_logs()
        return result if message in result[0] else None
    lines, observed = wait_for(ready, "memory log replay")
    assert message in lines
    if processes is not None:
        assert observed == processes, "Agent changes must preserve the independent collectors"
    return observed


def provision_test_roots(directory=TRUST_DIRECTORY):
    # The read-only /src bind mount belongs to the host runner, not container root.
    # Copy only the public fixture into a fresh container-owned protected directory;
    # never change the host checkout's ownership or relax bootstrap's trust checks.
    directory.mkdir(mode=0o755)
    target = directory / "public-keys.json"
    with target.open("xb") as output:
        output.write((ROOT / "crates/protocol/tests/fixtures/public-keys.json").read_bytes())
    target.chmod(0o644)
    return target


def check_runtime_access(runtime):
    account = pwd.getpwnam("apsp-singbox")
    for path, mode, group in [
        ("/opt/apsp", 0o755, 0),
        ("/opt/apsp/core", 0o755, 0),
        ("/opt/apsp/plugins", 0o755, 0),
        ("/opt/apsp/plugins/sing-box", 0o755, 0),
        ("/var/lib/apsp", 0o755, 0),
        ("/var/lib/apsp/plugins", 0o2750, account.pw_gid),
        ("/etc/apsp", 0o700, 0),
        ("/etc/apsp/identity", 0o700, 0),
        ("/var/lib/apsp/core", 0o700, 0),
    ]:
        properties = Path(path).lstat()
        observed = properties.st_mode & 0o7777
        print(f"Managed path: {path} mode={observed:04o} uid={properties.st_uid} gid={properties.st_gid}", flush=True)
        assert not Path(path).is_symlink()
        assert (observed, properties.st_uid, properties.st_gid) == (mode, 0, group)
    unprivileged = ("setpriv", f"--reuid={account.pw_uid}", f"--regid={account.pw_gid}", "--clear-groups", "--")
    assert run(*unprivileged, str(runtime), "version").startswith("sing-box version 1.14.2")
    run(*unprivileged, "/usr/bin/python3", "-c", """
import os
assert os.geteuid() != 0
assert os.access('/var/lib/apsp/plugins/sing-box@main/current/config.json', os.R_OK)
assert os.access('/var/lib/apsp/plugins/sing-box@main/data', os.W_OK | os.X_OK)
assert not os.access('/etc/apsp/identity', os.X_OK)
assert not os.access('/var/lib/apsp/core', os.X_OK)
""")
    print("Runtime account can execute the artifact and access its state; identity and ledger remain private", flush=True)


def install_script(origin):
    # Exercise the static signed installer after an independently trusted bootstrap.
    subprocess.run(["python3", str(ROOT / "tools/release.py"), "render-installer",
                    "--template", str(ROOT / "deploy/install.sh.tmpl"),
                    "--agent-unit", str(ROOT / "deploy/apsp-agent.service"),
                    "--runtime-unit", str(ROOT / "plugins/sing-box/apsp-singbox@.service"),
                    "--output", str(BUNDLE / "install.sh")], check=True)
    proof = RELEASE["proof"]("agent", AGENT_VERSION, "apsp-agent", AGENT, arch=RELEASE["architecture"]())
    lines = dict(line.split("  ")[::-1] for line in proof["checksums"].splitlines())
    lines["install.sh"] = hashlib.sha256((BUNDLE / "install.sh").read_bytes()).hexdigest()
    proof["checksums"] = "".join(f"{lines[path]}  {path}\n" for path in sorted(lines))
    proof["signature"] = RELEASE["sign"](proof["checksums"].encode())
    RELEASE["install"](BUNDLE, proof)
    entry = json.loads(proof["metadata_json"])["artifacts"][0]
    (BUNDLE / entry["asset_name"]).write_bytes(AGENT)
    installer = TRUST_DIRECTORY / "trusted-install.sh"
    installer.write_bytes((BUNDLE / "install.sh").read_bytes())
    installer.chmod(0o600)
    return ["python3", str(ROOT / "tools/bootstrap.py"), "--tag", f"agent-v{AGENT_VERSION}",
            "--panel", origin, "--trusted-keys", str(TRUST_FILE),
            "--release-dir", str(BUNDLE), "--trusted-installer", str(installer)]


def main():
    uid_map = Path("/proc/self/uid_map").read_text().split()
    isolated = Path("/.dockerenv").is_file() or uid_map[:2] != ["0", "0"]
    if os.geteuid() != 0 or os.environ.get("APSP_OPENRC_SMOKE") != "1" or not isolated:
        raise SystemExit("Run this test in its disposable container or isolated root filesystem.")

    provision_test_roots()
    runpy.run_path(str(ROOT / "tools/release.py"))["load_roots"](TRUST_FILE, require_protected=True)
    BUNDLE.mkdir(mode=0o755)
    Path("/run/openrc").mkdir(parents=True, exist_ok=True)
    Path("/var/lib").mkdir(mode=0o755, exist_ok=True)
    Path("/run/openrc/softlevel").write_text("default\n")
    with Path("/etc/rc.conf").open("a") as config:
        config.write('\nrc_sys="docker"\nrc_cgroup_mode="none"\nrc_logger="NO"\n')
    net = Path("/etc/init.d/net")
    net.write_text("#!/sbin/openrc-run\nstart() { return 0; }\nstop() { return 0; }\n")
    net.chmod(0o755)
    service("net", "start")
    dependencies = Path("/run/openrc/deptree")
    # Keep the old cache newer than new services to reproduce missed invalidation
    # deterministically, including on slower CI runners.
    cache_time = time.time() + 60
    os.utime(dependencies, (cache_time, cache_time))

    class Download(http.server.BaseHTTPRequestHandler):
        def do_GET(self):
            raise AssertionError("Agent binary must never be downloaded from panel")

        def log_message(self, format, *args):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Download)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    script = install_script(f"http://127.0.0.1:{server.server_port}")
    environment = {**os.environ, "NO_PROXY": "127.0.0.1", "no_proxy": "127.0.0.1", "APSP_ENROLLMENT_TOKEN": TOKEN}
    try:
        run(*script, env=environment)
        agent_pid = wait_for(lambda: pid(AGENT_STATE), "Agent startup after installation")
        # BusyBox must not leave shared intermediate parents at the private umask.
        for shared in ("/opt/apsp", "/opt/apsp/plugins", "/var/lib/apsp"):
            properties = Path(shared).stat()
            assert properties.st_uid == 0 and properties.st_gid == 0
            assert properties.st_mode & 0o777 == 0o755, shared
        assert Path("/var/lib/apsp/core").stat().st_mode & 0o777 == 0o700
        run("su", "-s", "/bin/sh", "apsp-singbox", "-c",
            "test -x /opt/apsp && test -x /opt/apsp/plugins && test -x /var/lib/apsp")
        for name in ["apsp-agent", RUNTIME_SERVICE]:
            assert f"_service='{name}'" in dependencies.read_text(), name
            assert Path(f"/etc/runlevels/default/{name}").is_symlink()
        assert not Path("/etc/systemd/system/apsp-agent.service").exists()

        runtime = Path("/opt/apsp/plugins/sing-box/current/sing-box")
        runtime.parent.mkdir(parents=True)
        runtime.parent.chmod(0o755)
        runtime.write_bytes(RUNTIME)
        runtime.chmod(0o755)
        packed = io.BytesIO()
        with tarfile.open(fileobj=packed, mode="w:gz") as archive:
            member = tarfile.TarInfo("sing-box")
            member.size = len(RUNTIME)
            member.mode = 0o755
            archive.addfile(member, io.BytesIO(RUNTIME))
        RELEASE["install"](runtime.parent, RELEASE["proof"]("sing-box", "1.14.2", "sing-box", packed.getvalue(), "tar.gz", arch=RELEASE["architecture"]()))
        current = Path("/var/lib/apsp/plugins/sing-box@main/current")
        current.mkdir(mode=0o755)
        current.chmod(0o755)
        configuration = current / "config.json"
        configuration.write_text("{}\n")
        configuration.chmod(0o644)
        check_runtime_access(runtime)
        service(RUNTIME_SERVICE, "restart")
        runtime_pid = wait_for(lambda: pid(RUNTIME_STATE), "runtime startup")
        status = Path(f"/proc/{runtime_pid}/status").read_text().splitlines()
        fields = dict(line.split(":", 1) for line in status if ":" in line)
        assert fields["Uid"].split()[0] != "0", fields["Uid"]
        assert int(fields["CapEff"].strip(), 16) == 1 << 10, fields["CapEff"]
        assert int(fields["CapAmb"].strip(), 16) == 1 << 10, fields["CapAmb"]
        assert fields["NoNewPrivs"].strip() == "1"
        assert int(Path(f"/run/{RUNTIME_SERVICE}.pid").read_text()) != runtime_pid
        collectors = expect_memory_log("fixture runtime started")

        identity = Path("/etc/apsp/identity/fixture-existing-key")
        identity.write_text("fixture identity must survive installation\n")
        ledger = Path("/var/lib/apsp/core/fixture-existing-ledger")
        ledger.write_text("fixture persisted accounting\n")
        run(*script, env=environment)
        agent_pid = wait_for(lambda: (number := pid(AGENT_STATE)) != agent_pid and number,
                             "Agent restart after repeated installation")
        assert pid(RUNTIME_STATE) == runtime_pid
        assert identity.read_text() == "fixture identity must survive installation\n"
        assert ledger.read_text() == "fixture persisted accounting\n"
        expect_memory_log("fixture runtime started", collectors)

        # A current bootstrap can still install an older independently verified
        # Agent. Its service must never call a command that binary does not have.
        unit = Path(f"/etc/init.d/{RUNTIME_SERVICE}")
        run(*script, env={**environment, "APSP_TEST_NO_MEMORY_LOG": "1"})
        assert unit.read_bytes() == (ROOT / "plugins/sing-box/apsp-singbox-legacy.openrc").read_bytes()
        assert pid(RUNTIME_STATE) == runtime_pid
        run(*script, env=environment)
        assert unit.read_bytes() == (ROOT / "plugins/sing-box/apsp-singbox.openrc").read_bytes()
        assert pid(RUNTIME_STATE) == runtime_pid
        agent_pid = wait_for(lambda: pid(AGENT_STATE), "Agent after compatibility installation")
        expect_memory_log("fixture runtime started", collectors)

        failed = subprocess.run(script, text=True, capture_output=True, timeout=90,
                                env={**environment, "APSP_TEST_ENROLL_FAIL": "1"})
        assert failed.returncode != 0
        assert pid(AGENT_STATE) == agent_pid
        assert pid(RUNTIME_STATE) == runtime_pid
        assert Path("/opt/apsp/core/current/apsp-agent").read_bytes() == AGENT

        service(RUNTIME_SERVICE, "reload")
        wait_for(lambda: int(RELOAD_STATE.read_text()) == 1, "HUP reaching the runtime child")
        assert pid(RUNTIME_STATE) == runtime_pid
        expect_memory_log("fixture runtime reloaded", collectors)
        service("apsp-agent", "stop")
        assert pid(RUNTIME_STATE) == runtime_pid
        expect_memory_log("fixture runtime reloaded", collectors)
        service(RUNTIME_SERVICE, "status")
        previous = Path("/opt/apsp/core/current").readlink()
        before_recovery = pid(AGENT_STATE)
        with unit.open("a") as output:
            output.write("\n# fixture: preserve the previous service on rollback\n")
        previous_unit = unit.read_bytes()
        failed = subprocess.run(
            script, capture_output=True, text=True, timeout=90,
            env={**environment, "APSP_TEST_STATUS_FAIL": "1"},
        )
        assert failed.returncode != 0 and "恢复上一版本" in failed.stderr
        assert Path("/opt/apsp/core/current").readlink() == previous
        assert unit.read_bytes() == previous_unit
        wait_for(lambda: pid(AGENT_STATE) != before_recovery, "Agent recovery after failed startup")
        assert pid(RUNTIME_STATE) == runtime_pid
        expect_memory_log("fixture runtime reloaded", collectors)
        service("apsp-agent", "stop")
        service("apsp-agent", "start")
        agent_pid = wait_for(lambda: (number := pid(AGENT_STATE)) != agent_pid and number,
                             "Agent starting independently")

        os.kill(runtime_pid, signal.SIGKILL)
        runtime_pid = wait_for(lambda: (number := pid(RUNTIME_STATE)) != runtime_pid and number,
                               "runtime automatic recovery")
        os.kill(agent_pid, signal.SIGKILL)
        wait_for(lambda: (number := pid(AGENT_STATE)) != agent_pid and number,
                 "Agent automatic recovery")
        assert pid(RUNTIME_STATE) == runtime_pid
        service("apsp-agent", "status")
        service(RUNTIME_SERVICE, "status")
        assert "fixture agent started" in Path("/var/log/apsp/agent.log").read_text()
        assert not Path("/var/log/apsp/runtime.log").exists()
        expect_memory_log("fixture runtime started")

        with sqlite3.connect("/var/lib/apsp/core/state.db") as database:
            database.executescript('''
                CREATE TABLE kv (key TEXT, value TEXT);
                CREATE TABLE usage_outbox (epoch TEXT, seq TEXT, batch TEXT, acknowledged INTEGER);
                CREATE TABLE usage_baselines (module TEXT, stat_name TEXT, epoch TEXT,
                                             uplink TEXT, downlink TEXT, observed_at INTEGER);
                CREATE TABLE intents (completed INTEGER);
            ''')
        run("bash", "scripts/e2e-real.sh", "snapshot", "openrc-fixture", "/tmp/evidence")
        snapshot = next(Path("/tmp/evidence").iterdir())
        assert (snapshot / "init-system.txt").read_text() == "openrc\n"
        assert f"ChildPIDs={runtime_pid}" in (snapshot / "runtime-service.txt").read_text()
        assert f"SupervisorPID={Path(f'/run/{RUNTIME_SERVICE}.pid').read_text().strip()}" in (snapshot / "runtime-service.txt").read_text()

        service("apsp-agent", "stop")
        service(RUNTIME_SERVICE, "stop")
        agent_pid = int(AGENT_STATE.read_text())
        runtime_pid = int(RUNTIME_STATE.read_text())
        print(run("openrc", "default"), flush=True)
        wait_for(lambda: (number := pid(AGENT_STATE)) != agent_pid and number,
                 "Agent starting from the default runlevel")
        wait_for(lambda: (number := pid(RUNTIME_STATE)) != runtime_pid and number,
                 "runtime starting from its last configuration")

        if os.environ.get("APSP_TEST_AGENT"):
            print(run("python3", "tools/openrc-job-smoke.py", os.environ["APSP_TEST_AGENT"]), flush=True)

        systemctl = Path("/usr/local/bin/systemctl")
        systemctl.write_text('#!/bin/sh\nprintf "%s\\n" "$*" >> /tmp/systemctl-calls\n')
        systemctl.chmod(0o755)
        Path("/run/systemd/system").mkdir(parents=True)
        Path("/etc/systemd/system").mkdir(parents=True, exist_ok=True)
        try:
            run(*script, env=environment)
            assert Path("/tmp/systemctl-calls").read_text().splitlines() == [
                "daemon-reload", "enable apsp-singbox@main.service apsp-agent.service",
                "restart apsp-agent.service",
            ]
            assert Path("/etc/systemd/system/apsp-agent.service").read_text() == (ROOT / "deploy/apsp-agent.service").read_text()
            assert Path("/etc/systemd/system/apsp-singbox@.service").read_text() == (ROOT / "plugins/sing-box/apsp-singbox@.service").read_text()
        finally:
            systemctl.unlink()
            Path("/etc/systemd/system/apsp-agent.service").unlink(missing_ok=True)
            Path("/etc/systemd/system/apsp-singbox@.service").unlink(missing_ok=True)
        print("OpenRC installer, repeated upgrade, failed enrollment, isolated runtime, HUP, "
              "unprivileged capabilities, logs, automatic recovery, default runlevel, "
              "read-only snapshot, and systemd installer command contract: passed")
    except Exception:
        directory = Path(f"/run/apsp/runtime-logs/{RUNTIME_SERVICE}.service")
        print("Memory socket paths:", [str(path) for path in directory.glob("*")], flush=True)
        result = subprocess.run(["ps", "-o", "pid,ppid,user,comm"], capture_output=True, text=True, timeout=5)
        print(result.stdout, flush=True)
        for command in [("rc-status", "--all"), ("rc-update", "show")]:
            result = subprocess.run(command, capture_output=True, text=True, timeout=30)
            print(f"{command}:\n{result.stdout}\n{result.stderr}", flush=True)
        for name in ["agent", "runtime"]:
            log = Path(f"/var/log/apsp/{name}.log")
            if log.exists():
                print(f"{log}:\n{log.read_text()}", flush=True)
        raise
    finally:
        for name in ["apsp-agent", RUNTIME_SERVICE, "net"]:
            subprocess.run(["rc-service", "--", name, "stop"], capture_output=True, timeout=30)
        server.shutdown()
        server.server_close()


if __name__ == "__main__":
    main()
