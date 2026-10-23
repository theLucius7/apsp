#!/usr/bin/env python3
"""Sign a disposable native CI Release with the deliberately public test key."""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shutil
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/protocol/tests/fixtures"


def release_tools(directory):
    specification = importlib.util.spec_from_file_location("apsp_ci_release_tools", Path(directory) / "release.py")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def command(arguments):
    result = subprocess.run(arguments, check=False, capture_output=True, text=True)
    if result.returncode:
        raise ValueError("TEST_ONLY release preparation or independent verification failed")


def panel_tree(bundle, output, metadata, tools):
    """Publish only independently verified bytes into the panel's canonical signed store."""
    bundle, output = Path(bundle), Path(output)
    if output.exists() or output.is_symlink():
        raise ValueError("CI panel artifact root already exists")
    confirmed, rows = tools.validate_manifest(bundle, metadata["tag"])
    if confirmed != metadata:
        raise ValueError("signed metadata changed after independent verification")
    stage = output.with_name(".ci-release-" + uuid.uuid4().hex)
    try:
        directory = stage / "releases" / metadata["tag"]
        directory.mkdir(parents=True)
        for name in ("release.json", "SHA256SUMS", "SHA256SUMS.minisig", "install.sh"):
            (directory / name).write_bytes(tools.read_regular(bundle / name, 262144))
        for entry in metadata["artifacts"]:
            data = tools.read_regular(bundle / entry["asset_name"])
            if len(data) != entry["archive_size"] or hashlib.sha256(data).hexdigest() != rows[tools.canonical_path(entry)]:
                raise ValueError("signed archive size differs")
            binary = tools.binary_bytes(data, entry["format"], entry["binary_name"])
            if len(binary) != entry["binary_size"] or hashlib.sha256(binary).hexdigest() != entry["binary_sha256"]:
                raise ValueError("signed binary differs")
            destination = directory / tools.canonical_path(entry)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
        stage.rename(output)
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--panel-root", type=Path, required=True)
    parser.add_argument("--agent-version", required=True)
    parser.add_argument("--arch", choices=("amd64", "arm64"), default="amd64")
    parser.add_argument("--runtime-version", default="1.14.2")
    parser.add_argument("--minisign", default="minisign")
    parser.add_argument("--release-tools", type=Path, default=ROOT / "tools")
    args = parser.parse_args()
    tools = release_tools(args.release_tools)
    # The key is publicly disclosed test material. There is no production signing option.
    roots = tools.load_roots(FIXTURES / "public-keys.json")
    try:
        tools.load_roots(FIXTURES / "public-keys.json", publication=True)
    except ValueError:
        pass
    else:
        raise ValueError("official publication unexpectedly accepts the TEST_ONLY root")
    installer = args.bundle.with_name(args.bundle.name + "-install.sh")
    command([sys.executable, str(args.release_tools / "release.py"), "render-installer",
             "--template", str(ROOT / "deploy/install.sh.tmpl"),
             "--agent-unit", str(ROOT / "deploy/apsp-agent.service"),
             "--runtime-unit", str(ROOT / "plugins/sing-box/apsp-singbox@.service"),
             "--output", str(installer)])
    command([sys.executable, str(args.release_tools / "release.py"), "assemble",
             "--source", str(args.source), "--output", str(args.bundle),
             "--tag", "agent-v" + args.agent_version, "--agent-version", args.agent_version,
             "--runtime-version", args.runtime_version, "--installer", str(installer), "--arch", args.arch])
    command([args.minisign, "-S", "-q", "-m", str(args.bundle / "SHA256SUMS"),
             "-s", str(FIXTURES / "TEST_ONLY.key"), "-x", str(args.bundle / "SHA256SUMS.minisig"),
             "-t", "Apsp TEST ONLY CI fixture; never publish as an official release"])
    metadata = tools.verify_bundle(args.bundle, roots, args.minisign, "agent-v" + args.agent_version)
    panel_tree(args.bundle, args.panel_root, metadata, tools)
    print("Independently verified TEST_ONLY signed CI Release prepared; official publication is prohibited.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise SystemExit("CI signed Release refused: " + str(error)) from error
