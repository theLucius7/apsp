#!/usr/bin/env python3
"""Exercise the actual Agent's independent OpenRC one-shot services in a disposable VM."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import uuid


def main():
    assert os.environ.get("APSP_OPENRC_SMOKE") == "1", "requires a disposable OpenRC environment"
    binary = str(Path(sys.argv[1]).resolve())
    host_namespace = os.readlink("/proc/self/ns/mnt")
    with tempfile.TemporaryDirectory(prefix="apsp-openrc-jobs-") as directory:
        root = Path(directory)
        for program, args, seconds, succeeds in [
            ("/bin/sh", ["-c", f"readlink /proc/self/ns/mnt > {root}/namespace"], 10, True),
            ("/bin/false", [], 10, False),
            ("/bin/sh", ["-c", "sleep 30 & wait"], 1, False),
        ]:
            unit = f"apsp-diagnostic-{uuid.uuid4()}.service"
            job = dict(unit=unit, program=program, args=args, working_directory=directory, timeout_secs=seconds)
            spec = root / f"{unit}.json"
            spec.write_text(json.dumps(job))
            subprocess.run([binary, "service-job", "--spec", str(spec)], check=True, timeout=20)
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                result = subprocess.run([binary, "service-job", "--spec", str(spec), "--status"], capture_output=True, text=True, check=True, timeout=10)
                status = json.loads(result.stdout)
                if status != "running":
                    break
                time.sleep(0.2)
            assert (status == "succeeded") == succeeds, (job, status)
            if not succeeds:
                assert isinstance(status, dict) and "failed" in status, status
            if seconds == 1:
                assert "timeout" in status["failed"]["error"], status
            if succeeds:
                assert (root / "namespace").read_text().strip() != host_namespace
            repeated = subprocess.run([binary, "service-job", "--spec", str(spec)], capture_output=True, timeout=20)
            assert repeated.returncode != 0, "a completed job must not run again"
            subprocess.run(["rc-service", "--", unit.removesuffix(".service"), "stop"], capture_output=True, timeout=10)

        # Stopping the runner must also stop the independently grouped payload and
        # its descendants, releasing the shared exclusion lock for the next job.
        unit = f"apsp-diagnostic-{uuid.uuid4()}.service"
        service = unit.removesuffix(".service")
        parent_pid = root / "stop-parent.pid"
        child_pid = root / "stop-child.pid"
        escaped = root / "stop-child-escaped"
        release = root / "stop-child-release"
        child_script = root / "stop-child.sh"
        child_script.write_text(f"#!/bin/sh\necho $$ > '{child_pid}'\nwhile [ ! -e '{release}' ]; do sleep 0.05; done\ntouch '{escaped}'\nsleep 60\n")
        program_script = root / "stop-parent.sh"
        program_script.write_text(f"#!/bin/sh\necho $$ > '{parent_pid}'\n/bin/sh '{child_script}' &\nwait\n")
        spec = root / f"{unit}.json"
        spec.write_text(json.dumps(dict(unit=unit, program="/bin/sh", args=[str(program_script)], working_directory=directory, timeout_secs=120)))
        payload_group = None
        try:
            subprocess.run([binary, "service-job", "--spec", str(spec)], check=True, timeout=20)
            deadline = time.monotonic() + 10
            while not (parent_pid.exists() and child_pid.exists()):
                assert time.monotonic() < deadline, "diagnostic descendants did not start"
                time.sleep(0.05)
            payload_group = os.getpgid(int(parent_pid.read_text()))
            assert payload_group != os.getpgrp(), "diagnostic payload shares the smoke process group"
            subprocess.run(["rc-service", "--", service, "stop"], check=True, timeout=15)
            release.touch()
            subprocess.run(["flock", "--exclusive", "--nonblock", "/run/apsp-diagnostic/lock", "true"], check=True, timeout=5)
            for pid_file in [parent_pid, child_pid]:
                pid = int(pid_file.read_text())
                status_file = Path(f"/proc/{pid}/stat")
                deadline = time.monotonic() + 5
                while status_file.exists():
                    # Orphan zombies awaiting init reaping have stopped execution.
                    try:
                        state = status_file.read_text().split(")", 1)[1].split()[0]
                    except FileNotFoundError:
                        break
                    if state == "Z":
                        break
                    assert time.monotonic() < deadline, f"diagnostic descendant {pid} survived stop"
                    time.sleep(0.05)
            time.sleep(0.2)
            assert not escaped.exists(), "stopped diagnostic child executed its delayed payload"
            result = subprocess.run([binary, "service-job", "--spec", str(spec), "--status"], capture_output=True, text=True, check=True, timeout=10)
            status = json.loads(result.stdout)
            assert isinstance(status, dict) and "failed" in status, status
            repeated = subprocess.run([binary, "service-job", "--spec", str(spec)], capture_output=True, timeout=20)
            assert repeated.returncode != 0, "a stopped diagnostic must not run again"
        finally:
            subprocess.run(["rc-service", "--", service, "stop"], capture_output=True, timeout=15)
            if payload_group is not None:
                for pid_file, script in [(parent_pid, program_script), (child_pid, child_script)]:
                    try:
                        pid = int(pid_file.read_text())
                        command = Path(f"/proc/{pid}/cmdline").read_bytes()
                        if str(script).encode() in command and os.getpgid(pid) == payload_group:
                            os.killpg(payload_group, 9)
                            break
                    except (FileNotFoundError, ProcessLookupError):
                        pass
        print("Actual Agent OpenRC jobs: independent lifetime, mount isolation, success, failure, timeout, stop descendant cleanup, lock release, and replay rejection passed")


if __name__ == "__main__":
    main()
