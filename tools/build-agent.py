#!/usr/bin/env python3
"""Build native Agents or verify prebuilt binaries on their target operating system."""

import argparse
import hashlib
import json
import pathlib
import re
import shutil
import struct
import subprocess
import tomllib
import sys
sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from artifact_manifest import publish


TARGETS = (
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
    "x86_64-unknown-freebsd",
    "aarch64-unknown-freebsd",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
)
REPOSITORY = pathlib.Path(__file__).resolve().parent.parent


def capture(command):
    return subprocess.run(
        command, check=True, capture_output=True, text=True, encoding="utf-8", timeout=30
    ).stdout


def verify_architecture(binary, target):
    with binary.open("rb") as source:
        header = source.read(64)
        if "windows" in target:
            if len(header) != 64 or header[:2] != b"MZ":
                raise ValueError("expected a Windows PE executable")
            source.seek(struct.unpack_from("<I", header, 60)[0])
            pe_header = source.read(6)
            expected = 0xAA64 if target.startswith("aarch64") else 0x8664
            if (
                len(pe_header) != 6
                or pe_header[:4] != b"PE\0\0"
                or struct.unpack_from("<H", pe_header, 4)[0] != expected
            ):
                raise ValueError("Windows binary has the wrong architecture")
        elif "apple" in target:
            if (
                len(header) < 8
                or header[:4] != b"\xcf\xfa\xed\xfe"
                or struct.unpack_from("<I", header, 4)[0] != 0x0100000C
            ):
                raise ValueError("expected an arm64 Mach-O executable")
        else:
            expected = 183 if target.startswith("aarch64") else 62
            if (
                len(header) != 64
                or header[:6] != b"\x7fELF\x02\x01"
                or struct.unpack_from("<H", header, 18)[0] != expected
            ):
                raise ValueError("ELF binary has the wrong format or architecture")


def verify_binary(binary, target, version):
    verify_architecture(binary, target)
    if "linux-gnu" in target:
        headers = capture(["readelf", "-lW", str(binary)])
        dynamic = capture(["readelf", "-dW", str(binary)])
        if "INTERP" not in headers or "[libc.so.6]" not in dynamic:
            raise ValueError("GNU/Linux Agent must dynamically link glibc")
        libraries = capture(["ldd", str(binary)])
        if "not found" in libraries:
            raise ValueError("GNU/Linux Agent has unresolved shared libraries")
        print(libraries, end="")
    actual_version = capture([str(binary), "--version"]).strip()
    if actual_version != f"apsp-agent {version}":
        raise ValueError(f"unexpected Agent version: {actual_version}")
    capture([str(binary), "--help"])
    help_text = capture([str(binary), "run", "--help"])
    if "--monitor-only" not in help_text:
        raise ValueError("Agent does not expose its native monitoring runtime")


def package_binary(binary, target, version, artifact_root):
    output = artifact_root / "agent" / version / target
    output.mkdir(parents=True)
    try:
        name = "apsp-agent.exe" if "windows" in target else "apsp-agent"
        destination = output / name
        shutil.copy2(binary, destination)
        digest = hashlib.sha256(destination.read_bytes()).hexdigest()
        (output / "SHA256SUMS").write_text(
            f"{digest}  {name}\n", encoding="utf-8", newline="\n"
        )
        architecture = "arm64" if target.startswith("aarch64") else "amd64"
        platform = "macos" if "apple" in target else "windows" if "windows" in target else "freebsd" if "freebsd" in target else "linux-gnu"
        publish(output.parent, f"{platform}-{architecture}", destination.read_bytes())
    except BaseException:
        shutil.rmtree(output)
        raise
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=TARGETS)
    parser.add_argument("artifact_root", type=pathlib.Path)
    parser.add_argument(
        "--binary", type=pathlib.Path,
        help="verify and package an existing binary without a Rust toolchain",
    )
    args = parser.parse_args()
    if args.binary is not None:
        manifest = tomllib.loads((REPOSITORY / "crates/agent/Cargo.toml").read_text(encoding="utf-8"))
        version = manifest["package"]["version"]
    else:
        host = next(
            line.removeprefix("host: ")
            for line in capture(["rustc", "-vV"]).splitlines()
            if line.startswith("host: ")
        )
        if host != args.target:
            parser.error(f"a native {args.target} Rust toolchain is required; host is {host}")
        metadata = json.loads(
            capture(
                [
                    "cargo", "metadata", "--locked", "--no-deps", "--format-version", "1",
                    "--manifest-path", str(REPOSITORY / "Cargo.toml"),
                ]
            )
        )
        version = next(
            package["version"]
            for package in metadata["packages"]
            if package["name"] == "apsp-agent"
        )
    if not isinstance(version, str) or not re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", version):
        parser.error("invalid Agent package version")
    output = args.artifact_root / "agent" / version / args.target
    if output.exists() or output.is_symlink():
        parser.error(f"immutable artifact already exists: {output}")
    if args.binary is not None:
        binary = args.binary.resolve()
    else:
        subprocess.run(
            [
                "cargo", "build", "--locked", "--release", "--package", "apsp-agent",
                "--target", args.target, "--manifest-path", str(REPOSITORY / "Cargo.toml"),
            ],
            check=True,
        )
        name = "apsp-agent.exe" if "windows" in args.target else "apsp-agent"
        binary = pathlib.Path(metadata["target_directory"]) / args.target / "release" / name
    verify_binary(binary, args.target, version)
    print(f"Artifact: {package_binary(binary, args.target, version, args.artifact_root)}")


if __name__ == "__main__":
    main()
