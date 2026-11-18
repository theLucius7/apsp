#!/usr/bin/env python3
"""Build native Agents or verify prebuilt binaries on their target operating system."""

import argparse
import hashlib
import json
import os
import pathlib
import re
import shutil
import struct
import subprocess
import sys
import tomllib

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
from artifact_manifest import publish
from release import (
    IPQUALITY_DEFAULT_VERSION as DEFAULT_IPQUALITY_VERSION,
    MAX_AGENT_RAW as MAX_AGENT_BINARY,
    NODEQUALITY_VERSION as DEFAULT_NODEQUALITY_VERSION,
    SINGBOX_VERSION as DEFAULT_SINGBOX_VERSION,
    WORKBENCH_VERSION as DEFAULT_WORKBENCH_VERSION,
    verify_embedded_agent_manifest,
)


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


def is_release_embed_required(env=None):
    source = os.environ if env is None else env
    return source.get("APSP_RELEASE_EMBED_REQUIRED", "").strip().lower() in (
        "1",
        "true",
        "yes",
        "on",
    )


def expected_embedded_versions(env=None):
    source = os.environ if env is None else env
    return {
        "sing-box": source.get("APSP_EMBEDDED_SINGBOX_VERSION") or DEFAULT_SINGBOX_VERSION,
        "nodequality": source.get("APSP_EMBEDDED_NODEQUALITY_VERSION")
        or DEFAULT_NODEQUALITY_VERSION,
        "ipquality": source.get("APSP_EMBEDDED_IPQUALITY_VERSION")
        or DEFAULT_IPQUALITY_VERSION,
        "network-workbench": source.get("APSP_EMBEDDED_NETWORK_WORKBENCH_VERSION")
        or DEFAULT_WORKBENCH_VERSION,
    }


def target_platform_arch(target):
    architecture = "arm64" if target.startswith("aarch64") else "amd64"
    platform = (
        "macos"
        if "apple" in target
        else "windows"
        if "windows" in target
        else "freebsd"
        if "freebsd" in target
        else "linux-gnu"
    )
    return platform, architecture


def ordinary_archive(path, label):
    path = pathlib.Path(path)
    if path.is_symlink() or not path.is_file() or path.stat().st_size <= 0:
        raise ValueError(f"missing or invalid {label} archive: {path}")
    return str(path.resolve())


def prepare_embedded_environment(target, artifact_root):
    env = dict(os.environ)
    platform, arch = target_platform_arch(target)
    release_required = is_release_embed_required(env)
    versions = expected_embedded_versions(env)

    singbox_version = versions["sing-box"]
    singbox_explicit = env.get("APSP_EMBEDDED_SINGBOX_ARCHIVE")
    if singbox_explicit:
        singbox_path = ordinary_archive(singbox_explicit, "sing-box")
    else:
        primary = artifact_root / "sing-box" / singbox_version / f"{platform}-{arch}"
        fallback = artifact_root / "sing-box" / singbox_version / arch
        if platform == "linux-gnu" and not primary.exists() and fallback.exists():
            singbox_path = ordinary_archive(fallback, "sing-box")
        else:
            singbox_path = ordinary_archive(primary, "sing-box")
    env["APSP_EMBEDDED_SINGBOX_VERSION"] = singbox_version
    env["APSP_EMBEDDED_SINGBOX_ARCHIVE"] = singbox_path

    if platform == "linux-gnu":
        for env_archive, env_version, name, default_ver in (
            (
                "APSP_EMBEDDED_NODEQUALITY_ARCHIVE",
                "APSP_EMBEDDED_NODEQUALITY_VERSION",
                "nodequality",
                versions["nodequality"],
            ),
            (
                "APSP_EMBEDDED_IPQUALITY_ARCHIVE",
                "APSP_EMBEDDED_IPQUALITY_VERSION",
                "ipquality",
                versions["ipquality"],
            ),
            (
                "APSP_EMBEDDED_NETWORK_WORKBENCH_ARCHIVE",
                "APSP_EMBEDDED_NETWORK_WORKBENCH_VERSION",
                "network-workbench",
                versions["network-workbench"],
            ),
        ):
            explicit = env.get(env_archive)
            candidate = (
                pathlib.Path(explicit)
                if explicit
                else (artifact_root / name / default_ver / arch)
            )
            if explicit or release_required or candidate.exists() or candidate.is_symlink():
                env[env_version] = default_ver
                env[env_archive] = ordinary_archive(candidate, name)

    if not env.get("APSP_NATIVE_TCP_SOURCE_COMMIT") and shutil.which("git"):
        try:
            commit = capture(
                ["git", "-C", str(REPOSITORY), "rev-parse", "--verify", "HEAD"]
            ).strip()
        except (OSError, subprocess.SubprocessError):
            commit = ""
        if re.fullmatch(r"[0-9a-f]{40}", commit):
            env["APSP_NATIVE_TCP_SOURCE_COMMIT"] = commit

    return env


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


