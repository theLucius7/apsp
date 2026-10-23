#!/usr/bin/env python3
"""Check CI wiring and failure contracts; Linux CI runs real minisign and systemd."""

import argparse
import gzip
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def load(name, filename):
    specification = importlib.util.spec_from_file_location(name, ROOT / "scripts" / filename)
    module = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(module)
    return module


TRUST = load("ci_test_trust", "ci-test-trust.py")
RELEASE = load("ci_signed_release", "ci-signed-release.py")
BOOTSTRAP = load("ci_bootstrap_install_contract", "ci-bootstrap-install.py")
PREFLIGHT = load("ci_signed_preflight", "ci-signed-preflight.py")
TOOLS = RELEASE.release_tools(Path(os.environ.get("APSP_CI_RELEASE_TOOLS", ROOT / "tools")))


def packed(name, contents):
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        member = tarfile.TarInfo(name)
        member.size = len(contents)
        member.mode = 0o755
        archive.addfile(member, io.BytesIO(contents))
    return gzip.compress(raw.getvalue(), mtime=0)


class SignedCiContracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)

    def bundle(self):
        source = self.directory / "source"
        for name, version, data in (("agent", "0.3.0", b"explicitly non-executable CI structural fixture"),
                                    ("sing-box", "1.14.2", packed("sing-box", b"runtime fixture")),
                                    ("nodequality", TOOLS.NODEQUALITY_VERSION, packed("nodequality", b"diagnostic fixture"))):
            target = source / name / version / "amd64"
            target.parent.mkdir(parents=True)
            target.write_bytes(data)
        installer = self.directory / "install.sh"
        installer.write_bytes(b"#!/bin/sh\nexit 1\n")
        bundle = self.directory / "bundle"
        TOOLS.assemble(argparse.Namespace(source=source, output=bundle, tag="agent-v0.3.0", agent_version="0.3.0",
            runtime_version="1.14.2", nodequality_version=TOOLS.NODEQUALITY_VERSION, installer=installer, arch=["amd64"]))
        # This fixture deliberately exercises shape and hash guards only, not crypto.
        (bundle / "SHA256SUMS.minisig").write_bytes(b"invalid signature; structural test only\n")
        metadata, _ = TOOLS.validate_manifest(bundle, "agent-v0.3.0")
        return bundle, metadata

    def descriptor(self):
        path = self.directory / "enrollment.json"
        path.write_text(json.dumps({"version":"0.3.0", "tag":"agent-v0.3.0", "token":"public-test-token",
                                    "origin":"http://127.0.0.1:18080"}))
        path.chmod(0o600)
        return path

    def test_ci_build_roots_are_explicit_test_material_and_cannot_publish(self):
        roots = json.loads(TRUST.public_keys())
        self.assertEqual(roots, json.loads((TRUST.FIXTURES / "public-keys.json").read_text()))
        for path in TRUST.FIXTURES.glob("TEST_ONLY*.pub"):
            key_file = self.directory / (path.name + ".json")
            key_file.write_text(json.dumps([path.read_text()]))
            with self.assertRaises(ValueError):
                TOOLS.load_roots(key_file, publication=True)

    def test_panel_tree_maps_canonical_release_and_has_no_unsigned_fallback(self):
        bundle, metadata = self.bundle()
        output = self.directory / "artifacts"
        RELEASE.panel_tree(bundle, output, metadata, TOOLS)
        release = output / "releases/agent-v0.3.0"
        for entry in metadata["artifacts"]:
            self.assertEqual((release / TOOLS.canonical_path(entry)).read_bytes(), (bundle / entry["asset_name"]).read_bytes())
        self.assertFalse((output / "agent").exists())
        self.assertEqual((release / "SHA256SUMS").read_bytes(), (bundle / "SHA256SUMS").read_bytes())
        with self.assertRaises(ValueError):
            RELEASE.panel_tree(bundle, output, metadata, TOOLS)

    def test_changed_archive_wrapper_with_same_binary_is_rejected_without_partial_publish(self):
        bundle, metadata = self.bundle()
        runtime = next(entry for entry in metadata["artifacts"] if entry["name"] == "sing-box")
        asset = bundle / runtime["asset_name"]
        changed = bytearray(asset.read_bytes())
        changed[4] ^= 1  # Gzip timestamp differs; the decompressed binary stays identical.
        asset.write_bytes(changed)
        output = self.directory / "artifacts"
        with self.assertRaises(ValueError):
            RELEASE.panel_tree(bundle, output, metadata, TOOLS)
        self.assertFalse(output.exists())
        self.assertEqual(list(self.directory.glob(".ci-release-*")), [])

    def test_changed_metadata_is_rejected_before_any_panel_directory_is_published(self):
        bundle, metadata = self.bundle()
        with (bundle / "release.json").open("ab") as output:
            output.write(b" ")
        output = self.directory / "artifacts"
        with self.assertRaises(ValueError):
            RELEASE.panel_tree(bundle, output, metadata, TOOLS)
        self.assertFalse(output.exists())

    def test_bootstrap_uses_operator_provisioned_code_and_only_environment_token(self):
        descriptor = self.descriptor()
        trust = self.directory / "operator"
        with patch.object(BOOTSTRAP.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            self.assertEqual(BOOTSTRAP.bootstrap(descriptor, trust, self.directory / "release", self.directory / "log"), 0)
        arguments = run.call_args.args[0]
        self.assertEqual(arguments[1], str(trust / "bootstrap.py"))
        self.assertIn("--release-dir", arguments)
        self.assertIn(str(trust / "public-keys.json"), arguments)
        self.assertNotIn("--token", arguments)
        self.assertNotIn("public-test-token", " ".join(arguments))
        self.assertEqual(run.call_args.kwargs["env"]["APSP_ENROLLMENT_TOKEN"], "public-test-token")

    def test_bootstrap_refuses_public_or_cross_origin_private_descriptors(self):
        descriptor = self.descriptor()
        descriptor.chmod(0o644)
        with self.assertRaises(ValueError):
            BOOTSTRAP.bootstrap(descriptor, self.directory, self.directory, self.directory / "log")
        descriptor.chmod(0o600)
        value = json.loads(descriptor.read_text())
        value["origin"] = "https://panel.example.test"
        descriptor.write_text(json.dumps(value))
        with self.assertRaises(ValueError):
            BOOTSTRAP.bootstrap(descriptor, self.directory, self.directory, self.directory / "log")

    def test_atomic_replacement_preserves_mode_and_does_not_modify_open_original_inode(self):
        binary = self.directory / "runtime"
        binary.write_bytes(b"original")
        binary.chmod(0o755)
        with binary.open("rb") as running_inode:
            PREFLIGHT.atomic_replace(binary, b"tampered", 0o755)
            self.assertEqual(running_inode.read(), b"original")
        self.assertEqual(binary.read_bytes(), b"tampered")
        self.assertEqual(binary.stat().st_mode & 0o777, 0o755)
        self.assertEqual(list(self.directory.glob(".ci-proof-*")), [])

    def test_systemd_probe_binds_role_and_does_not_claim_failed_gate_executed_payload(self):
        with patch.object(PREFLIGHT.subprocess, "run", return_value=subprocess.CompletedProcess([], 1)) as run:
            PREFLIGHT.probe(Path("/opt/apsp/plugins/sing-box/1.14.2/sing-box"), self.directory, False)
        gate = next(argument for argument in run.call_args.args[0] if argument.startswith("--property=ExecStartPre="))
        self.assertIn("--name sing-box --format tar.gz", gate)
        self.assertIn("--binary /opt/apsp/plugins/sing-box/1.14.2/sing-box", gate)
        self.assertEqual(list(self.directory.glob("systemd-marker-*")), [])


if __name__ == "__main__":
    unittest.main()
