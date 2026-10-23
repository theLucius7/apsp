#!/usr/bin/env python3
"""Collect bounded, private diagnostic evidence; never start a workload."""
import argparse
import ipaddress
import json
import os
from pathlib import Path
import re
import shlex
import stat
import subprocess
import time
import urllib.parse
import urllib.request

LIMIT = 512 * 1024
COMMANDS = {
    "os": "cat /etc/os-release",
    "cgtop": "systemd-cgtop --batch --iterations=1 --depth=4",
    "oom": "journalctl -k --no-pager --since '10 minutes ago' --grep='[Oo][Oo][Mm]|[Oo]ut of memory|Killed process'",
    "disk": "df -h",
    "memory": "cat /proc/meminfo; cat /proc/loadavg",
    "services": "systemctl show apsp-agent.service apsp-singbox@main.service --property=ActiveState,SubState,MainPID,NRestarts,OOMScoreAdjust,CPUWeight",
    "diagnostics": "systemctl list-units 'apsp-diagnostic-*' --all --no-pager",
    "mounts": "findmnt --list --output TARGET,SOURCE,FSTYPE",
}


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        raise ValueError("panel redirects are not followed")


def private_file(path, value):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as output:
        output.write(value)


def panel_times(value):
    sampled_at = value.get("metrics_sampled_at")
    # API timestamps have different units. Missing legacy times remain unknown.
    return {
        "last_device_message_at": value.get("last_seen"),
        "last_heartbeat_at": value.get("last_heartbeat_at"),
        "metrics_sampled_at_ms": sampled_at,
        "metrics_timestamp_available": isinstance(sampled_at, int) and sampled_at > 0,
    }


def read_cookie(path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_mode & 0o077 or info.st_size > 4096:
        raise ValueError("cookie file must be private, regular and bounded")
    value = path.read_text().strip()
    if not value or any(ord(c) < 32 or ord(c) == 127 for c in value):
        raise ValueError("invalid cookie file")
    return value


def read_panel(origin, identifier, cookie):
    parsed = urllib.parse.urlsplit(origin)
    if any(ord(character) < 32 or ord(character) == 127 for character in origin) or parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.password or parsed.path not in ("", "/") or parsed.query or parsed.fragment or (parsed.port is not None and not 0 < parsed.port <= 65535):
        raise ValueError("invalid panel origin")
    if parsed.scheme == "http":
        try:
            address = ipaddress.ip_address(parsed.hostname)
            address = getattr(address, "ipv4_mapped", None) or address
            loopback = address.is_loopback
        except ValueError:
            loopback = parsed.hostname == "localhost"
        if not loopback:
            raise ValueError("HTTP panel origins must be loopback; use HTTPS")
    request = urllib.request.Request(origin.rstrip("/") + f"/api/servers/{identifier}", headers={"Cookie": cookie})
    client = urllib.request.build_opener(NoRedirect())
    with client.open(request, timeout=10) as response:
        body = response.read(LIMIT + 1)
    if len(body) > LIMIT:
        raise ValueError("panel response too large")
    return panel_times(json.loads(body))


def remote(host, command, directory):
    command = shlex.join(["bash", "-o", "pipefail", "-c", f"({command}) 2>&1 | head -c {LIMIT + 1}"])
    result = subprocess.run(["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=5", "--", host, command], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=12)
    if len(result.stdout) > LIMIT:
        raise ValueError("node output too large")
    private_file(directory, result.stdout.decode(errors="replace"))
    return result.returncode


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", required=True, help="SSH config alias for a dedicated node")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--samples", type=int, default=1)
    parser.add_argument("--interval", type=int, default=10)
    parser.add_argument("--container", help="Optional disposable Debian test container")
    parser.add_argument("--panel-origin")
    parser.add_argument("--server-id", type=int)
    parser.add_argument("--cookie-file", type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", args.host) or args.host.startswith("-"):
        parser.error("use a plain SSH config alias")
    if args.container and not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*", args.container):
        parser.error("invalid container name")
    if not 1 <= args.samples <= 180 or not 1 <= args.interval <= 60:
        parser.error("sample count or interval outside bounds")
    if any((args.panel_origin, args.server_id, args.cookie_file)) and not all((args.panel_origin, args.server_id, args.cookie_file)):
        parser.error("panel origin, positive server id and cookie file must be provided together")
    if args.server_id is not None and args.server_id <= 0:
        parser.error("server id must be positive")
    args.output.mkdir(mode=0o700, parents=True, exist_ok=False)
    os.chmod(args.output, 0o700)
    cookie = read_cookie(args.cookie_file) if args.cookie_file else None
    for index in range(args.samples):
        directory = args.output / f"sample-{index:03d}"
        directory.mkdir(mode=0o700)
        summary = {"started_at": int(time.time()), "commands": {}, "workload_started": False}
        for label, command in COMMANDS.items():
            if args.container:
                command = shlex.join(["docker", "exec", args.container, "sh", "-c", command])
            try:
                summary["commands"][label] = {"exit_code": remote(args.host, command, directory / (label + ".txt"))}
            except (subprocess.TimeoutExpired, ValueError):
                summary["commands"][label] = {"error": "capture timed out or exceeded output limit"}
        if cookie:
            try:
                summary["panel"] = read_panel(args.panel_origin, args.server_id, cookie)
            except Exception:
                # URLs and HTTP response bodies can contain secrets; keep only this safe error.
                summary["panel"] = {"error": "panel timestamps unavailable"}
        else:
            summary["panel"] = {"error": "panel credentials not supplied"}
        summary["finished_at"] = int(time.time())
        private_file(directory / "summary.json", json.dumps(summary, indent=2) + "\n")
        if index + 1 < args.samples:
            time.sleep(args.interval)
    print("Private baseline captured; no diagnostic was started. Review timestamps and evidence before classifying.")


if __name__ == "__main__":
    main()
