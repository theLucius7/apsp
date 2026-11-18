#!/usr/bin/env python3
"""Real minisign interoperability, self-contained assembly, and historical 0.3.x read-verification tests."""

import argparse
import base64
import contextlib
import gzip
import http.server
import io
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import threading
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/protocol/tests/fixtures"
if not FIXTURES.is_dir():
    FIXTURES = ROOT / "fixtures"
sys.path.insert(0, str(ROOT / "tools"))
import release


def build_embedded_agent_manifest(agent_version, target, *, commit="a" * 40, abi_override=None,
                                  singbox_sha=None, omit_component=None, component_versions=None,
                                  extra_components=None):
    target_os, target_arch, target_env = abi_override or release.EXPECTED_EMBEDDED_ABI[target]
    versions = {
        "sing-box": release.SINGBOX_VERSION,
        "nodequality": release.NODEQUALITY_VERSION,
        "ipquality": release.IPQUALITY_DEFAULT_VERSION,
        "network-workbench": release.WORKBENCH_VERSION,
    }
    if component_versions:
        versions.update(component_versions)
    raw = {}
    if omit_component != "sing-box":
        raw["sing-box"] = {
            "format": "tar.gz",
            "sha256": singbox_sha or release.digest(
                f"sing-box:{target_os}:{target_arch}:{target_env}:{versions['sing-box']}".encode()
            ),
            "version": versions["sing-box"],
        }
    if target_os == "linux":
        for name in ("nodequality", "ipquality", "network-workbench"):
            if name != omit_component:
                raw[name] = {
                    "format": "tar.gz",
                    "sha256": release.digest(f"{name}:{target_arch}:{versions[name]}".encode()),
                    "version": versions[name],
                }
        if omit_component != "tcpquality":
            tcp_ver = versions.get(
                "tcpquality",
                f"{release.TCPQUALITY_ENGINE_VERSION}-{commit}-r1" if commit else release.TCPQUALITY_ENGINE_VERSION,
            )
            raw["tcpquality"] = {"format": "agent_subcommand", "subcommand": "tcp-probe", "version": tcp_ver}
    if extra_components:
        raw.update(extra_components)
    return {
        "schema": 1,
        "agent_version": agent_version,
        "target_os": target_os,
        "target_arch": target_arch,
        "target_env": target_env,
        "components": {k: raw[k] for k in sorted(raw)},
    }


def encode_embedded_agent_manifest_frame(manifest):
    body = json.dumps(manifest, separators=(",", ":")).encode("utf-8")
    return (release.EMBEDDED_FRAME_PREFIX + f"{len(body):08x}:{release.digest(body)}\n".encode("ascii")
            + body + release.EMBEDDED_FRAME_SUFFIX)


