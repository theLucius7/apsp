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


def exercise(args):
    binary = RUNTIME.resolve(strict=True)
    directory = binary.parent
    paths = [binary, *(directory / name for name in ("release.json", "SHA256SUMS", "SHA256SUMS.minisig", ".artifact.json"))]
    original = {path: (path.read_bytes(), path.stat().st_mode & 0o777) if path.exists() else None for path in paths}
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
                changed = bytes([contents[0] ^ 1]) + contents[1:]
                atomic_replace(binary, changed, mode)
            elif case == "cached-proof":
                proof = directory / "SHA256SUMS"
                contents, mode = original[proof]
                atomic_replace(proof, (b"0" if contents[:1] != b"0" else b"1") + contents[1:], mode)
            else:
                for name in ("release.json", "SHA256SUMS", "SHA256SUMS.minisig"):
                    (directory / name).unlink()
                atomic_replace(directory / ".artifact.json", json.dumps({"archive_sha256":archive_hash,
                    "binary_sha256":digest(binary)}).encode(), 0o644)
            result = subprocess.run([str(AGENT), "verify-cache"], check=False, capture_output=True, timeout=60)
            ensure(result.returncode != 0, "untrusted cache passed Agent preflight")
            probe(binary, args.log_directory, False)
            result = BOOTSTRAP.bootstrap(args.enrollment, args.trust_directory, args.bundle,
                                         args.log_directory / (case + ".log"))
            ensure(result != 0, "untrusted cache passed installer migration preflight")
            ensure(snapshot() == baseline, "rejected installation changed identity, configuration, ledger, links, or process IDs")
            for path, saved in original.items():
                if saved is None:
                    path.unlink(missing_ok=True)
                else:
                    atomic_replace(path, *saved)
            subprocess.run([str(AGENT), "verify-cache"], check=True, capture_output=True, timeout=60)
            probe(binary, args.log_directory, True)
            ensure(snapshot() == baseline, "restored proof changed the running installation")
            results.append(case)
            print("Signed installation rejection and unchanged running state verified:", case)
    finally:
        for path, saved in original.items():
            if saved is None:
                path.unlink(missing_ok=True)
            else:
                atomic_replace(path, *saved)
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
