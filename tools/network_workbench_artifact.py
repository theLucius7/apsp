#!/usr/bin/env python3
"""Prepare an offline workbench payload for the self-contained Agent build pipeline."""
import argparse
import copy
import gzip
import hashlib
import io
import json
from pathlib import Path
import subprocess
import tarfile

REQUIRED_OFFLINE_TOOLS = frozenset(
    {"ping", "curl", "mtr", "iperf3", "sysbench", "fio", "stress-ng", "smartctl"}
)
ROUTE_TOOLS = frozenset({"traceroute", "nexttrace"})
RESERVED_ARCHIVE_NAMES = frozenset(
    {"apsp-network-workbench", "tools-manifest.json", "LICENSE"}
)


def safe_relative_path(value):
    if not isinstance(value, str) or not value or Path(value).is_absolute():
        raise ValueError("bundled path must be a non-empty relative path")
    if any(ord(ch) < 32 or ord(ch) == 127 for ch in value) or "\\" in value:
        raise ValueError("bundled path contains invalid characters")
    parts = Path(value).parts
    if not parts or len(parts) > 12 or any(
        part in ("", ".", "..") or part.startswith("-") for part in parts
    ):
        raise ValueError("bundled path must not contain traversal or invalid segments")
    normalized = "/".join(parts)
    if normalized in RESERVED_ARCHIVE_NAMES or normalized == ".bundled.json":
        raise ValueError("bundled path collides with reserved workbench metadata")
    return normalized