def stage_historical_0_3_bundle(source, output, installer, *, agent_version="0.3.0",
                                runtime_version="1.14.0-apsp.1",
                                nodequality_version=release.NODEQUALITY_VERSION,
                                architectures=("amd64", "arm64"),
                                include_musl_runtime=False, tcp_probe_version=None):
    """Test-only constructor for historical 0.3.x multi-artifact bundles to exercise read-only verifiers."""
    source, output, installer = Path(source), Path(output), Path(installer)
    output.mkdir(parents=True)
    modules = [
        ("agent", agent_version, "raw", "apsp-agent"),
        ("sing-box", runtime_version, "tar.gz", "sing-box"),
        ("nodequality", nodequality_version, "tar.gz", "nodequality"),
    ]
    if tcp_probe_version is not None:
        modules.append(("tcpquality", tcp_probe_version, "tar.gz", "apsp-tcp-probe"))
    paths, artifacts = {}, []
    for name, version, archive_format, binary_name in modules:
        targets = list(architectures)
        if name == "sing-box" and include_musl_runtime:
            targets += ["linux-musl-" + arch for arch in architectures]
        for arch in targets:
            entry = {"name": name, "version": version, "arch": arch,
                     "format": archive_format, "binary_name": binary_name}
            path = release.canonical_path(entry)
            data = release.read_regular(source / path)
            auxiliary = {}
            if name == "tcpquality":
                import tcp_probe_artifact as tcp
                files = tcp.archive_files(data)
                tcp.validate_files(files, version, arch)
                auxiliary = {fn: {"sha256": release.digest(c), "size": len(c)}
                             for fn, c in files.items() if fn != binary_name}
                entry["auxiliary_files"] = auxiliary
            binary = release.binary_bytes(data, archive_format, binary_name, auxiliary)
            entry.update(archive_size=len(data), binary_sha256=release.digest(binary),
                         binary_size=len(binary), asset_name=release.asset_name(entry))
            (output / entry["asset_name"]).write_bytes(data)
            paths[path] = release.digest(data)
            artifacts.append(entry)
    artifacts.sort(key=release.canonical_path)
    metadata = {"schema": 1, "source_repo": release.REPOSITORY, "tag": "agent-v" + agent_version,
                "protocol_min": 1, "protocol_max": 1, "artifacts": artifacts}
    encoded = (json.dumps(metadata, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
    inst_bytes = release.read_regular(installer, 256 * 1024)
    (output / "release.json").write_bytes(encoded)
    (output / "install.sh").write_bytes(inst_bytes)
    paths.update({"release.json": release.digest(encoded), "install.sh": release.digest(inst_bytes)})
    (output / "SHA256SUMS").write_text("".join(f"{paths[p]}  {p}\n" for p in sorted(paths)), encoding="utf-8")
    return output


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="apsp-release-test-")
        self.directory = Path(self.temporary.name)
        self.source = self.directory / "artifacts"
        for arch in ("amd64", "arm64"):
            agent = self.source / "agent" / "0.3.0"
            agent.mkdir(parents=True, exist_ok=True)
            (agent / arch).write_bytes(b"TEST ONLY Agent " + arch.encode())
            for name, version in (("sing-box", "1.14.0-apsp.1"),
                                  ("nodequality", release.NODEQUALITY_VERSION)):
                runtime = self.source / name / version
                runtime.mkdir(parents=True, exist_ok=True)
                output = io.BytesIO()
                with tarfile.open(fileobj=output, mode="w") as archive:
                    data = b"TEST ONLY " + name.encode() + b" " + arch.encode()
                    member = tarfile.TarInfo(name)
                    member.size = len(data)
                    member.mode = 0o755
                    archive.addfile(member, io.BytesIO(data))
                (runtime / arch).write_bytes(gzip.compress(output.getvalue(), mtime=0))
        self.installer = self.directory / "installer.sh"
        self.installer.write_text("#!/bin/sh\nexit 0\n")
        self.bundle = self.directory / "release"
        stage_historical_0_3_bundle(self.source, self.bundle, self.installer)
        self.roots = release.load_roots(FIXTURES / "public-keys.json")
        self.sign()

    def tearDown(self):
        self.temporary.cleanup()

    def sign(self, bundle_dir=None):
        target = bundle_dir or self.bundle
        result = subprocess.run(["minisign", "-S", "-m", str(target / "SHA256SUMS"),
                                 "-s", str(FIXTURES / "TEST_ONLY.key"), "-x",
                                 str(target / "SHA256SUMS.minisig"), "-t",
                                 "Apsp TEST ONLY automated fixture"], capture_output=True)
        self.assertEqual(result.returncode, 0, "fixture signing failed")

    def verify(self):
        return release.verify_bundle(self.bundle, self.roots, "minisign", "agent-v0.3.0")

    def test_legacy_separate_runtime_assembly_arguments_are_rejected(self):
        for extra in ({"runtime_version": "1.14.4-apsp.2"},
                      {"include_musl_runtime": True},
                      {"tcp_probe_version": "0.3.0-" + "a" * 40 + "-r1"}):
            with self.subTest(extra=extra):
                args = argparse.Namespace(
                    source=str(self.source), output=str(self.directory / "rejected-legacy"),
                    agent_version="0.4.0", tag="agent-v0.4.0", installer=str(self.installer), **extra
                )
                with self.assertRaisesRegex(ValueError, "legacy separate-runtime assembly"):
                    release.assemble(args)
        for removed_flag in ("--runtime-version", "--include-musl-runtime", "--tcp-probe-version",
                             "--singbox-source-version", "--nodequality-version"):
            proc = subprocess.run(
                [sys.executable, str(ROOT / "tools/release.py"), "assemble", removed_flag, "1.0.0"],
                capture_output=True, text=True,
            )
            self.assertNotEqual(proc.returncode, 0)

    def test_musl_runtime_has_a_separate_signed_identity_and_tampering_is_rejected(self):
        runtime = self.source / "sing-box" / "1.14.0-apsp.1"
        for arch in ("amd64", "arm64"):
            (runtime / ("linux-musl-" + arch)).write_bytes((runtime / arch).read_bytes())
        self.bundle = stage_historical_0_3_bundle(
            self.source, self.directory / "release-musl", self.installer, include_musl_runtime=True
        )
        self.sign()
        metadata = self.verify()
        entries = [entry for entry in metadata["artifacts"] if entry["name"] == "sing-box"]
        self.assertEqual({entry["arch"] for entry in entries},
                         {"amd64", "arm64", "linux-musl-amd64", "linux-musl-arm64"})
        self.assertEqual(len(metadata["artifacts"]), 8)
        entry = next(entry for entry in entries if entry["arch"] == "linux-musl-amd64")
        (self.bundle / entry["asset_name"]).write_bytes(b"tampered musl runtime")
        with self.assertRaises(ValueError):
            self.verify()

    def rewrite_metadata(self, operation):
        path = self.bundle / "release.json"
        data = json.loads(path.read_text())
        operation(data)
        path.write_text(json.dumps(data, sort_keys=True, separators=(",", ":")) + "\n")
        manifest = self.bundle / "SHA256SUMS"
        lines = [release.digest(path.read_bytes()) + "  release.json" if line.endswith("  release.json")
                 else line for line in manifest.read_text().splitlines()]
        manifest.write_text("\n".join(lines) + "\n")
        self.sign()

    def signed_native_runtime(self, files=None):
        binary, auxiliary = b"TEST ONLY native runtime", b"TEST ONLY native DLL"
        files = files if files is not None else [("sing-box.exe", binary), ("wintun.dll", auxiliary)]
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w") as archive:
            for name, content in files:
                member = tarfile.TarInfo(name)
                member.size = len(content)
                member.mode = 0o755
                archive.addfile(member, io.BytesIO(content))
        data = gzip.compress(output.getvalue(), mtime=0)
        metadata_path = self.bundle / "release.json"
        metadata = json.loads(metadata_path.read_text())
        entry = next(item for item in metadata["artifacts"]
                     if item["name"] == "sing-box" and item["arch"] in ("amd64", "windows-amd64"))
        old_path, old_asset = release.canonical_path(entry), entry["asset_name"]
        entry.update(arch="windows-amd64", binary_name="sing-box.exe", archive_size=len(data),
                     binary_size=len(binary), binary_sha256=release.digest(binary),
                     auxiliary_files={"wintun.dll": {"size": len(auxiliary), "sha256": release.digest(auxiliary)}})
        entry["asset_name"] = release.asset_name(entry)
        (self.bundle / old_asset).unlink()
        (self.bundle / entry["asset_name"]).write_bytes(data)
        metadata_path.write_text(json.dumps(metadata, sort_keys=True, separators=(",", ":")) + "\n")
        sums = dict((path, value) for value, path in
                    (line.split("  ", 1) for line in (self.bundle / "SHA256SUMS").read_text().splitlines()))
        sums.pop(old_path)
        sums[release.canonical_path(entry)] = release.digest(data)
        sums["release.json"] = release.digest(metadata_path.read_bytes())
        (self.bundle / "SHA256SUMS").write_text("".join(f"{sums[path]}  {path}\n" for path in sorted(sums)))
        self.sign()
        return binary, auxiliary

    def test_signed_native_runtime_with_auxiliary_file(self):
        self.signed_native_runtime()
        metadata = self.verify()
        entry = next(item for item in metadata["artifacts"] if item["arch"] == "windows-amd64")
        self.assertIn("wintun.dll", entry["auxiliary_files"])

    def test_signed_native_runtime_rejects_wrong_missing_extra_duplicate_and_unsafe_files(self):
        binary, auxiliary = b"TEST ONLY native runtime", b"TEST ONLY native DLL"
        for files in [
            [("sing-box.exe", binary), ("wintun.dll", b"wrong DLL")],
            [("sing-box.exe", binary)],
            [("sing-box.exe", binary), ("wintun.dll", auxiliary), ("unsigned.dll", auxiliary)],
            [("sing-box.exe", binary), ("wintun.dll", auxiliary), ("wintun.dll", auxiliary)],
            [("sing-box.exe", binary), ("../wintun.dll", auxiliary)],
        ]:
            with self.subTest(files=[name for name, _ in files]):
                self.signed_native_runtime(files)
                with self.assertRaises(ValueError):
                    self.verify()

    def test_signed_auxiliary_metadata_rejects_bad_names_sizes_digests_and_field_sets(self):
        self.signed_native_runtime()
        valid = {"sha256": release.digest(b"TEST ONLY native DLL"), "size": len(b"TEST ONLY native DLL")}
        for auxiliary in [
            {"../wintun.dll": valid}, {"sing-box.exe": valid},
            {"release.json": valid}, {".artifact.json": valid},
            {"wintun.dll": dict(valid, size=0)}, {"wintun.dll": dict(valid, size=True)},
            {"wintun.dll": dict(valid, sha256="g" * 64)},
            {"wintun.dll": dict(valid, unsigned="field")},
            {f"extra-{number}.dll": valid for number in range(8)}, [],
        ]:
            with self.subTest(auxiliary=auxiliary):
                self.rewrite_metadata(lambda metadata: next(item for item in metadata["artifacts"]
                    if item["arch"] == "windows-amd64").update(auxiliary_files=auxiliary))
                with self.assertRaises(ValueError):
                    self.verify()

    def test_signed_runner_only_nodequality_cannot_claim_offline_auxiliary_inventory(self):
        def claim(metadata):
            entry = next(item for item in metadata["artifacts"] if item["name"] == "nodequality")
            entry["auxiliary_files"] = {
                "rootfs.tar.gz": {"sha256": "0" * 64, "size": 1},
                "rootfs-manifest.json": {"sha256": "1" * 64, "size": 1},
            }
        self.rewrite_metadata(claim)
        with self.assertRaisesRegex(ValueError, "runner-only NodeQuality identity"):
            release.verify_manifest(self.bundle, self.roots, "minisign", "agent-v0.3.0")

    def append_ipquality_fixture(self, arch="amd64", mutate_notice=None):
        """Signing alone does not authenticate the fixture's intentionally fake profile."""
        import ipquality_artifact
        files = {name: b"TEST ONLY unapproved IPQuality profile " + name.encode()
                 for name in ipquality_artifact.FILES}
        paired = b"TEST ONLY unapproved corresponding Debian and Apsp sources " + arch.encode()
        asset = f"ipquality-{ipquality_artifact.VERSION}-linux-{arch}-sources.tar.gz"
        notice = {"source_offer": {"asset": asset, "size": len(paired), "sha256": release.digest(paired)},
                  "notice": "TEST ONLY unapproved source closure", "license": "AGPL-3.0-only"}
        if mutate_notice is not None:
            mutate_notice(notice)
        files["THIRD_PARTY_NOTICES.txt"] = (b"Apsp IPQuality node self-query\n"
            + (json.dumps(notice, sort_keys=True, ensure_ascii=False, separators=(",", ":")) + "\n").encode())
        (self.bundle / asset).write_bytes(paired)
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w") as archive:
            for name in sorted(files):
                member = tarfile.TarInfo(name)
                member.size = len(files[name])
                member.mode = 0o755 if name == "ipquality" else 0o644
                archive.addfile(member, io.BytesIO(files[name]))
        data = gzip.compress(output.getvalue(), mtime=0)
        entry = {"name": "ipquality", "version": ipquality_artifact.VERSION,
                 "arch": arch, "format": "tar.gz", "binary_name": "ipquality",
                 "archive_size": len(data), "binary_size": len(files["ipquality"]),
                 "binary_sha256": release.digest(files["ipquality"]),
                 "auxiliary_files": {name: {"size": len(content), "sha256": release.digest(content)}
                                     for name, content in files.items() if name != "ipquality"}}
        entry["asset_name"] = release.asset_name(entry)
        (self.bundle / entry["asset_name"]).write_bytes(data)
        metadata_path = self.bundle / "release.json"
        metadata = json.loads(metadata_path.read_text())
        metadata["artifacts"].append(entry)
        metadata_path.write_text(json.dumps(metadata, sort_keys=True, separators=(",", ":")) + "\n")
        sums = dict((path, value) for value, path in
                    (line.split("  ", 1) for line in (self.bundle / "SHA256SUMS").read_text().splitlines()))
        sums[release.canonical_path(entry)] = release.digest(data)
        sums["release.json"] = release.digest(metadata_path.read_bytes())
        (self.bundle / "SHA256SUMS").write_text("".join(f"{sums[path]}  {path}\n" for path in sorted(sums)))
        self.sign()
        return files

    def test_signed_ipquality_manifest_requires_exact_offline_inventory_and_fixed_identity(self):
        self.append_ipquality_fixture()
        release.verify_manifest(self.bundle, self.roots, "minisign", "agent-v0.3.0")
        for mutation in (
            lambda entry: entry.update(version="main"),
            lambda entry: entry.update(binary_name="nodequality"),
            lambda entry: entry.update(arch="windows-amd64"),
            lambda entry: entry.update(auxiliary_files={}),
            lambda entry: entry["auxiliary_files"].pop("source.tar.gz"),
            lambda entry: entry["auxiliary_files"].update({"unexpected.json": {"size": 1, "sha256": "1" * 64}}),
        ):
            original = json.loads((self.bundle / "release.json").read_text())
            self.rewrite_metadata(lambda metadata: mutation(next(item for item in metadata["artifacts"] if item["name"] == "ipquality")))
            with self.assertRaises(ValueError):
                release.verify_manifest(self.bundle, self.roots, "minisign", "agent-v0.3.0")
            self.rewrite_metadata(lambda metadata: metadata.update(original))

    def test_signed_ipquality_bundle_still_requires_source_license_and_rootfs_profile_validation(self):
        import ipquality_artifact
        files = self.append_ipquality_fixture()
        with patch.object(ipquality_artifact, "validate_files", side_effect=ValueError("unapproved IPQuality closure")) as validate:
            with self.assertRaisesRegex(ValueError, "unapproved IPQuality closure"):
                self.verify()
        validate.assert_called_once_with(files, ipquality_artifact.VERSION, "amd64")

    def test_signed_ipquality_source_offer_requires_the_exact_asset_hash_and_full_inventory_check(self):
        import ipquality_artifact
        files = self.append_ipquality_fixture()
        offer = ipquality_artifact.source_offer(files, ipquality_artifact.VERSION, "amd64")
        paired = self.bundle / offer["asset"]
        original = paired.read_bytes()
        with patch.object(ipquality_artifact, "validate_files"), \
                patch.object(ipquality_artifact, "validate_source_offer") as validate:
            self.verify()
            validate.assert_called_once_with(paired, files, ipquality_artifact.VERSION, "amd64")
            validate.reset_mock()
            paired.write_bytes(original + b"changed source")
            with self.assertRaisesRegex(ValueError, "signed declaration"):
                self.verify()
            validate.assert_not_called()
            paired.unlink()
            with self.assertRaisesRegex(ValueError, "ordinary file"):
                self.verify()
            validate.assert_not_called()
            paired.write_bytes(original)
            (self.bundle / "unlisted-sources.tar.gz").write_bytes(b"unsigned source offer")
            with self.assertRaisesRegex(ValueError, "extra release assets"):
                self.verify()
            (self.bundle / "unlisted-sources.tar.gz").unlink()
        with patch.object(ipquality_artifact, "validate_files"), \
                patch.object(ipquality_artifact, "validate_source_offer", side_effect=ValueError("incomplete Debian source inventory")):
            with self.assertRaisesRegex(ValueError, "incomplete Debian source inventory"):
                self.verify()

    def test_signed_ipquality_source_offer_cannot_change_architecture_version_or_path(self):
        import ipquality_artifact
        for name in ("../untrusted.tar.gz", "ipquality-main-linux-amd64-sources.tar.gz",
                     f"ipquality-{ipquality_artifact.VERSION}-linux-arm64-sources.tar.gz"):
            with self.subTest(name=name):
                files = self.append_ipquality_fixture(mutate_notice=lambda notice: notice["source_offer"].update(asset=name))
                with self.assertRaises(ValueError):
                    ipquality_artifact.source_offer(files, ipquality_artifact.VERSION, "amd64")
                self.rewrite_metadata(lambda metadata: metadata.update(artifacts=[
                    entry for entry in metadata["artifacts"] if entry["name"] != "ipquality"]))
                sums = self.bundle / "SHA256SUMS"
                sums.write_text("".join(line for line in sums.read_text().splitlines(keepends=True)
                                        if "  ipquality/" not in line))
                self.sign()

    def test_ipquality_paired_source_validation_during_self_contained_assembly(self):
        import ipquality_artifact
        files = self.append_ipquality_fixture()
        offer = ipquality_artifact.source_offer(files, ipquality_artifact.VERSION, "amd64")
        original = (self.bundle / offer["asset"]).read_bytes()
        entry = next(item for item in json.loads((self.bundle / "release.json").read_text())["artifacts"]
                     if item["name"] == "ipquality")
        src, installer = self.write_self_contained_source("sc-ipq-source", targets=("amd64",))
        ipq_dir = src / "ipquality" / ipquality_artifact.VERSION
        ipq_dir.mkdir(parents=True, exist_ok=True)
        (ipq_dir / "amd64").write_bytes((self.bundle / entry["asset_name"]).read_bytes())
        paired = ipq_dir / "amd64.sources.tar.gz"
        paired.write_bytes(original)
        with patch.object(ipquality_artifact, "validate_files"), \
                patch.object(ipquality_artifact, "validate_source_offer") as validate:
            out = self.assemble_self_contained(
                "paired-ipquality-release", src, installer, targets=("amd64",), require_paired=True
            )
            validate.assert_called_once_with(paired, files, ipquality_artifact.VERSION, "amd64")
            self.assertEqual((out / offer["asset"]).read_bytes(), original)
            self.assertNotIn(offer["asset"], (out / "SHA256SUMS").read_text())
            paired.write_bytes(b"different source offer")
            with self.assertRaisesRegex(ValueError, "signed declaration"):
                self.assemble_self_contained(
                    "changed-ipquality-release", src, installer, targets=("amd64",), require_paired=True
                )
            self.assertFalse((self.directory / "changed-ipquality-release").exists())
        paired.write_bytes(original)
        with patch.object(ipquality_artifact, "archive_files", return_value={"fixture": b"unapproved"}), \
                patch.object(ipquality_artifact, "validate_files", side_effect=ValueError("unapproved IPQuality closure")):
            with self.assertRaisesRegex(ValueError, "unapproved IPQuality closure"):
                self.assemble_self_contained(
                    "unapproved-ipquality-release", src, installer, targets=("amd64",), require_paired=True
                )
            self.assertFalse((self.directory / "unapproved-ipquality-release").exists())

    def test_paired_source_assets_are_streamed_and_reject_unsafe_or_oversized_files(self):
        paired = self.directory / "paired.tar.gz"
        content = b"TEST ONLY streamed source" * 1024
        paired.write_bytes(content)
        with patch.object(Path, "read_bytes", side_effect=AssertionError("source asset must be streamed")):
            self.assertEqual(release.regular_file_proof(paired),
                             {"size": len(content), "sha256": release.digest(content)})
        with self.assertRaises(ValueError):
            release.regular_file_proof(paired, len(content) - 1)
        linked = self.directory / "source-link.tar.gz"
        linked.symlink_to(paired)
        with self.assertRaises(ValueError):
            release.regular_file_proof(linked)
        linked.unlink()
        os.link(paired, linked)
        with self.assertRaises(ValueError):
            release.regular_file_proof(paired)

    def test_raw_artifact_cannot_claim_auxiliary_files(self):
        self.rewrite_metadata(lambda metadata: metadata["artifacts"][0].update(
            auxiliary_files={"extra.dll": {"sha256": "0" * 64, "size": 1}}))
        with self.assertRaises(ValueError):
            self.verify()

    def test_auxiliary_file_cannot_be_a_symbolic_link(self):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w") as archive:
            binary = tarfile.TarInfo("sing-box.exe")
            binary.size = 1
            archive.addfile(binary, io.BytesIO(b"x"))
            auxiliary = tarfile.TarInfo("wintun.dll")
            auxiliary.type = tarfile.SYMTYPE
            auxiliary.linkname = "../../untrusted.dll"
            archive.addfile(auxiliary)
        with self.assertRaises(ValueError):
            release.binary_bytes(gzip.compress(output.getvalue()), "tar.gz", "sing-box.exe",
                                 {"wintun.dll": {"sha256": release.digest(b"x"), "size": 1}})

    def test_concatenated_gzip_cannot_hide_nonzero_tail(self):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w") as archive:
            member = tarfile.TarInfo("sing-box")
            member.size = 1
            archive.addfile(member, io.BytesIO(b"x"))
        payload = gzip.compress(output.getvalue()) + gzip.compress(b"unlisted trailing payload")
        with self.assertRaisesRegex(ValueError, "trailing data"):
            release.binary_bytes(payload, "tar.gz", "sing-box")

    def test_raw_extension_headers_and_total_unpacked_overflow_are_rejected(self):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w", pax_headers={"comment": "unlisted metadata"}) as archive:
            member = tarfile.TarInfo("sing-box")
            member.size = 1
            archive.addfile(member, io.BytesIO(b"x"))
        with self.assertRaisesRegex(ValueError, "unsafe archive member"):
            release.binary_bytes(gzip.compress(output.getvalue()), "tar.gz", "sing-box")
        self.signed_native_runtime()
        entry = next(item for item in json.loads((self.bundle / "release.json").read_text())["artifacts"]
                     if item["arch"] == "windows-amd64")
        with patch.object(release, "MAX_BINARY", 4096):
            with self.assertRaisesRegex(ValueError, "unpacked size limit"):
                release.binary_bytes((self.bundle / entry["asset_name"]).read_bytes(), "tar.gz",
                                     entry["binary_name"], entry["auxiliary_files"])

    def test_legacy_archive_still_rejects_additional_unsigned_file(self):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w") as archive:
            for name in ("sing-box", "unsigned.dll"):
                member = tarfile.TarInfo(name)
                member.size = 1
                archive.addfile(member, io.BytesIO(b"x"))
        with self.assertRaises(ValueError):
            release.binary_bytes(gzip.compress(output.getvalue()), "tar.gz", "sing-box")

    def test_valid_complete_signature_and_every_asset(self):
        self.assertEqual(len(self.verify()["artifacts"]), 6)

    def test_single_architecture_ci_bundle_and_deterministic_manifests(self):
        src, installer = self.write_self_contained_source("sc-single", targets=("amd64",))
        first = self.assemble_self_contained("single-1", src, installer, targets=("amd64",))
        second = self.assemble_self_contained("single-2", src, installer, targets=("amd64",))
        data = json.loads((first / "release.json").read_text(encoding="utf-8"))
        self.assertEqual([(e["name"], e["arch"]) for e in data["artifacts"]], [("agent", "amd64")])
        for name in ("release.json", "SHA256SUMS", "install.sh"):
            self.assertEqual((first / name).read_bytes(), (second / name).read_bytes())

    def test_tampered_signed_checksum_file(self):
        with (self.bundle / "SHA256SUMS").open("a") as file:
            file.write("0" * 64 + "  extra\n")
        with self.assertRaises(ValueError):
            self.verify()

    def test_tampered_trusted_comment(self):
        path = self.bundle / "SHA256SUMS.minisig"
        lines = path.read_text().splitlines()
        lines[2] += " modified"
        path.write_text("\n".join(lines) + "\n")
        with self.assertRaises(ValueError):
            self.verify()

    def test_truncated_signature(self):
        path = self.bundle / "SHA256SUMS.minisig"
        path.write_text("\n".join(path.read_text().splitlines()[:3]) + "\n")
        with self.assertRaises(ValueError):
            self.verify()

    def test_legacy_signature_refused(self):
        path = self.bundle / "SHA256SUMS.minisig"
        lines = path.read_text().splitlines()
        data = base64.b64decode(lines[1])
        lines[1] = base64.b64encode(b"Ed" + data[2:]).decode()
        path.write_text("\n".join(lines) + "\n")
        with self.assertRaises(ValueError):
            self.verify()

    def test_modified_archive(self):
        path = self.bundle / "sing-box-1.14.0-apsp.1-linux-amd64.tar.gz"
        path.write_bytes(path.read_bytes() + b"modified")
        with self.assertRaises(ValueError):
            self.verify()

    def test_signed_wrong_binary_digest(self):
        self.rewrite_metadata(lambda data: data["artifacts"][0].update(binary_sha256="0" * 64))
        with self.assertRaises(ValueError):
            self.verify()

    def test_signed_wrong_asset_name(self):
        self.rewrite_metadata(lambda data: data["artifacts"][0].update(asset_name="../escape"))
        with self.assertRaises(ValueError):
            self.verify()

    def test_signed_wrong_protocol_range(self):
        self.rewrite_metadata(lambda data: data.update(protocol_min=2, protocol_max=2))
        with self.assertRaises(ValueError):
            self.verify()

    def test_signed_protocol_range_that_includes_this_client_is_compatible(self):
        self.rewrite_metadata(lambda data: data.update(protocol_max=2))
        self.assertEqual(self.verify()["protocol_max"], 2)

    def test_signed_unknown_metadata_field(self):
        self.rewrite_metadata(lambda data: data.update(public_keys=self.roots))
        with self.assertRaises(ValueError):
            self.verify()

    def test_extra_asset_refused(self):
        (self.bundle / "unsigned").write_text("unsigned")
        with self.assertRaises(ValueError):
            self.verify()

    def test_duplicate_checksum_refused_even_when_signed(self):
        path = self.bundle / "SHA256SUMS"
        text = path.read_text()
        path.write_text(text + text.splitlines()[0] + "\n")
        self.sign()
        with self.assertRaises(ValueError):
            self.verify()

    def test_test_key_cannot_publish(self):
        with self.assertRaises(ValueError):
            release.load_roots(FIXTURES / "public-keys.json", publication=True)

    def test_duplicate_trust_root_refused(self):
        path = self.directory / "duplicate-public-keys.json"
        path.write_text(json.dumps(self.roots * 2))
        with self.assertRaises(ValueError):
            release.load_roots(path)

    def test_test_key_cannot_publish_with_another_key_id(self):
        material = bytearray(base64.b64decode(self.roots[0]))
        material[2:10] = b"other-id"
        path = self.directory / "renamed-key.json"
        path.write_text(json.dumps([base64.b64encode(material).decode()]))
        with self.assertRaises(ValueError):
            release.load_roots(path, publication=True)

    def test_rotation_key_cannot_publish_with_another_key_id(self):
        material = bytearray(base64.b64decode(release.TEST_ONLY_ROTATION_PUBLIC_KEY))
        material[2:10] = b"other-id"
        path = self.directory / "renamed-rotation-key.json"
        path.write_text(json.dumps([base64.b64encode(material).decode()]))
        with self.assertRaises(ValueError):
            release.load_roots(path, publication=True)

    def test_every_test_public_fixture_is_denied(self):
        material = b"Ed" + b"test-id!" + b"\x42" * 32
        record = base64.b64encode(material).decode()
        fixtures = self.directory / "fixture-pubs"
        fixtures.mkdir()
        (fixtures / "TEST_ONLY_FUTURE.pub").write_text("untrusted comment: TEST ONLY\n" + record + "\n")
        path = self.directory / "new-fixture-key.json"
        path.write_text(json.dumps([record]))
        with patch.object(release, "TEST_PUBLIC_KEY_DIRS", (fixtures,)):
            with self.assertRaises(ValueError):
                release.load_roots(path, publication=True)

    def test_archive_path_traversal_refused(self):
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w") as archive:
            member = tarfile.TarInfo("../sing-box")
            member.size = 1
            archive.addfile(member, io.BytesIO(b"x"))
        with self.assertRaises(ValueError):
            release.binary_bytes(gzip.compress(output.getvalue()), "tar.gz", "sing-box")

    def signed_executable_fixture(self):
        self.installer_architecture = {"x86_64": "amd64", "amd64": "amd64",
                                       "aarch64": "arm64", "arm64": "arm64"}[platform.machine()]
        self.installer_asset = "agent-0.3.0-linux-musl-" + self.installer_architecture
        good_marker = self.directory / "trusted-agent-executed"
        bad_marker = self.directory / "untrusted-agent-executed"

        def executable(marker):
            return ("#!/bin/sh\n"
                    '[ "$#" = 7 ] && [ "$1" = verify-installed ] && [ "$2" = --binary ] && '
                    '[ "$3" = "$0" ] && [ "$4" = --name ] && [ "$5" = agent ] && '
                    '[ "$6" = --format ] && [ "$7" = raw ] || exit 93\n'
                    "printf verified > " + shlex.quote(str(marker)) + "\n").encode()

        good, bad = executable(good_marker), executable(bad_marker)
        size = max(len(good), len(bad)) + 64
        good, bad = good.ljust(size, b"\n"), bad.ljust(size, b"\n")
        (self.bundle / self.installer_asset).write_bytes(good)

        def update(metadata):
            entry = next(item for item in metadata["artifacts"]
                         if item["name"] == "agent" and item["arch"] == self.installer_architecture)
            entry.update(archive_size=size, binary_size=size, binary_sha256=release.digest(good))

        self.rewrite_metadata(update)
        manifest = self.bundle / "SHA256SUMS"
        proof_identity = "  agent/0.3.0/" + self.installer_architecture
        lines = [release.digest(good) + proof_identity if line.endswith(proof_identity) else line
                 for line in manifest.read_text().splitlines()]
        manifest.write_text("\n".join(lines) + "\n")
        self.sign()
        self.verify()
        return good, bad, good_marker, bad_marker

    @contextlib.contextmanager
    def panel_response(self, payload, length=True, redirect=False):
        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.server.requests += 1
                self.send_response(302 if redirect else 200)
                if redirect:
                    self.send_header("Location", "/never-follow")
                elif length:
                    self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                if not redirect:
                    try:
                        self.wfile.write(payload)
                    except (BrokenPipeError, ConnectionResetError):
                        pass

            def log_message(self, format, *arguments):
                pass

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        server.requests = 0
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield server, f"http://127.0.0.1:{server.server_port}"
        finally:
            server.shutdown()
            server.server_close()
            thread.join()

    def installer_prefix(self, panel, forced_optimization=False, payload=None, target=None, core_root=None):
        if payload is not None:
            (self.bundle / self.installer_asset).write_bytes(payload)
        text = release.installer_source(ROOT / "deploy/install.sh.tmpl", ROOT / "deploy/apsp-agent.service",
                                        ROOT / "deploy/apsp-singbox@.service", source_root=ROOT)
        text = text.split("# Reject unverifiable legacy caches", 1)[0] + "\nexit 0\n"
        systemd_marker = self.directory / "run-systemd-system"
        systemd_marker.mkdir(exist_ok=True)
        self.assertEqual(text.count("[ -d /run/systemd/system ]"), 1)
        text = text.replace("[ -d /run/systemd/system ]", "[ -d " + shlex.quote(str(systemd_marker)) + " ]")
        if core_root:
            text = text.replace("/opt/apsp/core", str(core_root))
        if forced_optimization:
            self.assertGreaterEqual(text.count("python3 -I - "), 2)
            text = text.replace("python3 -I - ", "python3 -I -O - ")
        script = self.directory / "verified-installer-prefix.sh"
        script.write_text(text)
        token = self.directory / "fixture-token"
        token.write_text("TEST_ONLY_token")
        commands = self.directory / "commands"
        commands.mkdir(exist_ok=True)
        for name in ("systemctl", "useradd"):
            path = commands / name
            path.write_text("#!/bin/sh\nexit 0\n")
            path.chmod(0o755)
        modules = self.directory / "untrusted-python-path"
        modules.mkdir(exist_ok=True)
        environment_marker = self.directory / "python-environment-injected"
        (modules / "json.py").write_text(
            "from pathlib import Path\nPath(" + repr(str(environment_marker)) + ").write_text('injected')\n")
        environment = dict(os.environ, PATH=str(commands) + ":" + os.environ["PATH"],
                           PYTHONOPTIMIZE="1", PYTHONPATH=str(modules),
                           HTTP_PROXY="http://127.0.0.1:9", HTTPS_PROXY="http://127.0.0.1:9",
                           ALL_PROXY="socks5://127.0.0.1:9", NO_PROXY="", no_proxy="")
        arguments = ["/bin/sh", str(script), "--bundle", str(self.bundle),
                     "--version", "0.3.0", "--panel", panel, "--token-file", str(token)]
        if target:
            arguments.extend(["--target", target])
        result = subprocess.run(arguments, env=environment, capture_output=True, timeout=35)
        self.assertFalse(environment_marker.exists(), "installer imported an untrusted Python module")
        return result

    @unittest.skipUnless(os.getuid() == 0, "real installer prefix requires isolated Linux container root")
    def test_optimized_installer_refuses_tampered_raw_without_execution(self):
        good, bad, good_marker, bad_marker = self.signed_executable_fixture()
        self.assertEqual(len(good), len(bad))
        for forced in (False, True):
            with self.subTest(forced_optimization=forced), self.panel_response(bad) as (server, panel):
                result = self.installer_prefix(panel, forced, bad)
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(server.requests, 0)
                self.assertIn("摘要不匹配".encode(), result.stderr)
                self.assertFalse(bad_marker.exists())
                self.assertFalse(good_marker.exists())

    @unittest.skipUnless(os.getuid() == 0, "real installer prefix requires isolated Linux container root")
    def test_installer_accepts_signed_raw_and_passes_role_binding(self):
        good, _, good_marker, bad_marker = self.signed_executable_fixture()
        with self.panel_response(good) as (server, panel):
            result = self.installer_prefix(panel, True, good)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertEqual(server.requests, 0)
            self.assertEqual(good_marker.read_text(), "verified")
            self.assertFalse(bad_marker.exists())

    def signed_abi_fixture(self, abi):
        good, _, marker, _ = self.signed_executable_fixture()
        old_arch, old_asset = self.installer_architecture, self.installer_asset
        target = "linux-" + abi + "-" + old_arch
        self.installer_asset = "agent-0.3.0-" + target
        (self.bundle / old_asset).rename(self.bundle / self.installer_asset)

        def update(metadata):
            entry = next(item for item in metadata["artifacts"]
                         if item["name"] == "agent" and item["arch"] == old_arch)
            entry.update(arch=target, asset_name=self.installer_asset)

        self.rewrite_metadata(update)
        manifest = self.bundle / "SHA256SUMS"
        rows = dict(line.split("  ")[::-1] for line in manifest.read_text().splitlines())
        rows["agent/0.3.0/" + target] = rows.pop("agent/0.3.0/" + old_arch)
        manifest.write_text("".join(f"{rows[path]}  {path}\n" for path in sorted(rows)))
        self.sign()
        self.verify()
        return good, marker, target

    @unittest.skipUnless(os.getuid() == 0, "real installer prefix requires isolated Linux container root")
    def test_installer_accepts_exact_signed_musl_identity(self):
        good, marker, target = self.signed_abi_fixture("musl")
        with self.panel_response(good) as (server, panel):
            result = self.installer_prefix(panel, payload=good, target=target)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertEqual(server.requests, 0)
            self.assertEqual(marker.read_text(), "verified")

    @unittest.skipUnless(os.getuid() == 0 and platform.libc_ver()[0] == "glibc",
                         "GNU Agent prefix requires root on a GNU/glibc host")
    def test_installer_accepts_exact_signed_gnu_identity(self):
        good, marker, target = self.signed_abi_fixture("gnu")
        with self.panel_response(good) as (server, panel):
            result = self.installer_prefix(panel, payload=good, target=target)
            self.assertEqual(result.returncode, 0, result.stderr.decode())
            self.assertEqual(server.requests, 0)
            self.assertEqual(marker.read_text(), "verified")

    @unittest.skipUnless(os.getuid() == 0, "protected executable staging test requires root")
    def test_installer_refuses_writable_execution_parent_before_agent_execution(self):
        good, _, marker, _ = self.signed_executable_fixture()
        with tempfile.TemporaryDirectory(prefix="apsp-installer-parent-", dir="/root") as directory:
            unsafe = Path(directory) / "unsafe-core"
            unsafe.mkdir(mode=0o777)
            unsafe.chmod(0o777)
            with self.panel_response(good) as (server, panel):
                result = self.installer_prefix(panel, payload=good, core_root=unsafe)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("root 保护".encode(), result.stderr)
            self.assertFalse(marker.exists())
            self.assertEqual(server.requests, 0)
            self.assertEqual(list(unsafe.iterdir()), [])

    @unittest.skipUnless(os.getuid() == 0, "real installer prefix requires isolated Linux container root")
    def test_installer_refuses_unbounded_body_at_signed_size(self):
        good, _, good_marker, bad_marker = self.signed_executable_fixture()
        with self.panel_response(good + b"x" * 1048576, length=False) as (server, panel):
            result = self.installer_prefix(panel, payload=good + b"x" * 1048576)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(server.requests, 0)
            self.assertIn("有界普通文件".encode(), result.stderr)
            self.assertFalse(good_marker.exists())
            self.assertFalse(bad_marker.exists())

    @unittest.skipUnless(os.getuid() == 0, "real installer prefix requires isolated Linux container root")
    def test_missing_preloaded_agent_never_falls_back_to_panel(self):
        good, _, good_marker, bad_marker = self.signed_executable_fixture()
        (self.bundle / self.installer_asset).unlink()
        with self.panel_response(good, redirect=True) as (server, panel):
            result = self.installer_prefix(panel)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(server.requests, 0)
            self.assertIn("有界普通文件".encode(), result.stderr)
            self.assertFalse(good_marker.exists())
            self.assertFalse(bad_marker.exists())

    def self_contained_agent_bytes(self, target="amd64", agent_version="0.4.0", **kwargs):
        manifest = build_embedded_agent_manifest(agent_version, target, **kwargs)
        return b"TEST ONLY self-contained apsp-agent " + target.encode() + encode_embedded_agent_manifest_frame(manifest)

    def write_self_contained_source(self, name="sc-source", targets=("amd64", "arm64"), agent_version="0.4.0"):
        src = self.directory / name
        agent_dir = src / "agent" / agent_version
        agent_dir.mkdir(parents=True, exist_ok=True)
        for target in targets:
            (agent_dir / target).write_bytes(self.self_contained_agent_bytes(target, agent_version))
        sb_dir = src / "sing-box" / release.SINGBOX_VERSION
        sb_dir.mkdir(parents=True, exist_ok=True)
        (sb_dir / f"sing-box-{release.SINGBOX_VERSION}-sources.tar.gz").write_bytes(b"TEST ONLY sing-box source")
        for arch in ("amd64", "arm64"):
            for comp, ver in (("nodequality", release.NODEQUALITY_VERSION),
                              ("network-workbench", release.WORKBENCH_VERSION),
                              ("ipquality", release.IPQUALITY_DEFAULT_VERSION)):
                comp_dir = src / comp / ver
                comp_dir.mkdir(parents=True, exist_ok=True)
                (comp_dir / f"{arch}.sources.tar.gz").write_bytes(f"TEST ONLY {comp} {arch} source".encode())
        installer = self.directory / f"{name}-install.sh"
        installer.write_text("#!/bin/sh\n# APSP_BOOTSTRAP_AGENT_SOURCE=preloaded-github-v1\nexit 0\n")
        return src, installer

    def assemble_self_contained(self, output_name, source, installer, targets=("amd64", "arm64"),
                                agent_version="0.4.0", require_paired=False):
        out = self.directory / output_name
        args = argparse.Namespace(
            source=str(source), output=str(out), agent_version=agent_version,
            tag="agent-v" + agent_version, installer=str(installer), target=list(targets),
            include_present_targets=False, paired_source=[],
            no_auto_paired_sources=not require_paired, require_paired_sources=require_paired,
        )
        release.assemble(args)
        return out

    def test_self_contained_release_assembles_and_verifies_real_embedded_manifest_closure(self):
        self.verify()
        targets = ("amd64", "arm64", "linux-gnu-amd64", "linux-gnu-arm64", "macos-arm64", "windows-amd64")
        src, installer = self.write_self_contained_source("sc-full", targets=targets)
        out = self.assemble_self_contained("sc-full-bundle", src, installer, targets=targets, require_paired=True)
        self.sign(out)
        metadata = release.verify_bundle(out, self.roots, "minisign", "agent-v0.4.0")
        self.assertEqual({entry["arch"] for entry in metadata["artifacts"]}, set(targets))
        self.assertTrue(all(entry["name"] == "agent" for entry in metadata["artifacts"]))
        self.assertEqual(len(release.bundle_source_offers(out, metadata)), 7)

    def test_self_contained_assemble_rejects_missing_corrupted_or_duplicate_frames(self):
        src, installer = self.write_self_contained_source("sc-frames", targets=("amd64",))
        valid_frame = encode_embedded_agent_manifest_frame(build_embedded_agent_manifest("0.4.0", "amd64"))
        bad_digest = valid_frame[:40] + (b"0" if valid_frame[40:41] != b"0" else b"1") + valid_frame[41:]
        cases = {
            "bare_dev_binary": b"TEST ONLY raw apsp-agent without embedded manifest frame",
            "truncated_header": b"elf" + release.EMBEDDED_FRAME_PREFIX + b"00000010",
            "bad_hex_length": b"elf" + release.EMBEDDED_FRAME_PREFIX + b"0000001G:" + b"a" * 64 + b"\n{}",
            "missing_suffix": b"elf" + valid_frame[:-5],
            "corrupted_digest": b"elf" + bad_digest,
            "duplicate_valid_frames": b"elf" + valid_frame + b"mid" + valid_frame,
            "valid_plus_corrupted_prefix": b"elf" + valid_frame + release.EMBEDDED_FRAME_PREFIX + b"broken",
        }
        for idx, (case_name, payload) in enumerate(cases.items()):
            with self.subTest(case=case_name):
                (src / "agent/0.4.0/amd64").write_bytes(payload)
                with self.assertRaises(ValueError):
                    self.assemble_self_contained(f"sc-bad-frame-{idx}", src, installer, targets=("amd64",))

    def test_self_contained_assemble_rejects_missing_tools_abi_mismatch_and_schema_anomalies(self):
        src, installer = self.write_self_contained_source("sc-schema", targets=("amd64", "macos-arm64"))
        for missing in ("sing-box", "nodequality", "ipquality", "network-workbench", "tcpquality"):
            with self.subTest(missing=missing):
                (src / "agent/0.4.0/amd64").write_bytes(
                    self.self_contained_agent_bytes("amd64", omit_component=missing)
                )
                with self.assertRaisesRegex(ValueError, missing):
                    self.assemble_self_contained(f"sc-missing-{missing}", src, installer, targets=("amd64",))

        dev_only = build_embedded_agent_manifest("0.4.0", "amd64")
        dev_only["components"] = {k: dev_only["components"][k] for k in ("sing-box", "tcpquality")}
        (src / "agent/0.4.0/amd64").write_bytes(b"dev" + encode_embedded_agent_manifest_frame(dev_only))
        with self.assertRaisesRegex(ValueError, "lacks verified embedded"):
            self.assemble_self_contained("sc-dev-partial", src, installer, targets=("amd64",))

        dup_sha = build_embedded_agent_manifest("0.4.0", "amd64")
        dup_sha["components"]["ipquality"]["sha256"] = dup_sha["components"]["sing-box"]["sha256"]
        bad_record = build_embedded_agent_manifest("0.4.0", "amd64")
        bad_record["components"]["sing-box"]["extra"] = "unexpected"
        bad_manifests = [
            ("wrong_agent_version", build_embedded_agent_manifest("0.3.9", "amd64")),
            ("wrong_abi_env", build_embedded_agent_manifest("0.4.0", "amd64", abi_override=("linux", "x86_64", "gnu"))),
            ("wrong_abi_arch", build_embedded_agent_manifest("0.4.0", "amd64", abi_override=("linux", "aarch64", "musl"))),
            ("historical_nq_rootfs", build_embedded_agent_manifest(
                "0.4.0", "amd64", component_versions={"nodequality": "a92fca6c0067df29ddd03fdc2fee6f3000f64545-offline-rootfs-r1"})),
            ("wrong_ipq_version", build_embedded_agent_manifest("0.4.0", "amd64", component_versions={"ipquality": "1.0.0-wrong"})),
            ("unpinned_tcp_probe", build_embedded_agent_manifest("0.4.0", "amd64", commit="")),
            ("bool_schema", dict(build_embedded_agent_manifest("0.4.0", "amd64"), schema=True)),
            ("unknown_top_field", dict(build_embedded_agent_manifest("0.4.0", "amd64"), extra=1)),
            ("unknown_linux_tool", build_embedded_agent_manifest(
                "0.4.0", "amd64", extra_components={"unexpected": {"format": "tar.gz", "sha256": "e" * 64, "version": "1.0.0"}})),
            ("duplicate_component_sha256", dup_sha),
            ("unknown_component_field", bad_record),
        ]
        for idx, (label, manifest) in enumerate(bad_manifests):
            with self.subTest(anomaly=label):
                (src / "agent/0.4.0/amd64").write_bytes(b"elf" + encode_embedded_agent_manifest_frame(manifest))
                with self.assertRaises(ValueError):
                    self.assemble_self_contained(f"sc-anomaly-{idx}", src, installer, targets=("amd64",))

        base_json = json.dumps(build_embedded_agent_manifest("0.4.0", "amd64"), separators=(",", ":"))
        dup_body = ('{"schema":1,' + base_json[1:]).encode("utf-8")
        dup_frame = (release.EMBEDDED_FRAME_PREFIX
                     + f"{len(dup_body):08x}:{release.digest(dup_body)}\n".encode("ascii")
                     + dup_body + release.EMBEDDED_FRAME_SUFFIX)
        (src / "agent/0.4.0/amd64").write_bytes(b"elf" + dup_frame)
        with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
            self.assemble_self_contained("sc-dup-key", src, installer, targets=("amd64",))

        bad_macos = build_embedded_agent_manifest(
            "0.4.0", "macos-arm64",
            extra_components={"tcpquality": {"format": "agent_subcommand", "subcommand": "tcp-probe",
                                             "version": "0.3.0-" + "a" * 40 + "-r1"}},
        )
        (src / "agent/0.4.0/macos-arm64").write_bytes(b"macho" + encode_embedded_agent_manifest_frame(bad_macos))
        with self.assertRaises(ValueError):
            self.assemble_self_contained("sc-bad-macos", src, installer, targets=("macos-arm64",))

    def test_verify_bundle_rejects_post_assembly_tampered_self_contained_agent(self):
        src, installer = self.write_self_contained_source("sc-tamper", targets=("amd64",))
        out = self.assemble_self_contained("sc-tamper-bundle", src, installer, targets=("amd64",))
        bare = b"tampered raw agent without embedded manifest"
        (out / "agent-0.4.0-linux-musl-amd64").write_bytes(bare)
        metadata = json.loads((out / "release.json").read_text(encoding="utf-8"))
        metadata["artifacts"][0].update(archive_size=len(bare), binary_size=len(bare),
                                        binary_sha256=release.digest(bare))
        encoded_meta = (json.dumps(metadata, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
        (out / "release.json").write_bytes(encoded_meta)
        rows = {
            "agent/0.4.0/amd64": release.digest(bare),
            "install.sh": release.digest((out / "install.sh").read_bytes()),
            "release.json": release.digest(encoded_meta),
        }
        (out / "SHA256SUMS").write_text("".join(f"{rows[k]}  {k}\n" for k in sorted(rows)), encoding="utf-8")
        self.sign(out)
        with self.assertRaisesRegex(ValueError, "embedded component manifest frame"):
            release.verify_bundle(out, self.roots, "minisign", "agent-v0.4.0")


if __name__ == "__main__":
    unittest.main()
