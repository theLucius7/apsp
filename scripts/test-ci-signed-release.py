#!/usr/bin/env python3
"""Check CI wiring and failure contracts; Linux CI runs real minisign and systemd."""

import argparse
import gzip
import hashlib
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

    def bundle(self, legacy=False):
        source = self.directory / ("legacy-source" if legacy else "source")
        agent_fixture = b"explicitly non-executable CI structural fixture"
        if not legacy:
            agent_fixture += TOOLS.encode_embedded_agent_manifest_frame(
                TOOLS.build_embedded_agent_manifest("0.4.0", "amd64")
            )
        items = [("agent", "0.4.0", agent_fixture)]
        if legacy:
            items.extend([
                ("sing-box", "1.14.4-apsp.1", packed("sing-box", b"runtime fixture")),
                ("nodequality", TOOLS.NODEQUALITY_VERSION, packed("nodequality", b"diagnostic fixture")),
            ])
        for name, version, data in items:
            target = source / name / version / "amd64"
            target.parent.mkdir(parents=True)
            target.write_bytes(data)
        installer = self.directory / ("legacy-install.sh" if legacy else "install.sh")
        installer.write_bytes(b"#!/bin/sh\nexit 1\n")
        bundle = self.directory / ("legacy-bundle" if legacy else "bundle")
        namespace = argparse.Namespace(
            source=source,
            output=bundle,
            tag="agent-v0.4.0",
            agent_version="0.4.0",
            runtime_version="1.14.4-apsp.1" if legacy else None,
            nodequality_version=TOOLS.NODEQUALITY_VERSION,
            installer=installer,
            arch=["amd64"],
        )
        TOOLS.assemble(namespace)
        # This fixture deliberately exercises shape and hash guards only, not crypto.
        (bundle / "SHA256SUMS.minisig").write_bytes(b"invalid signature; structural test only\n")
        metadata, _ = TOOLS.validate_manifest(bundle, "agent-v0.4.0")
        return bundle, metadata

    def descriptor(self):
        path = self.directory / "enrollment.json"
        path.write_text(json.dumps({"version": "0.4.0", "tag": "agent-v0.4.0", "token": "public-test-token",
                                    "origin": "http://127.0.0.1:18080"}))
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

    def test_self_contained_panel_tree_contains_only_raw_agent_and_has_no_unsigned_fallback(self):
        bundle, metadata = self.bundle(legacy=False)
        self.assertEqual([entry["name"] for entry in metadata["artifacts"]], ["agent"])
        self.assertEqual(metadata["artifacts"][0]["format"], "raw")
        output = self.directory / "artifacts"
        RELEASE.panel_tree(bundle, output, metadata, TOOLS)
        release = output / "releases/agent-v0.4.0"
        for entry in metadata["artifacts"]:
            self.assertEqual((release / TOOLS.canonical_path(entry)).read_bytes(), (bundle / entry["asset_name"]).read_bytes())
        self.assertFalse((output / "agent").exists())
        self.assertFalse((release / "sing-box").exists())
        self.assertEqual((release / "SHA256SUMS").read_bytes(), (bundle / "SHA256SUMS").read_bytes())
        with self.assertRaises(ValueError):
            RELEASE.panel_tree(bundle, output, metadata, TOOLS)

    def test_changed_raw_agent_or_legacy_archive_wrapper_is_rejected_without_partial_publish(self):
        bundle, metadata = self.bundle(legacy=False)
        agent_entry = metadata["artifacts"][0]
        asset = bundle / agent_entry["asset_name"]
        changed = bytearray(asset.read_bytes())
        changed[0] ^= 1
        asset.write_bytes(changed)
        output = self.directory / "artifacts"
        with self.assertRaises(ValueError):
            RELEASE.panel_tree(bundle, output, metadata, TOOLS)
        self.assertFalse(output.exists())
        self.assertEqual(list(self.directory.glob(".ci-release-*")), [])

        legacy_bundle, legacy_metadata = self.bundle(legacy=True)
        runtime = next(entry for entry in legacy_metadata["artifacts"] if entry["name"] == "sing-box")
        runtime_asset = legacy_bundle / runtime["asset_name"]
        changed_archive = bytearray(runtime_asset.read_bytes())
        changed_archive[4] ^= 1  # Gzip timestamp differs; the decompressed binary stays identical.
        runtime_asset.write_bytes(changed_archive)
        with self.assertRaises(ValueError):
            RELEASE.panel_tree(legacy_bundle, output, legacy_metadata, TOOLS)
        self.assertFalse(output.exists())
        self.assertEqual(list(self.directory.glob(".ci-release-*")), [])

    def test_changed_metadata_is_rejected_before_any_panel_directory_is_published(self):
        bundle, metadata = self.bundle(legacy=False)
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

    def test_latest_enrollment_installs_the_bundled_signed_tag(self):
        descriptor = self.descriptor()
        value = json.loads(descriptor.read_text())
        descriptor.write_text(json.dumps({**value, "version": "latest", "tag": None}))
        release = self.directory / "release"
        release.mkdir()
        (release / "release.json").write_text(json.dumps({"tag": "agent-v0.4.1"}))
        with patch.object(BOOTSTRAP.subprocess, "run", return_value=subprocess.CompletedProcess([], 0)) as run:
            self.assertEqual(BOOTSTRAP.bootstrap(descriptor, self.directory / "operator", release, self.directory / "log"), 0)
        arguments = run.call_args.args[0]
        self.assertEqual(arguments[arguments.index("--tag") + 1], "agent-v0.4.1")
        (release / "release.json").write_text(json.dumps({"tag": "agent-vlatest"}))
        with self.assertRaises(ValueError):
            BOOTSTRAP.bootstrap(descriptor, self.directory / "operator", release, self.directory / "log")

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

    def test_bundled_manifest_tampering_forges_primary_binary_sha256_without_breaking_json_shape(self):
        digest = hashlib.sha256(b"runtime-binary").hexdigest()
        manifest = {
            "schema": 1,
            "artifact_name": "sing-box",
            "binary_name": "sing-box",
            "version": "1.14.4-apsp.2",
            "bundle_sha256": hashlib.sha256(b"archive").hexdigest(),
            "binary_sha256": digest,
            "files": {"sing-box": {"sha256": digest, "size": 14, "executable": True}},
        }
        tampered = json.loads(PREFLIGHT.tampered_bundled_manifest(json.dumps(manifest).encode("utf-8")))
        self.assertNotEqual(tampered["binary_sha256"], digest)
        self.assertEqual(tampered["binary_sha256"], tampered["files"]["sing-box"]["sha256"])
        self.assertEqual(len(tampered["binary_sha256"]), 64)

    def test_systemd_probe_binds_role_and_does_not_claim_failed_gate_executed_payload(self):
        with patch.object(PREFLIGHT.subprocess, "run", return_value=subprocess.CompletedProcess([], 1)) as run:
            PREFLIGHT.probe(Path("/opt/apsp/plugins/sing-box/1.14.4-apsp.2/sing-box"), self.directory, False)
        gate = next(argument for argument in run.call_args.args[0] if argument.startswith("--property=ExecStartPre="))
        self.assertIn("--name sing-box --format tar.gz", gate)
        self.assertIn("--binary /opt/apsp/plugins/sing-box/1.14.4-apsp.2/sing-box", gate)
        self.assertEqual(list(self.directory.glob("systemd-marker-*")), [])

    def test_ci_fixture_signs_subset_agent_while_official_release_assembly_refuses_it(self):
        source = self.directory / "subset-source"
        subset_agent = b"TEST_ONLY subset Agent fixture" + TOOLS.encode_embedded_agent_manifest_frame(
            TOOLS.build_embedded_agent_manifest("0.4.0", "amd64", omit_component="ipquality")
        )
        target = source / "agent/0.4.0/amd64"
        target.parent.mkdir(parents=True)
        target.write_bytes(subset_agent)
        installer = self.directory / "subset-install.sh"
        installer.write_bytes(b"#!/bin/sh\nexit 1\n")

        with self.assertRaises(ValueError):
            TOOLS.assemble(argparse.Namespace(
                source=source,
                output=self.directory / "official-rejected",
                tag="agent-v0.4.0",
                agent_version="0.4.0",
                runtime_version=None,
                installer=installer,
                arch=["amd64"],
            ))

        ci_bundle = self.directory / "ci-fixture-bundle"
        metadata, manifest = RELEASE.assemble_ci_fixture(
            source,
            ci_bundle,
            "0.4.0",
            "amd64",
            TOOLS,
            {"sign": lambda _: "invalid signature; structural test only\n"},
            installer_bytes=b"#!/bin/sh\nexit 1\n",
        )
        self.assertEqual(sorted(manifest["components"]), ["network-workbench", "nodequality", "sing-box", "tcpquality"])
        confirmed, _ = TOOLS.validate_manifest(ci_bundle, "agent-v0.4.0")
        self.assertEqual(confirmed, metadata)
        with patch.object(TOOLS, "verify_signature"):
            with self.assertRaises(ValueError):
                TOOLS.verify_bundle(ci_bundle, ["dummy-root"], "minisign", "agent-v0.4.0")
        RELEASE.panel_tree(ci_bundle, self.directory / "ci-panel-artifacts", confirmed, TOOLS)

        no_singbox_source = self.directory / "no-singbox-source"
        no_singbox_target = no_singbox_source / "agent/0.4.0/amd64"
        no_singbox_target.parent.mkdir(parents=True)
        no_singbox_target.write_bytes(
            b"missing sing-box"
            + TOOLS.encode_embedded_agent_manifest_frame(
                TOOLS.build_embedded_agent_manifest("0.4.0", "amd64", omit_component="sing-box")
            )
        )
        with self.assertRaises(ValueError):
            RELEASE.assemble_ci_fixture(
                no_singbox_source,
                self.directory / "no-singbox-bundle",
                "0.4.0",
                "amd64",
                TOOLS,
                {"sign": lambda _: "invalid signature; structural test only\n"},
                installer_bytes=b"#!/bin/sh\nexit 1\n",
            )

    def test_ci_signed_release_cli_rejects_removed_runtime_version_flag(self):
        with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
            RELEASE.build_parser().parse_args([
                "--source", str(self.directory),
                "--bundle", str(self.directory / "b"),
                "--panel-root", str(self.directory / "p"),
                "--agent-version", "0.4.0",
                "--runtime-version", "1.14.4-apsp.2",
            ])


if __name__ == "__main__":
    unittest.main()
