#!/usr/bin/env python3
"""Run the independently provisioned CI bootstrap without putting tokens in argv."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import urllib.parse


def bootstrap(enrollment, trust_directory, bundle, log):
    enrollment, trust_directory, bundle, log = map(Path, (enrollment, trust_directory, bundle, log))
    metadata = enrollment.lstat()
    if not enrollment.is_file() or enrollment.is_symlink() or metadata.st_mode & 0o077:
        raise ValueError("enrollment must be an ordinary private file")
    descriptor = json.loads(enrollment.read_text())
    version, tag, token, panel = (descriptor[key] for key in ("version", "tag", "token", "origin"))
    if version == "latest" and tag is None:
        # The panel offers "latest" without a tag; this offline run installs
        # the one bundled release, whose signed metadata bootstrap verifies.
        tag = json.loads((bundle / "release.json").read_text()).get("tag")
        if not isinstance(tag, str) or not re.fullmatch(r"agent-v[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?", tag):
            raise ValueError("bundled release has no valid Agent tag")
    elif tag != "agent-v" + version:
        raise ValueError("invalid private enrollment descriptor")
    if not isinstance(token, str) or not token or any(ord(c) < 32 or ord(c) == 127 for c in token):
        raise ValueError("invalid private enrollment descriptor")
    parsed = urllib.parse.urlsplit(panel)
    if parsed.scheme != "http" or parsed.hostname != "127.0.0.1" or parsed.path or parsed.query or parsed.fragment or parsed.username or parsed.password:
        raise ValueError("CI bootstrap requires this run's loopback panel origin")
    environment = dict(os.environ, APSP_ENROLLMENT_TOKEN=token)
    arguments = ["/usr/bin/python3", str(trust_directory / "bootstrap.py"), "--tag", tag,
                 "--panel", panel, "--trusted-keys", str(trust_directory / "public-keys.json"),
                 "--minisign", "/usr/bin/minisign", "--release-dir", str(bundle)]
    with log.open("wb") as output:
        code = subprocess.run(arguments, env=environment, stdout=output, stderr=subprocess.STDOUT,
                              check=False, timeout=600).returncode
    if code:
        # The run log stays private; surface only its last lines, without the
        # enrollment token, so a refused TEST_ONLY installation is diagnosable.
        tail = log.read_bytes()[-2048:].decode("utf-8", "replace").replace(token, "<token>")
        print("CI bootstrap exited " + str(code) + ":\n" + "\n".join(tail.splitlines()[-6:]), file=sys.stderr)
    return code


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for argument in ("enrollment", "trust-directory", "bundle", "log"):
        parser.add_argument("--" + argument, type=Path, required=True)
    args = parser.parse_args()
    if os.getuid() != 0:
        raise ValueError("CI bootstrap driver requires root on a disposable runner")
    raise SystemExit(bootstrap(args.enrollment, args.trust_directory, args.bundle, args.log))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError, json.JSONDecodeError, subprocess.SubprocessError):
        raise SystemExit("CI bootstrap refused; inspect only the private run log") from None
