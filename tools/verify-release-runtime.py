#!/usr/bin/env python3
"""Inspect both pinned runtime architectures without executing cached binaries."""

import argparse
import hashlib
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tarfile
import tempfile

VERSION = "1.14.2"
REVISION = "af6e64c3b69e6132ebaee0e1a3d24e93903f6709"
TAGS = {
    "with_gvisor", "with_quic", "with_dhcp", "with_wireguard", "with_utls",
    "with_acme", "with_clash_api", "with_tailscale", "with_ccm", "with_ocm",
    "with_cloudflared", "with_naive_outbound", "with_usbip", "with_openvpn",
    "with_openconnect", "badlinkname", "tfogo_checklinkname0", "with_v2ray_api",
}
MAX_ARCHIVE = 256 * 1024 * 1024
MAX_BINARY = 512 * 1024 * 1024


def require(condition, message):
    if not condition:
        raise ValueError(message)


def ordinary(path, maximum):
    require(path.is_file() and not path.is_symlink(), "expected an ordinary file")
    require(0 < path.stat().st_size <= maximum, "file size exceeds the allowed bound")


def verify_metadata(metadata, arch, libc="gnu"):
    require(libc in ("gnu", "musl"), "unsupported libc")
    lines = metadata.splitlines()
    require(lines and lines[0].endswith(": go1.26.8"), "unexpected Go toolchain")
    values = {}
    for line in lines[1:]:
        fields = line.strip().split("\t", 1)
        if len(fields) == 2 and fields[0] == "build":
            key, separator, value = fields[1].partition("=")
            require(separator and key not in values, "invalid or duplicate build setting")
            values[key] = value
    for key, expected in {
        "GOOS": "linux", "GOARCH": arch, "CGO_ENABLED": "1",
        "-trimpath": "true", "vcs": "git", "vcs.revision": REVISION,
        "vcs.modified": "false", "-buildmode": "exe", "-compiler": "gc",
    }.items():
        require(values.get(key) == expected, "unexpected build setting: " + key)
    tags = values.get("-tags", "").split(",")
    expected_tags = TAGS | ({"with_musl"} if libc == "musl" else set())
    require(len(tags) == len(expected_tags) and set(tags) == expected_tags,
            "unexpected runtime build tags")
    # Trimmed Go metadata does not preserve the Version linker override.
    # The pinned source revision binds the expected release; this is no attestation.


def verify(root, arch, reader=None, *, libc=None):
    require(arch in ("amd64", "arm64"), "unsupported architecture")
    require(libc in (None, "gnu", "musl"), "unsupported libc")
    target = arch if libc is None else f"linux-{libc}-{arch}"
    directory = Path(root) / "sing-box" / VERSION
    archive, sums = directory / target, directory / "SHA256SUMS"
    ordinary(archive, MAX_ARCHIVE)
    ordinary(sums, 1024)
    entries = {}
    for line in sums.read_text(encoding="ascii").splitlines():
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *]((?:linux-(?:gnu|musl)-)?(?:amd64|arm64))", line)
        require(match and match[2] not in entries, "invalid or duplicate runtime checksum")
        entries[match[2]] = match[1].lower()
    with archive.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    require(entries.get(target) == digest, "runtime archive checksum mismatch")
    with tempfile.TemporaryDirectory(prefix="apsp-release-inspection-") as temporary:
        binary = Path(temporary) / "runtime"
        with tarfile.open(archive, "r|gz") as package:
            member = package.next()
            require(member is not None and member.name == "sing-box" and member.isfile(),
                    "expected one ordinary runtime binary")
            require(0 < member.size <= MAX_BINARY, "invalid binary member size")
            with package.extractfile(member) as source, binary.open("xb") as output:
                shutil.copyfileobj(source, output, length=1024 * 1024)
            require(binary.stat().st_size == member.size, "incomplete binary member")
            require(package.next() is None, "unexpected runtime archive member")
        with binary.open("rb") as source:
            header = source.read(64)
        require(len(header) == 64 and header[:7] == b"\x7fELF\x02\x01\x01",
                "expected a little-endian ELF64 binary")
        require(struct.unpack_from("<H", header, 18)[0] == {"amd64": 62, "arm64": 183}[arch],
                "runtime ELF architecture mismatch")
        if reader is None:
            metadata = subprocess.check_output(["go", "version", "-m", str(binary)],
                                               text=True, timeout=30)
        else:
            metadata = reader(binary)
        verify_metadata(metadata, arch, libc or "gnu")
        if libc == "musl":
            headers = subprocess.check_output(["readelf", "-lW", str(binary)],
                                               text=True, timeout=30)
            dynamic = subprocess.check_output(["readelf", "-dW", str(binary)],
                                               text=True, timeout=30)
            require("INTERP" not in headers and "NEEDED" not in dynamic,
                    "musl runtime must be statically linked")
    return digest


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--arch", choices=("amd64", "arm64"), required=True)
    parser.add_argument("--libc", choices=("gnu", "musl"))
    args = parser.parse_args()
    try:
        verify(args.root, args.arch, libc=args.libc)
    except (OSError, ValueError, UnicodeError, subprocess.SubprocessError, tarfile.TarError) as error:
        raise SystemExit("Runtime inspection failed: " + str(error)) from None
    print(f"Pinned runtime cache inspected: {VERSION}, {args.arch}, Go metadata and SHA-256")
