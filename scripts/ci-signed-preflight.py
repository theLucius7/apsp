#!/usr/bin/env python3
"""Verify rejected cache replacements preserve the running disposable installation."""

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import tempfile
import uuid

SPEC = importlib.util.spec_from_file_location("ci_bootstrap_install", Path(__file__).with_name("ci-bootstrap-install.py"))
BOOTSTRAP = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BOOTSTRAP)
AGENT = Path("/usr/local/bin/apsp-agent")
RUNTIME = Path("/opt/apsp/plugins/sing-box/current/sing-box")
DATABASE = Path("/var/lib/apsp/core/state.db")


def ensure(condition, message):
    if not condition:
        raise ValueError(message)


def digest(path):
    with Path(path).open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def snapshot():
    files = [Path("/etc/apsp/agent.toml"), Path("/etc/systemd/system/apsp-agent.service"),
             Path("/etc/systemd/system/apsp-singbox@.service")]
    identity = Path("/etc/apsp/identity")
    ensure(identity.is_dir() and not identity.is_symlink(), "identity directory is not ordinary")
    files.extend(sorted(identity.iterdir()))
    ensure(all(path.is_file() and not path.is_symlink() for path in files), "preserved installation files must be ordinary")
    links = [Path("/opt/apsp/core/current"), Path("/opt/apsp/plugins/sing-box/current"),
             Path("/var/lib/apsp/plugins/sing-box@main/current")]
    units = ("apsp-agent.service", "apsp-singbox@main.service")
    processes = {}
    for unit in units:
        process = subprocess.run(["systemctl", "show", unit, "-p", "MainPID", "--value"],
                                 check=True, capture_output=True, text=True, timeout=15).stdout.strip()
        ensure(process.isdigit() and int(process) > 0, "running service has no PID")
        subprocess.run(["systemctl", "is-active", "--quiet", unit], check=True, timeout=15)
        processes[unit] = int(process)
    with sqlite3.connect(f"file:{DATABASE}?mode=ro", uri=True, timeout=10) as connection:
        connection.execute("BEGIN")
        ledger = connection.execute("SELECT module,stat_name,epoch,uplink,downlink FROM usage_baselines ORDER BY module,stat_name").fetchall()
        pending = connection.execute("SELECT epoch,seq,batch FROM usage_outbox WHERE acknowledged=0 ORDER BY seq").fetchall()
        sequence = connection.execute("SELECT value FROM kv WHERE key='usage:last_seq'").fetchall()
        applied = connection.execute("SELECT key,value FROM kv WHERE key LIKE 'applied:%' ORDER BY key").fetchall()
        intents = connection.execute("SELECT op_id,module,payload FROM intents WHERE completed=0 ORDER BY op_id").fetchall()
    ensure(not pending and not intents, "negative checks require a stable acknowledged ledger")
    return {"files": {str(path): digest(path) for path in files},
            "links": {str(path): os.readlink(path) for path in links},
            "processes": processes, "ledger": ledger, "sequence": sequence, "applied": applied}


