#!/usr/bin/env python3
"""Build a locked, source-pinned native musl TCP artifact; never sign or publish."""
import argparse
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tempfile
import sys
import tomllib

from artifact_manifest import publish
from tcp_probe_notices import collect as collect_notices
from tcp_probe_artifact import BINARY, TARGETS, TOOL_VERSION, artifact_version, digest, ensure, pack, source_files, validate_files

ROOT = Path(__file__).resolve().parents[1]


def capture(command, **kwargs):
    return subprocess.run(command, check=True, capture_output=True, timeout=60, **kwargs).stdout


def pinned_source(repository, commit):
    version = artifact_version(commit)
    actual = capture(["git", "-C", str(repository), "rev-parse", "--verify", commit + "^{commit}"]).decode().strip()
    ensure(actual == commit, "source object does not match the explicit commit")
    ensure(not capture(["git", "-C", str(repository), "status", "--porcelain", "--untracked-files=normal"]),
           "TCP build requires a clean source repository")
    source = capture(["git", "-C", str(repository), "archive", "--format=tar.gz", commit])
    files = source_files(source, commit)
    workspace = tomllib.loads(files["Cargo.toml"].decode())
    ensure(workspace["workspace"]["package"]["version"] == TOOL_VERSION, "unexpected tool version")
    return version, source


def verify_binary(binary, commit):
    ensure(capture([str(binary), "--version"]).decode().strip() == f"{BINARY} {TOOL_VERSION}",
           "TCP executable version mismatch")
    help_text = capture([str(binary), "--help"]).decode()
    ensure("--workspace" in help_text and "--no-rank-upload" in help_text, "TCP executable help mismatch")
    info = json.loads(capture([str(binary), "--build-info"]))
    ensure(info == {"version": TOOL_VERSION, "source_repo": "theLucius7/apsp", "source_commit": commit},
           "TCP executable is missing or has different source provenance")


def build(repository, arch, commit):
    target = TARGETS[arch]
    env = dict(os.environ)
    ensure(not env.get("CARGO_ENCODED_RUSTFLAGS"), "unset CARGO_ENCODED_RUSTFLAGS")
    env["APSP_NATIVE_TCP_SOURCE_COMMIT"] = commit
    env["RUSTFLAGS"] = env.get("RUSTFLAGS", "") + " -Ctarget-feature=+crt-static -Clink-self-contained=yes"
    key = target.replace("-", "_")
    env[f"CARGO_TARGET_{key.upper()}_LINKER"] = "cc"
    env[f"CC_{key}"] = "musl-gcc"
    metadata = json.loads(capture(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1",
                                  "--manifest-path", str(repository / "Cargo.toml")], env=env))
    subprocess.run(["cargo", "build", "--locked", "--release", "--package", BINARY, "--target", target,
                    "--manifest-path", str(repository / "Cargo.toml")], env=env, check=True)
    binary = Path(metadata["target_directory"]) / target / "release" / BINARY
    ensure(binary.is_file() and not binary.is_symlink(), "TCP executable must be an ordinary file")
    verify_binary(binary, commit)
    return binary, capture(["rustc", "-vV"]).decode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("arch", choices=TARGETS)
    parser.add_argument("artifact_root", type=Path)
    parser.add_argument("--source-commit", required=True, help="explicit existing full 40-hex source commit; no branch names")
    parser.add_argument("--source-repository", type=Path, default=ROOT, help=argparse.SUPPRESS)
    parser.add_argument("--snapshot-archive", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    version, source = pinned_source(args.source_repository, args.source_commit)
    if args.snapshot_archive is None:
        # Execute the recipe in the fixed source, including when invoked by a newer Release tree.
        with tempfile.TemporaryDirectory(prefix="apsp-tcp-source-") as temporary:
            snapshot = Path(temporary) / "source"
            snapshot.mkdir(mode=0o700)
            for name, data in source_files(source, args.source_commit).items():
                target = snapshot / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
            archive = Path(temporary) / "source.tar.gz"
            archive.write_bytes(source)
            subprocess.run([sys.executable, str(snapshot / "tools/build-tcp-probe.py"), args.arch,
                            str(args.artifact_root.resolve()), "--source-commit", args.source_commit,
                            "--source-repository", str(args.source_repository.resolve()),
                            "--snapshot-archive", str(archive)], check=True)
        return
    ensure(args.snapshot_archive.is_file() and not args.snapshot_archive.is_symlink()
           and args.snapshot_archive.read_bytes() == source, "snapshot archive differs from fixed Git object")
    snapshot = source_files(source, args.source_commit)
    for name, data in snapshot.items():
        path = ROOT / name
        ensure(path.is_file() and not path.is_symlink() and path.read_bytes() == data,
               "build snapshot differs from pinned source")
    machine = "x86_64" if args.arch == "amd64" else "aarch64"
    ensure(platform.system() == "Linux" and platform.machine() == machine, "use the requested native Linux architecture")
    for command in ("cargo", "rustc", "cc", "musl-gcc"):
        ensure(shutil.which(command), f"missing build tool: {command}")
    output = args.artifact_root.resolve() / "tcpquality" / version
    ensure(not output.is_symlink() and not (output / args.arch).exists(), "immutable TCP artifact already exists")
    output.mkdir(parents=True, exist_ok=True)
    lock = output / ".build.lock"
    lock.mkdir()
    try:
        notices = collect_notices(ROOT, args.arch)
        binary, compiler = build(ROOT, args.arch, args.source_commit)
        ensure(collect_notices(ROOT, args.arch) == notices, "locked dependency or notices changed during build")
        # A concurrent source edit cannot be disguised by a previously captured source archive.
        final_version, final_source = pinned_source(args.source_repository, args.source_commit)
        ensure(final_version == version and final_source == source, "source changed during TCP build")
        ensure(all((ROOT / name).read_bytes() == data for name, data in snapshot.items()),
               "fixed snapshot changed during TCP build")
        files = {BINARY: binary.read_bytes(), "source.tar.gz": source,
                 "Cargo.lock": (ROOT / "Cargo.lock").read_bytes(), "LICENSE": (ROOT / "LICENSE").read_bytes(), "THIRD_PARTY_NOTICES.txt": notices}
        info = {"schema": 1, "tool": BINARY, "tool_version": TOOL_VERSION, "artifact_version": version,
                "source_repo": "theLucius7/apsp", "source_commit": args.source_commit,
                "target": TARGETS[args.arch], "rustc": compiler, "cargo_locked": True,
                "source_sha256": digest(source), "lock_sha256": digest(files["Cargo.lock"]),
                "license_sha256": digest(files["LICENSE"]), "binary_sha256": digest(files[BINARY]),
                "notices_sha256": digest(notices)}
        files["build-info.json"] = (json.dumps(info, sort_keys=True, separators=(",", ":")) + "\n").encode()
        validate_files(files, version, args.arch)
        publish(output, args.arch, pack(files))
        print(f"Artifact: {output / args.arch}")
    finally:
        lock.rmdir()


if __name__ == "__main__":
    main()