def verify_binary(binary, target, version, env=None):
    if binary.is_symlink() or not binary.is_file():
        raise ValueError("Agent binary must be an ordinary file")
    size = binary.stat().st_size
    if not 0 < size <= MAX_AGENT_BINARY:
        raise ValueError(f"Agent binary size ({size} bytes) outside 1..={MAX_AGENT_BINARY}")
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
    tcp_probe_version = capture([str(binary), "tcp-probe", "--version"]).strip()
    if not tcp_probe_version.startswith("apsp-tcp-probe "):
        raise ValueError(f"unexpected built-in tcp-probe version: {tcp_probe_version}")
    capture([str(binary), "--help"])
    help_text = capture([str(binary), "run", "--help"])
    if "--monitor-only" not in help_text:
        raise ValueError("Agent does not expose its native monitoring runtime")
    verify_embedded_agent_manifest(
        binary.read_bytes(),
        version,
        target,
        require_release_closure=is_release_embed_required(env),
        expected_versions=expected_embedded_versions(env),
    )


def package_binary(binary, target, version, artifact_root):
    output = artifact_root / "agent" / version / target
    output.mkdir(parents=True)
    try:
        name = "apsp-agent.exe" if "windows" in target else "apsp-agent"
        destination = output / name
        shutil.copy2(binary, destination)
        payload = destination.read_bytes()
        digest = hashlib.sha256(payload).hexdigest()
        (output / "SHA256SUMS").write_text(
            f"{digest}  {name}\n", encoding="utf-8", newline="\n"
        )
        platform, architecture = target_platform_arch(target)
        target_name = f"{platform}-{architecture}"
        publish(output.parent, target_name, payload)
        if "windows" not in target:
            (output.parent / target_name).chmod(0o755)
    except BaseException:
        shutil.rmtree(output)
        raise
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("target", choices=TARGETS)
    parser.add_argument("artifact_root", type=pathlib.Path)
    parser.add_argument(
        "--binary",
        type=pathlib.Path,
        help="verify and package an existing binary without a Rust toolchain",
    )
    args = parser.parse_args()
    if args.binary is not None:
        manifest = tomllib.loads(
            (REPOSITORY / "crates/agent/Cargo.toml").read_text(encoding="utf-8")
        )
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
                    "cargo",
                    "metadata",
                    "--locked",
                    "--no-deps",
                    "--format-version",
                    "1",
                    "--manifest-path",
                    str(REPOSITORY / "Cargo.toml"),
                ]
            )
        )
        version = next(
            package["version"]
            for package in metadata["packages"]
            if package["name"] == "apsp-agent"
        )
    if not isinstance(version, str) or not re.fullmatch(
        r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", version
    ):
        parser.error("invalid Agent package version")
    output = args.artifact_root / "agent" / version / args.target
    if output.exists() or output.is_symlink():
        parser.error(f"immutable artifact already exists: {output}")
    if args.binary is not None:
        binary = args.binary.resolve()
        verify_env = os.environ
    else:
        verify_env = prepare_embedded_environment(args.target, args.artifact_root)
        subprocess.run(
            [
                "cargo",
                "build",
                "--locked",
                "--release",
                "--package",
                "apsp-agent",
                "--target",
                args.target,
                "--manifest-path",
                str(REPOSITORY / "Cargo.toml"),
            ],
            check=True,
            env=verify_env,
        )
        name = "apsp-agent.exe" if "windows" in args.target else "apsp-agent"
        binary = pathlib.Path(metadata["target_directory"]) / args.target / "release" / name
    verify_binary(binary, args.target, version, env=verify_env)
    print(f"Artifact: {package_binary(binary, args.target, version, args.artifact_root)}")


if __name__ == "__main__":
    main()
