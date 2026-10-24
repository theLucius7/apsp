#!/usr/bin/env python3
"""Exercise the actual Agent's diagnostic job boundary on OpenRC in a disposable VM.

Diagnostics require systemd with cgroup v2 CPU ceilings and the swap system
call filter. OpenRC cannot prove those protections, so the Agent must refuse
every diagnostic job before creating a service, running the payload or taking
the shared diagnostic lock, and it must keep refusing on replay.
"""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import uuid

REFUSAL = "诊断启动需要 systemd/cgroup v2 的 CPU 硬上限与 swap 系统调用保护"


def main():
    assert os.environ.get("APSP_OPENRC_SMOKE") == "1", "requires a disposable OpenRC environment"
    assert Path("/run/openrc/softlevel").is_file(), "OpenRC must be the active service manager"
    binary = str(Path(sys.argv[1]).resolve())
    with tempfile.TemporaryDirectory(prefix="apsp-openrc-jobs-") as directory:
        root = Path(directory)
        marker = root / "payload-ran"
        for attempt in range(2):
            unit = f"apsp-diagnostic-{uuid.uuid4()}.service"
            service = unit.removesuffix(".service")
            spec = root / f"{unit}.json"
            spec.write_text(json.dumps(dict(unit=unit, program="/bin/sh", args=["-c", f"touch '{marker}'"],
                                            working_directory=directory, timeout_secs=10)))
            result = subprocess.run([binary, "service-job", "--spec", str(spec)],
                                    capture_output=True, text=True, timeout=20)
            assert result.returncode != 0, f"attempt {attempt}: OpenRC diagnostic must be refused"
            assert REFUSAL in result.stderr, result.stderr
            assert not marker.exists(), "refused diagnostic payload executed"
            assert not Path("/etc/init.d", service).exists(), "refused diagnostic created an OpenRC service"
            status = subprocess.run(["rc-service", "--", service, "status"], capture_output=True, timeout=10)
            assert status.returncode != 0, "refused diagnostic left a runnable OpenRC service"
        lock = Path("/run/apsp-diagnostic/lock")
        if lock.exists():
            subprocess.run(["flock", "--exclusive", "--nonblock", str(lock), "true"], check=True, timeout=5)
    print("Actual Agent OpenRC diagnostics: refused without systemd protections, no service, payload or lock, refusal repeated")


if __name__ == "__main__":
    main()
