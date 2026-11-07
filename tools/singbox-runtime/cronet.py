#!/usr/bin/env python3
"""Fetch the exact upstream Cronet shared library included in a signed runtime."""
import argparse
import hashlib
from pathlib import Path
import urllib.request

COMMIT = "c10c03c318db"
LIBRARIES = {
    "linux_amd64": (11915768, "23109c55b08829bb1a68682a61a64852b1bbf243753b153cbd8f6e4ea2c05294"),
    "linux_arm64": (12381208, "3c1fcfcb56261a4b318ffb6d2227b9726782edb5a894e74db2b534406db62498"),
    "windows_amd64": (9529344, "3217c6260fbca5f16072e0b79735742f40109a63bb0ff88fd6b96dd6b54a2928"),
    "windows_arm64": (8441856, "a75a1b99a7e31802cf67ee14763e5cb9db6a24aaf7062f86c49f2fc1c030fcf0"),
}

def library_name(target):
    return "libcronet.dll" if target.startswith("windows_") else "libcronet.so"

def validate(target, data):
    size, digest = LIBRARIES[target]
    if len(data) != size or hashlib.sha256(data).hexdigest() != digest:
        raise ValueError("Cronet library differs from the pinned upstream artifact")

def fetch(target, directory):
    name = library_name(target)
    with urllib.request.urlopen(f"https://raw.githubusercontent.com/SagerNet/cronet-go/{COMMIT}/lib/{target}/{name}", timeout=60) as response:
        data = response.read(LIBRARIES[target][0] + 1)
    validate(target, data)
    path = Path(directory) / name
    path.write_bytes(data)
    path.chmod(0o755)
    return path

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=LIBRARIES)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    fetch(args.target, args.directory)
