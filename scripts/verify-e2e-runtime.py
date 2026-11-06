#!/usr/bin/env python3
"""Verify an amd64 runtime archive on every cache restore before installing it."""

import hashlib
from pathlib import Path
import re
import subprocess
import sys
import tarfile


VERSION = "1.14.0-apsp.1"
REVISION = "7cf0370571977d4a3ed01d35dcf08ffbd67f1d9b"
# The legacy service's build tags, plus the statistics API the Agent reads.
TAGS = "with_quic,with_utls,with_wireguard,with_clash_api,with_v2ray_api"


def require(condition, message):
    if not condition:
        raise ValueError(message)


def verify(root, destination):
    directory = root / "sing-box" / VERSION
    archive = directory / "amd64"
    sums = directory / "SHA256SUMS"
    require(archive.is_file() and not archive.is_symlink(), "runtime archive must be ordinary")
    require(sums.is_file() and not sums.is_symlink(), "runtime checksums must be ordinary")
    entries = {}
    for line in sums.read_text().splitlines():
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *](amd64|arm64)", line)
        require(match and match[2] not in entries, "invalid or duplicate runtime checksum")
        entries[match[2]] = match[1].lower()
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    require(entries.get("amd64") == digest, "runtime archive checksum mismatch")
    destination.mkdir(mode=0o700, parents=True, exist_ok=False)
    binary = destination / "sing-box"
    with tarfile.open(archive, "r:gz") as package:
        members = package.getmembers()
        require(len(members) == 1 and members[0].name == "sing-box", "unexpected runtime archive members")
        require(members[0].isfile() and 0 < members[0].size <= 512 * 1024 * 1024, "invalid binary member")
        with package.extractfile(members[0]) as source, binary.open("xb") as output:
            while chunk := source.read(1024 * 1024):
                output.write(chunk)
    binary.chmod(0o755)
    header = subprocess.check_output(["readelf", "-h", str(binary)], text=True, timeout=30)
    require("Advanced Micro Devices X86-64" in header, "runtime architecture mismatch")
    version = subprocess.check_output([str(binary), "version"], text=True, timeout=30)
    require(version.splitlines()[0] == f"sing-box version {VERSION}", "runtime version mismatch")
    tags_line = next(line for line in version.splitlines() if line.startswith("Tags: "))
    require(tags_line.removeprefix("Tags: ") == TAGS, "runtime tags mismatch")
    require(f"Revision: {REVISION}" in version, "runtime revision mismatch")
    require("CGO: disabled" in version, "runtime must be the pure Go build")
    metadata = subprocess.check_output(["go", "version", "-m", str(binary)], text=True, timeout=30)
    require(metadata.splitlines()[0].endswith(": go1.26.8"), "runtime toolchain mismatch")
    require("GOOS=linux" in metadata and "GOARCH=amd64" in metadata, "runtime build target mismatch")
    print(f"Pinned runtime verified: {VERSION}, amd64, legacy service tags and statistics API")
    return binary


if __name__ == "__main__":
    try:
        verify(Path(sys.argv[1]), Path(sys.argv[2]))
    except (AssertionError, OSError, ValueError, subprocess.SubprocessError, tarfile.TarError) as error:
        raise SystemExit("Runtime verification failed: " + str(error)) from None
