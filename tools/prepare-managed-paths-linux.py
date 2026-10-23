#!/usr/bin/env python3
"""Build frozen native binaries and a new TEST_ONLY signed runtime, never publish."""
import argparse
import base64
import gzip
import importlib.util
import io
import json
import os
from pathlib import Path
import platform
import re
import shutil
import signal
import struct
import tarfile
import time
import tomllib
import uuid

from managed_paths_support import absolute, capture, digest, identity, load, regular, require, write

TARGETS = {"aarch64-unknown-linux-gnu": ("aarch64", "linux-gnu-arm64", 183),
           "x86_64-unknown-linux-gnu": ("x86_64", "linux-gnu-amd64", 62)}
MIN_DISK = 4 * 1024**3
MIN_MEMORY = 1280 * 1024**2
RESERVE_DISK = 1536 * 1024**2
RESERVE_MEMORY = 256 * 1024**2


def module(name, path):
    specification = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(value)
    return value


def verify_frozen(source, frozen_file, frozen_sha):
    encoded = regular(frozen_file)
    require(digest(encoded) == frozen_sha, "frozen_manifest_identity_mismatch")
    frozen = json.loads(encoded)
    rows = frozen.get("files")
    require(frozen.get("schema") == 1 and isinstance(rows, dict) and 1 <= len(rows) <= 3000,
            "frozen_manifest_shape_invalid")
    require({"Cargo.toml", "Cargo.lock", "AGENTS.md", "crates/agent-core/src/config.rs",
             "deploy/apsp-agent.service", "plugins/sing-box/apsp-singbox@.service"} <= rows.keys(),
            "frozen_manifest_missing_build_inputs")
    for relative, expected in rows.items():
        path = Path(relative)
        require(not path.is_absolute() and ".." not in path.parts and path.parts,
                "frozen_path_escape")
        candidate = source / path
        for parent in (candidate, *candidate.parents):
            if parent == source:
                break
            require(not parent.is_symlink(), "frozen_symlink_refused")
        require(identity(candidate, 32 * 1024**2) == expected, "frozen_source_changed")
    # Cargo cannot reach unlisted crate manifests or module inputs from this copy.
    required = set()
    for name in ("crates", "plugins", "tools", "scripts", "deploy", "web/src", "web/public", "web/dist"):
        for path in (source / name).rglob("*"):
            require(not path.is_symlink(), "source_tree_symlink_refused")
            if path.is_file() and "__pycache__" not in path.parts:
                required.add(path.relative_to(source).as_posix())
    require(required <= rows.keys(), "unfrozen_build_input")
    require(any(name.startswith("web/dist/") for name in rows), "frozen_embedded_web_dist_required")
    canonical = json.dumps(rows, sort_keys=True, separators=(",", ":")).encode()
    return {"head": frozen.get("head"), "frozen_inputs_sha256": frozen_sha,
            "functional_sha256": digest(canonical), "file_count": len(rows),
            "cargo_lock_sha256": rows["Cargo.lock"]["sha256"]}


def memory_available():
    values = dict(line.split(":", 1) for line in Path("/proc/meminfo").read_text().splitlines())
    return int(values["MemAvailable"].split()[0]) * 1024


def capacity(paths, initial=False):
    spaces = {str(path): shutil.disk_usage(path).free for path in paths}
    inodes = {str(path): os.statvfs(path).f_favail for path in paths}
    memory = memory_available()
    required_disk = MIN_DISK if initial else RESERVE_DISK
    required_memory = MIN_MEMORY if initial else RESERVE_MEMORY
    require(all(value >= required_disk for value in spaces.values()), "managed_build_disk_reserve_rejected")
    require(all(value >= 16384 for value in inodes.values()), "managed_build_inode_reserve_rejected")
    require(memory >= required_memory, "managed_build_memory_reserve_rejected")
    return {"available_memory_bytes": memory, "free_disk_bytes": spaces, "free_inodes": inodes,
            "minimum_free_disk_bytes": required_disk, "minimum_available_memory_bytes": required_memory}


def native_header(path, target):
    data = regular(path, 128 * 1024**2)
    require(data[:6] == b"\x7fELF\x02\x01" and len(data) >= 64 and
            struct.unpack_from("<H", data, 18)[0] == TARGETS[target][2], "native_elf_identity_mismatch")
    return data


def archive_runtime(data):
    target = io.BytesIO()
    with gzip.GzipFile(fileobj=target, mode="wb", mtime=0, filename="") as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            member = tarfile.TarInfo("sing-box")
            member.size, member.mode = len(data), 0o755
            member.uid = member.gid = member.mtime = 0
            archive.addfile(member, io.BytesIO(data))
    return target.getvalue()


