#!/usr/bin/env python3
"""Reject generated build outputs in the Git index, including forced additions."""
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent.parent
files = subprocess.check_output(["git", "ls-files", "-z"], cwd=root).decode().split("\0")
prefixes = ("web/dist/", "web/node_modules/", "target/", "artifacts/", ".local/", ".ci-artifacts/", ".ci-binaries/")
unexpected = [name for name in files if name.startswith(prefixes)]
if unexpected:
    raise SystemExit("Generated files must not be tracked:\n" + "\n".join(unexpected[:20]))
print("Source tree contains no tracked build outputs")
