#!/usr/bin/env python3
"""Verify a freshly built native artifact through the existing TEST_ONLY release contract."""
import os
from pathlib import Path
import shutil
import unittest

import test_release
import test_tcp_probe_artifacts as contracts


@unittest.skipUnless(os.environ.get("APSP_TCP_ARTIFACT_ROOT"), "native build is exercised by the artifact matrix")
class NativeBundleTests(unittest.TestCase):
    def test_real_native_musl_payload_is_signed_and_verified_with_complete_provenance(self):
        root = Path(os.environ["APSP_TCP_ARTIFACT_ROOT"])
        commit = os.environ["APSP_TCP_SOURCE_COMMIT"]
        arch = os.environ["APSP_TCP_ARCH"]
        fixture = test_release.ReleaseTests(methodName="runTest")
        fixture.setUp()
        try:
            version = contracts.tcp.artifact_version(commit)
            source = root / "tcpquality" / version / arch
            files = contracts.tcp.archive_files(source.read_bytes())
            info = contracts.tcp.validate_files(files, version, arch)
            self.assertEqual(info["source_commit"], commit)
            binary = fixture.directory / contracts.tcp.BINARY
            binary.write_bytes(files[contracts.tcp.BINARY])
            binary.chmod(0o700)
            contracts.builder.verify_binary(binary, commit)
            output = fixture.source / "tcpquality" / version
            output.mkdir(parents=True)
            shutil.copyfile(source, output / arch)
            shutil.rmtree(fixture.bundle)
            fixture.arguments.arch = [arch]
            fixture.arguments.tcp_probe_version = version
            contracts.release.assemble(fixture.arguments)
            fixture.sign()
            entries = fixture.verify()["artifacts"]
            self.assertEqual(len(entries), 4)
            entry = next(item for item in entries if item["name"] == "tcpquality")
            self.assertEqual(set(entry["auxiliary_files"]), contracts.tcp.FILES - {contracts.tcp.BINARY})
            with self.assertRaises(ValueError):
                contracts.release.load_roots(test_release.FIXTURES / "public-keys.json", publication=True)
        finally:
            fixture.tearDown()