def store(path, data, mode=0o600):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, mode)
    with os.fdopen(fd, "wb") as destination:
        destination.write(data)


def verify_bounded_bundle(tools, bundle, roots, expected_tag):
    require(len(roots) == 1 and roots[0] == tools.TEST_ONLY_PUBLIC_KEY, "exact_public_test_root_required")
    lines = regular(bundle / "SHA256SUMS.minisig", 16384).decode().splitlines()
    require(len(lines) == 4 and lines[0].startswith("untrusted comment: ") and
            lines[2].startswith("trusted comment: ") and
            base64.b64decode(lines[1], validate=True)[:2] == b"ED", "complete_prehashed_signature_required")
    capture(["minisign", "-V", "-H", "-q", "-m", str(bundle / "SHA256SUMS"), "-x",
             str(bundle / "SHA256SUMS.minisig"), "-P", roots[0]], timeout=10)
    metadata, rows = tools.validate_manifest(bundle, expected_tag)
    expected_files = {"release.json", "install.sh", "SHA256SUMS", "SHA256SUMS.minisig"}
    for entry in metadata["artifacts"]:
        require(entry["name"] in {"agent", "sing-box"}, "managed_release_role_invalid")
        data = regular(bundle / entry["asset_name"], 128 * 1024**2)
        require(len(data) == entry["archive_size"] and digest(data) == rows[tools.canonical_path(entry)],
                "independent_archive_verification_mismatch")
        binary = tools.binary_bytes(data, entry["format"], entry["binary_name"])
        require(len(binary) == entry["binary_size"] and digest(binary) == entry["binary_sha256"],
                "independent_binary_verification_mismatch")
        expected_files.add(entry["asset_name"])
    require({path.name for path in bundle.iterdir()} == expected_files, "unexpected_signed_bundle_file")
    return metadata


def stage_release(source, output, target, runtime, runtime_sha, version):
    tools = module("managed_release", source / "tools/release.py")
    signer = module("managed_signer", source / "scripts/ci-signed-release.py")
    roots = tools.load_roots(source / "crates/protocol/tests/fixtures/public-keys.json")
    denied = False
    try:
        tools.load_roots(source / "crates/protocol/tests/fixtures/public-keys.json", publication=True)
    except ValueError:
        denied = True
    require(denied, "test_root_official_publication_not_rejected")
    runtime_bytes = native_header(runtime, target)
    require(digest(runtime_bytes) == runtime_sha, "runtime_binary_changed")
    info = capture([str(runtime), "version"]).decode()
    require(info.splitlines()[0] == "sing-box version 1.14.2", "runtime_version_mismatch")
    tags = next((line.partition(":")[2].strip().split(",") for line in info.splitlines()
                 if line.startswith("Tags:")), [])
    require({"with_clash_api", "with_v2ray_api", "with_utls", "with_quic"} <= set(tags),
            "runtime_capabilities_missing")
    agent_bytes = native_header(output / "bin/apsp-agent", target)
    artifacts = []
    bundle = output / "test-only-release"
    bundle.mkdir(mode=0o700)
    archive = archive_runtime(runtime_bytes)
    for name, artifact_version, format_, binary, data in (
            ("agent", version, "raw", "apsp-agent", agent_bytes),
            ("sing-box", "1.14.2", "tar.gz", "sing-box", archive)):
        actual_binary = agent_bytes if name == "agent" else runtime_bytes
        entry = {"name": name, "version": artifact_version, "arch": TARGETS[target][1],
                 "format": format_, "binary_name": binary, "archive_size": len(data),
                 "binary_sha256": digest(actual_binary), "binary_size": len(actual_binary)}
        entry["asset_name"] = tools.asset_name(entry)
        store(bundle / entry["asset_name"], data)
        artifacts.append(entry)
    metadata = {"schema": 1, "source_repo": "theLucius7/apsp", "tag": "agent-v" + version,
                "protocol_min": 1, "protocol_max": 1, "artifacts": artifacts}
    encoded = (json.dumps(metadata, sort_keys=True, separators=(",", ":")) + "\n").encode()
    installer = b"#!/bin/sh\n# TEST_ONLY inert acceptance asset; not a production installer.\nexit 1\n"
    store(bundle / "release.json", encoded)
    store(bundle / "install.sh", installer)
    rows = {"release.json": digest(encoded), "install.sh": digest(installer)}
    rows.update({tools.canonical_path(entry): digest(regular(bundle / entry["asset_name"], 128 * 1024**2))
                 for entry in artifacts})
    store(bundle / "SHA256SUMS", "".join(rows[name] + "  " + name + "\n" for name in sorted(rows)).encode())
    capture(["minisign", "-S", "-q", "-m", str(bundle / "SHA256SUMS"), "-s",
             str(source / "crates/protocol/tests/fixtures/TEST_ONLY.key"), "-x",
             str(bundle / "SHA256SUMS.minisig"), "-t", "Apsp TEST_ONLY managed acceptance; never publish"])
    confirmed = verify_bounded_bundle(tools, bundle, roots, metadata["tag"])
    require(confirmed == metadata, "independent_release_verification_mismatch")
    signer.panel_tree(bundle, output / "panel-artifacts", metadata, tools)
    capture([str(output / "bin/apsp-agent"), "verify-release", "--proof-dir", str(bundle)])
    return {"test_only": True, "official_publication_rejected": True, "metadata": metadata,
            "metadata_sha256": digest(encoded), "checksums_sha256": identity(bundle / "SHA256SUMS")["sha256"],
            "signature_sha256": identity(bundle / "SHA256SUMS.minisig")["sha256"], "runtime_tags": tags}


