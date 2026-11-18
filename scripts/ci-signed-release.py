#!/usr/bin/env python3
"""Sign a disposable TEST_ONLY CI integration fixture with the public test key."""

import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import runpy
import shutil
import uuid

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/protocol/tests/fixtures"


def release_tools(directory):
    specification = importlib.util.spec_from_file_location("apsp_ci_release_tools", Path(directory) / "release.py")
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


def fixture_tools(directory):
    return runpy.run_path(str(Path(directory) / "ci-release-fixture.py"))


def assemble_ci_fixture(source, bundle, agent_version, arch, tools, fixture, installer_bytes=None):
    """Assemble and sign a single-Agent TEST_ONLY CI fixture without claiming official 4-tool closure."""
    source, bundle = Path(source), Path(bundle)
    tools.ensure(tools.VERSION.fullmatch(agent_version), "invalid Agent version")
    tools.ensure(arch in ("amd64", "arm64"), "unsupported CI fixture architecture")
    tools.ensure(not bundle.exists() and not bundle.is_symlink(), "CI release bundle already exists")
    entry = {
        "name": "agent",
        "version": agent_version,
        "arch": arch,
        "format": "raw",
        "binary_name": "apsp-agent",
    }
    path = tools.canonical_path(entry)
    agent_bytes = tools.read_regular(source / path, tools.MAX_AGENT_RAW)
    manifest = tools.verify_embedded_agent_manifest(
        agent_bytes, agent_version, arch, require_release_closure=False
    )
    if installer_bytes is None:
        installer_bytes = tools.installer_source(
            ROOT / "deploy/install.sh.tmpl",
            ROOT / "deploy/apsp-agent.service",
            ROOT / "deploy/apsp-singbox@.service",
        ).encode("utf-8")
    tools.ensure(0 < len(installer_bytes) <= 262144, "CI installer size outside permitted range")
    tools.parse_paired_source_headers(installer_bytes)
    entry.update(
        archive_size=len(agent_bytes),
        binary_sha256=tools.digest(agent_bytes),
        binary_size=len(agent_bytes),
        asset_name=tools.asset_name(entry),
    )
    metadata = {
        "schema": 1,
        "source_repo": tools.REPOSITORY,
        "tag": "agent-v" + agent_version,
        "protocol_min": 1,
        "protocol_max": 1,
        "artifacts": [entry],
    }
    encoded = (json.dumps(metadata, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    tools.ensure(len(encoded) <= 32768, "metadata too large")
    rows = {
        path: entry["binary_sha256"],
        "release.json": tools.digest(encoded),
        "install.sh": tools.digest(installer_bytes),
    }
    checksums = "".join(f"{rows[name]}  {name}\n" for name in sorted(rows)).encode("utf-8")
    tools.ensure(len(checksums) <= 8192, "checksums too large")
    bundle.mkdir(parents=True)
    (bundle / entry["asset_name"]).write_bytes(agent_bytes)
    (bundle / "release.json").write_bytes(encoded)
    (bundle / "install.sh").write_bytes(installer_bytes)
    (bundle / "SHA256SUMS").write_bytes(checksums)
    (bundle / "SHA256SUMS.minisig").write_text(fixture["sign"](checksums), encoding="utf-8")
    return metadata, manifest


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
            limit = getattr(tools, "MAX_AGENT_RAW", tools.MAX_BINARY) if entry["name"] == "agent" else tools.MAX_BINARY
            data = tools.read_regular(bundle / entry["asset_name"], limit)
            if len(data) != entry["archive_size"] or hashlib.sha256(data).hexdigest() != rows[tools.canonical_path(entry)]:
                raise ValueError("signed archive size differs")
            binary = tools.binary_bytes(data, entry["format"], entry["binary_name"], entry.get("auxiliary_files", {}))
            if len(binary) != entry["binary_size"] or hashlib.sha256(binary).hexdigest() != entry["binary_sha256"]:
                raise ValueError("signed binary differs")
            destination = directory / tools.canonical_path(entry)
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(data)
        stage.rename(output)
    finally:
        if stage.exists():
            shutil.rmtree(stage)


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--panel-root", type=Path, required=True)
    parser.add_argument("--agent-version", required=True)
    parser.add_argument("--arch", choices=("amd64", "arm64"), default="amd64")
    parser.add_argument("--minisign", default="minisign")
    parser.add_argument("--release-tools", type=Path, default=ROOT / "tools")
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    tools = release_tools(args.release_tools)
    fixture = fixture_tools(args.release_tools)
    # The key is publicly disclosed test material. There is no production signing option.
    roots = tools.load_roots(FIXTURES / "public-keys.json")
    try:
        tools.load_roots(FIXTURES / "public-keys.json", publication=True)
    except ValueError:
        pass
    else:
        raise ValueError("official publication unexpectedly accepts the TEST_ONLY root")
    metadata, manifest = assemble_ci_fixture(
        args.source, args.bundle, args.agent_version, args.arch, tools, fixture
    )
    confirmed, _ = tools.verify_manifest(args.bundle, roots, args.minisign, metadata["tag"])
    tools.ensure(confirmed == metadata, "verified CI fixture metadata differs")
    expected_files = {
        "release.json",
        "install.sh",
        "SHA256SUMS",
        "SHA256SUMS.minisig",
        metadata["artifacts"][0]["asset_name"],
    }
    tools.ensure({path.name for path in args.bundle.iterdir()} == expected_files, "unexpected CI fixture bundle files")
    components = sorted(manifest["components"])
    if set(components) != set(tools.LINUX_REQUIRED_EMBEDDED_COMPONENTS):
        try:
            tools.verify_bundle(args.bundle, roots, args.minisign, metadata["tag"])
        except ValueError:
            pass
        else:
            raise ValueError("incomplete CI fixture unexpectedly passed official full-release verification")
    panel_tree(args.bundle, args.panel_root, confirmed, tools)
    print(
        "Independently verified TEST_ONLY signed CI fixture prepared "
        f"(embedded={','.join(components)}); official publication is prohibited."
    )


if __name__ == "__main__":
    try:
        main()
    except (ValueError, RuntimeError, OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise SystemExit("CI signed Release refused: " + str(error)) from error