def resolve_source_file(raw_path, bundle_root, allow_empty=False):
    candidate = Path(raw_path)
    if not candidate.is_absolute():
        candidate = bundle_root / candidate
    for check in (candidate, *candidate.parents):
        if check.is_symlink():
            raise ValueError(f"source path must not contain symlinks: {check}")
        if check == bundle_root:
            break
    if not candidate.is_file() or candidate.is_symlink():
        raise ValueError(f"source file must be an ordinary file: {candidate}")
    data = candidate.read_bytes()
    if not data and not allow_empty:
        raise ValueError(f"source file must not be empty: {candidate}")
    return candidate, data


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--inventory", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--bundle-root", type=Path, default=None)
    args = parser.parse_args()

    bundle_root = (args.bundle_root or args.inventory.parent).resolve()
    inventory = copy.deepcopy(json.loads(args.inventory.read_text(encoding="utf-8")))
    if inventory.get("schema") != 1 or inventory.get("engine_version") != "1.0.0":
        raise ValueError("explicit versioned inventory required")

    extra_files = {}
    bundled_lib_dirs = inventory.get("bundled_lib_dirs")
    run_env = None
    if bundled_lib_dirs is not None:
        if not isinstance(bundled_lib_dirs, list) or len(bundled_lib_dirs) > 16:
            raise ValueError("bundled_lib_dirs must be a bounded list")
        normalized_libs = [safe_relative_path(entry) for entry in bundled_lib_dirs]
        inventory["bundled_lib_dirs"] = normalized_libs
        source_lib_dirs = [str(bundle_root / entry) for entry in normalized_libs]
        run_env = {
            "PATH": "/usr/bin:/bin",
            "LANG": "C",
            "LC_ALL": "C",
            "LD_LIBRARY_PATH": ":".join(source_lib_dirs),
        }

    bundled_support = inventory.pop("bundled_files", None)
    if bundled_support is not None:
        if not isinstance(bundled_support, dict):
            raise ValueError("bundled_files must be a mapping")
        for rel_name, meta in bundled_support.items():
            normalized = safe_relative_path(rel_name)
            if not isinstance(meta, dict):
                raise ValueError(f"invalid bundled_files metadata for {normalized}")
            executable = bool(meta.get("executable"))
            source_ref = meta.get("source_path", normalized)
            _, content = resolve_source_file(
                source_ref, bundle_root, allow_empty=not executable
            )
            if meta.get("sha256") != hashlib.sha256(content).hexdigest():
                raise ValueError(f"bundled support file digest mismatch: {normalized}")
            mode = 0o755 if executable else 0o644
            extra_files[normalized] = (content, mode)

    bundled_ca = inventory.get("bundled_ca_bundle")
    if bundled_ca is not None:
        normalized_ca = safe_relative_path(bundled_ca)
        inventory["bundled_ca_bundle"] = normalized_ca
        if normalized_ca not in extra_files:
            _, ca_bytes = resolve_source_file(normalized_ca, bundle_root)
            extra_files[normalized_ca] = (ca_bytes, 0o644)
        if run_env is not None:
            ca_file = str(bundle_root / normalized_ca)
            run_env["SSL_CERT_FILE"] = ca_file
            run_env["CURL_CA_BUNDLE"] = ca_file

    notices_candidate = bundle_root / "THIRD_PARTY_NOTICES.txt"
    if (
        "THIRD_PARTY_NOTICES.txt" not in extra_files
        and not notices_candidate.is_symlink()
        and notices_candidate.is_file()
    ):
        _, notices_bytes = resolve_source_file(notices_candidate, bundle_root)
        extra_files["THIRD_PARTY_NOTICES.txt"] = (notices_bytes, 0o644)

    interpreter = inventory.get("interpreter", {})
    if not isinstance(interpreter, dict):
        raise ValueError("explicit approved Python interpreter digest required")
    if interpreter.get("bundled_path") is not None:
        bundled_interp = safe_relative_path(interpreter["bundled_path"])
        source_ref = interpreter.get("source_path") or interpreter.get("path") or bundled_interp
        _, interp_bytes = resolve_source_file(source_ref, bundle_root)
        if interpreter.get("sha256") != hashlib.sha256(interp_bytes).hexdigest():
            raise ValueError("explicit approved Python interpreter digest required")
        extra_files[bundled_interp] = (interp_bytes, 0o755)
        cleaned_interp = {"bundled_path": bundled_interp, "sha256": interpreter["sha256"]}
        if interpreter.get("bundled_python_home") is not None:
            cleaned_interp["bundled_python_home"] = safe_relative_path(
                interpreter["bundled_python_home"]
            )
        inventory["interpreter"] = cleaned_interp
    else:
        if (
            interpreter.get("path") != "/usr/bin/python3"
            or interpreter.get("sha256")
            != hashlib.sha256(Path("/usr/bin/python3").read_bytes()).hexdigest()
        ):
            raise ValueError("explicit approved Python interpreter digest required")

    tools = inventory.get("tools", {})
    if not isinstance(tools, dict):
        raise ValueError("inventory tools must be a mapping")
    for name, item in tools.items():
        if (
            not isinstance(item, dict)
            or not item.get("licensed")
            or not item.get("license")
            or not item.get("source_url", "").startswith("https://")
        ):
            raise ValueError(
                "absolute tool, license confirmation and official source evidence required: "
                + str(name)
            )
        if item.get("bundled_path") is not None:
            bundled_tool = safe_relative_path(item["bundled_path"])
            source_ref = item.get("source_path") or item.get("path") or bundled_tool
            exec_path, tool_bytes = resolve_source_file(source_ref, bundle_root)
        else:
            if "path" not in item or not Path(item["path"]).is_absolute():
                raise ValueError(
                    "absolute tool, license confirmation and official source evidence required: "
                    + str(name)
                )
            exec_path = Path(item["path"])
            tool_bytes = exec_path.read_bytes()

        digest = hashlib.sha256(tool_bytes).hexdigest()
        if item.get("sha256") != digest:
            raise ValueError(
                "installed tool differs from previously approved source digest: " + str(name)
            )
        flag = "-V" if name == "ping" else "--version"
        if run_env is not None:
            version = subprocess.run(
                [str(exec_path), flag],
                capture_output=True,
                timeout=3,
                check=True,
                env=run_env,
            )
        else:
            version = subprocess.run(
                [str(exec_path), flag], capture_output=True, timeout=3, check=True
            )
        text = (version.stdout + version.stderr).decode("utf-8", "replace")
        if item["version"] not in text or len(text) > 4096:
            raise ValueError("tool identity differs: " + str(name))
        item["version_output_sha256"] = hashlib.sha256(
            version.stdout + version.stderr
        ).hexdigest()
        if item.get("bundled_path") is not None:
            if bundled_tool in extra_files:
                raise ValueError(f"duplicate bundled payload path: {bundled_tool}")
            extra_files[bundled_tool] = (tool_bytes, 0o755)
            item["bundled_path"] = bundled_tool
            item.pop("path", None)
            item.pop("source_path", None)

    all_bundled = (
        bool(inventory["interpreter"].get("bundled_path"))
        and bool(tools)
        and all(
            bool(entry.get("bundled_path"))
            and "path" not in entry
            and entry["bundled_path"] in extra_files
            for entry in tools.values()
        )
    )
    complete_offline_bundle = (
        all_bundled
        and REQUIRED_OFFLINE_TOOLS.issubset(tools)
        and bool(ROUTE_TOOLS & set(tools))
    )

    root = Path(__file__).resolve().parents[1]
    base_files = {
        "apsp-network-workbench": (
            (root / "tools/network-workbench.py").read_bytes(),
            0o755,
        ),
        "tools-manifest.json": (
            json.dumps(inventory, sort_keys=True, separators=(",", ":")).encode("utf-8"),
            0o644,
        ),
        "LICENSE": ((root / "LICENSE").read_bytes(), 0o644),
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    raw_tar = io.BytesIO()
    with tarfile.open(fileobj=raw_tar, mode="w") as archive:
        for name, (content, mode) in sorted({**base_files, **extra_files}.items()):
            entry = tarfile.TarInfo(name)
            entry.mode = mode
            entry.size = len(content)
            entry.mtime = 0
            entry.uid = 0
            entry.gid = 0
            entry.uname = ""
            entry.gname = ""
            archive.addfile(entry, io.BytesIO(content))
    compressed = gzip.compress(raw_tar.getvalue(), mtime=0)
    args.output.write_bytes(compressed)
    print(
        json.dumps(
            {
                "artifact": str(args.output),
                "version": "1.0.0",
                "sha256": hashlib.sha256(compressed).hexdigest(),
                "signed": False,
                "release_authorized": complete_offline_bundle,
                "tool_count": len(tools),
            }
        )
    )


if __name__ == "__main__":
    main()