def prepare(args):
    require(args.dedicated_test_node and platform.system() == "Linux" and
            platform.machine() == TARGETS[args.target][0], "dedicated_native_linux_required")
    source, output, target_dir = map(absolute, (args.source_root, args.output_dir, args.target_dir))
    require(not output.exists() and not output.is_symlink() and not target_dir.exists() and
            not target_dir.is_symlink(), "fresh_build_and_output_required")
    require(not output.is_relative_to(source) and not target_dir.is_relative_to(source),
            "build_outputs_must_be_outside_frozen_source")
    frozen_identity = verify_frozen(source, args.frozen_inputs, args.frozen_inputs_sha256)
    runtime_identity = identity(args.runtime_binary, 128 * 1024**2)
    require(runtime_identity["sha256"] == args.runtime_sha256, "runtime_identity_mismatch")
    run_id = str(uuid.uuid4())
    output.mkdir(mode=0o700)
    store(output / ".apsp-managed-test-run", (run_id + "\n").encode())
    before = {"source_identity": frozen_identity, "test_only": True, "run_id": run_id,
              "capacity": {"output_free_bytes": shutil.disk_usage(output).free,
                           "target_free_bytes": shutil.disk_usage(target_dir.parent).free,
                           "available_memory_bytes": memory_available()},
              "budgets": {"build_timeout_secs": 900, "minimum_free_bytes": MIN_DISK,
                          "disk_reserve_bytes": RESERVE_DISK, "memory_reserve_bytes": RESERVE_MEMORY}}
    write(output / "preflight.json", before)
    try:
        capacity([output, target_dir.parent], initial=True)
        require(Path("/sys/fs/cgroup/cgroup.controllers").is_file() and shutil.which("systemd-run"),
                "systemd_cgroup_v2_build_required")
        cargo_home, toolchain = map(absolute, (args.cargo_home, args.toolchain_dir))
        require(not (cargo_home / "config").exists() and not (cargo_home / "config.toml").exists(),
                "unreviewed_cargo_configuration_refused")
        require(all(not (directory / (".cargo/" + name)).exists()
                    for directory in (source, *source.parents) for name in ("config", "config.toml")),
                "unfrozen_cargo_configuration_refused")
        host = capture([str(toolchain / "bin/rustc"), "-vV"]).decode()
        require("host: " + args.target in host.splitlines(), "native_toolchain_mismatch")
        trust = module("managed_test_trust", source / "scripts/ci-test-trust.py").public_keys()
        env = {"PATH": str(toolchain / "bin") + ":/usr/bin:/bin", "LANG": "C.UTF-8", "TZ": "UTC",
               "CARGO_HOME": str(cargo_home), "CARGO_TARGET_DIR": str(target_dir),
               "CARGO_BUILD_JOBS": "1", "CARGO_INCREMENTAL": "0", "CARGO_PROFILE_DEV_DEBUG": "0",
               "APSP_RELEASE_PUBLIC_KEYS": trust}
        unit = "apsp-managed-build-" + run_id
        command = ["systemd-run", "--quiet", "--pipe", "--wait", "--collect", "--unit=" + unit,
                   "--property=MemoryMax=1G", "--property=MemorySwapMax=0", "--property=TasksMax=128",
                   "--property=PrivateNetwork=yes", "--property=KillMode=control-group",
                   "--property=RuntimeMaxSec=900", "--property=TimeoutStopSec=5",
                   "--property=WorkingDirectory=" + str(source),
                   "--property=UnsetEnvironment=RUSTFLAGS RUSTDOCFLAGS RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER RUSTC CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET RUSTUP_TOOLCHAIN APSP_CI_FIXTURE_SIGNER"]
        command.extend("--setenv=" + key + "=" + value for key, value in env.items())
        command.extend(["/usr/bin/env", "-i", *[key + "=" + value for key, value in env.items()],
                        str(toolchain / "bin/cargo"), "build", "--locked", "--offline", "-p",
                        "apsp-agent", "-p", "apsp-panel"])
        try:
            capture(command, timeout=910, log=output / "native-build-log.json",
                    guard=lambda: capacity([output, target_dir.parent]))
        finally:
            state = capture(["systemctl", "show", unit, "--property=MainPID", "--property=ActiveState"], timeout=10).decode()
            if "MainPID=0" not in state or "ActiveState=active" in state or "ActiveState=activating" in state:
                capture(["systemctl", "stop", unit], timeout=10)
            state = capture(["systemctl", "show", unit, "--property=MainPID", "--property=ControlGroup"], timeout=10).decode()
            require("MainPID=0" in state, "managed_build_cleanup_not_confirmed")
            cgroup = next((line.partition("=")[2] for line in state.splitlines() if line.startswith("ControlGroup=")), "")
            if cgroup:
                events = Path("/sys/fs/cgroup") / cgroup.lstrip("/") / "cgroup.events"
                require(not events.exists() or "populated 0" in events.read_text().splitlines(),
                        "managed_build_cgroup_still_populated")
        require(verify_frozen(source, args.frozen_inputs, args.frozen_inputs_sha256) == frozen_identity,
                "source_changed_during_build")
        binaries = output / "bin"
        binaries.mkdir(mode=0o700)
        for name in ("apsp-agent", "apsp-panel"):
            store(binaries / name, native_header(target_dir / "debug" / name, args.target), 0o755)
        version = tomllib.loads(regular(source / "crates/agent/Cargo.toml").decode())["package"]["version"]
        require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?", version), "invalid_agent_version")
        builder = module("managed_native_agent", source / "tools/build-agent.py")
        builder.verify_binary(binaries / "apsp-agent", args.target, version)
        # The panel has no CLI parser: invoking --help would start its database
        # and migration path. Inspect ELF/linkage without executing that daemon.
        headers = capture(["readelf", "-lW", str(binaries / "apsp-panel")]).decode()
        dynamic = capture(["readelf", "-dW", str(binaries / "apsp-panel")]).decode()
        libraries = capture(["ldd", str(binaries / "apsp-panel")]).decode()
        require("INTERP" in headers and "[libc.so.6]" in dynamic and "not found" not in libraries,
                "panel_native_glibc_linkage_invalid")
        release = stage_release(source, output, args.target, args.runtime_binary, args.runtime_sha256, version)
        require(verify_frozen(source, args.frozen_inputs, args.frozen_inputs_sha256) == frozen_identity,
                "source_changed_during_release_preparation")
        result = {**before, "status": "prepared", "native_target": args.target,
                  "toolchain": host.splitlines(), "release": release,
                  "binaries": {name: identity(binaries / name) for name in ("apsp-agent", "apsp-panel")},
                  "finished_at": int(time.time()), "managed_acceptance": "not_run"}
        write(output / "prepared-artifacts.json", result)
        return result
    except BaseException as error:
        write(output / "prepare-failure.json", {"status": "refused_or_failed", "run_id": run_id,
              "failure_type": type(error).__name__, "code": str(error) if isinstance(error, ValueError) else "private_log_required",
              "materials_retained": True, "managed_acceptance": "not_run"})
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source-root", "frozen-inputs", "toolchain-dir", "cargo-home", "target-dir",
                 "runtime-binary", "output-dir"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--frozen-inputs-sha256", required=True)
    parser.add_argument("--runtime-sha256", required=True)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--dedicated-test-node", action="store_true")
    args = parser.parse_args()
    def cancelled(_number, _frame):
        raise ValueError("managed_preparation_cancelled")
    for number in (signal.SIGTERM, signal.SIGINT):
        signal.signal(number, cancelled)
    try:
        result = prepare(args)
    except (ValueError, OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise SystemExit("Managed native preparation refused: " +
                         (str(error) if isinstance(error, ValueError) else type(error).__name__)) from None
    print(json.dumps({"run_id": result["run_id"], "status": "prepared", "test_only": True,
                      "managed_acceptance": "not_run"}))


if __name__ == "__main__":
    main()