def atomic_replace(path, contents, mode):
    path = Path(path)
    descriptor, stage = tempfile.mkstemp(prefix=".ci-proof-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "wb") as output:
            output.write(contents)
            output.flush()
            os.fsync(output.fileno())
        os.chmod(stage, mode)
        os.replace(stage, path)
    finally:
        if os.path.exists(stage):
            os.unlink(stage)


def probe(binary, directory, must_pass):
    marker = Path(directory) / ("systemd-marker-" + uuid.uuid4().hex)
    unit = "apsp-ci-proof-" + uuid.uuid4().hex
    result = subprocess.run(["systemd-run", "--quiet", "--wait", "--collect", "--unit", unit,
                             "--property=Type=oneshot", f"--property=ExecStartPre={AGENT.resolve()} verify-installed --binary {binary} --name sing-box --format tar.gz",
                             "--", "/usr/bin/touch", str(marker)], check=False,
                            capture_output=True, timeout=60)
    ensure((result.returncode == 0) == must_pass and marker.exists() == must_pass,
           "real systemd signature gate did not match the expected execution result")
    marker.unlink(missing_ok=True)


def running_runtime_executable():
    process = subprocess.run(["systemctl", "show", "apsp-singbox@main.service", "-p", "MainPID", "--value"],
                             check=True, capture_output=True, text=True, timeout=15).stdout.strip()
    ensure(process.isdigit() and int(process) > 0, "runtime service has no PID")
    return os.readlink(f"/proc/{process}/exe")


def exercise(args):
    binary = RUNTIME.resolve(strict=True)
    directory = binary.parent
    paths = [binary, *(directory / name for name in ("release.json", "SHA256SUMS", "SHA256SUMS.minisig", ".artifact.json"))]
    original = {path: (path.read_bytes(), path.stat().st_mode & 0o777) if path.exists() else None for path in paths}
    executable = running_runtime_executable()
    ensure(not executable.endswith(" (deleted)"), "runtime must run from its installed executable")
    # Move originals aside and back by rename instead of rewriting their bytes.
    # Replacing the path under the running runtime leaves its executable
    # deleted, which the Agent's exact inspection rightly refuses; a rename
    # moves the same entry, so the process follows it back. Keep the originals
    # outside the artifact tree, on the same filesystem.
    aside = Path(tempfile.mkdtemp(prefix=".apsp-ci-originals-", dir="/opt"))
    moved = {}

    def move_aside(path):
        moved[path] = aside / path.name
        os.rename(path, moved[path])

    def restore():
        for path, saved in original.items():
            if path in moved:
                os.replace(moved.pop(path), path)
            elif saved is None:
                path.unlink(missing_ok=True)
    metadata = json.loads(original[directory / "release.json"][0])
    entry = next(item for item in metadata["artifacts"] if item["name"] == "sing-box" and item["version"] == directory.name and item["arch"] == "amd64")
    rows = dict((path, checksum) for checksum, path in (line.split("  ") for line in original[directory / "SHA256SUMS"][0].decode().splitlines()))
    archive_hash = rows["/".join(entry[key] for key in ("name", "version", "arch"))]
    baseline = snapshot()
    results = []
    try:
        for case in ("cached-binary", "cached-proof", "legacy-unsigned-cache"):
            if case == "cached-binary":
                contents, mode = original[binary]
                move_aside(binary)
                atomic_replace(binary, bytes([contents[0] ^ 1]) + contents[1:], mode)
            elif case == "cached-proof":
                proof = directory / "SHA256SUMS"
                contents, mode = original[proof]
                move_aside(proof)
                atomic_replace(proof, (b"0" if contents[:1] != b"0" else b"1") + contents[1:], mode)
            else:
                for name in ("release.json", "SHA256SUMS", "SHA256SUMS.minisig"):
                    move_aside(directory / name)
                if original[directory / ".artifact.json"] is not None:
                    move_aside(directory / ".artifact.json")
                atomic_replace(directory / ".artifact.json", json.dumps({"archive_sha256":archive_hash,
                    "binary_sha256":digest(binary)}).encode(), 0o644)
            result = subprocess.run([str(AGENT), "verify-cache"], check=False, capture_output=True, timeout=60)
            ensure(result.returncode != 0, "untrusted cache passed Agent preflight")
            probe(binary, args.log_directory, False)
            result = BOOTSTRAP.bootstrap(args.enrollment, args.trust_directory, args.bundle,
                                         args.log_directory / (case + ".log"))
            ensure(result != 0, "untrusted cache passed installer migration preflight")
            ensure(snapshot() == baseline, "rejected installation changed identity, configuration, ledger, links, or process IDs")
            restore()
            ensure(running_runtime_executable() == executable,
                   "restored cache must keep the running runtime on its original executable")
            ensure(all(path.read_bytes() == saved[0] for path, saved in original.items() if saved is not None),
                   "restored cache differs from the original files")
            subprocess.run([str(AGENT), "verify-cache"], check=True, capture_output=True, timeout=60)
            probe(binary, args.log_directory, True)
            ensure(snapshot() == baseline, "restored proof changed the running installation")
            results.append(case)
            print("Signed installation rejection and unchanged running state verified:", case)
    finally:
        restore()
        aside.rmdir()
    ensure(running_runtime_executable() == executable, "runtime executable changed during cache rejection checks")
    args.summary.write_text(json.dumps({"passed":True,"checks":results + ["restored-systemd-verifier"]}) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for argument in ("enrollment", "trust-directory", "bundle", "log-directory", "summary"):
        parser.add_argument("--" + argument, type=Path, required=True)
    args = parser.parse_args()
    ensure(os.getuid() == 0 and os.environ.get("APSP_E2E_DISPOSABLE_HOST") == "1", "cache mutation requires an explicitly disposable root-run CI host")
    args.log_directory.mkdir(mode=0o700)
    exercise(args)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError, json.JSONDecodeError, subprocess.SubprocessError):
        raise SystemExit("Signed CI preflight checks failed; private logs retained until cleanup") from None
