#!/usr/bin/env python3
"""Signed CI fixture files retain their exact bytes on Windows."""

from pathlib import Path
import runpy
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/protocol/tests/fixtures"
HELPER = runpy.run_path(str(ROOT / "tools/ci-release-fixture.py"))


class CiFixtureTests(unittest.TestCase):
    def test_installed_proof_preserves_signed_utf8_bytes_with_windows_newlines(self):
        proof = dict(metadata_json='{"fixture":"测试"}\n',
                     checksums=(FIXTURES / "payload.txt").read_bytes().decode("utf-8"),
                     signature=(FIXTURES / "payload.txt.minisig").read_bytes().decode("utf-8"))
        original_open = Path.open

        def windows_open(path, mode="r", *args, **kwargs):
            if "b" not in mode and kwargs.get("newline") is None:
                kwargs["newline"] = "\r\n"
            return original_open(path, mode, *args, **kwargs)

        with tempfile.TemporaryDirectory(prefix="apsp-ci-fixture-") as temporary:
            directory = Path(temporary)
            with patch.object(Path, "open", windows_open):
                HELPER["install"](directory, proof)
            for name, field in (("release.json", "metadata_json"),
                                ("SHA256SUMS", "checksums"),
                                ("SHA256SUMS.minisig", "signature")):
                self.assertEqual((directory / name).read_bytes(), proof[field].encode("utf-8"))
            if shutil.which("minisign"):
                result = subprocess.run(["minisign", "-V", "-q", "-m",
                                         str(directory / "SHA256SUMS"), "-x",
                                         str(directory / "SHA256SUMS.minisig"), "-p",
                                         str(FIXTURES / "TEST_ONLY.pub")], capture_output=True)
                self.assertEqual(result.returncode, 0, result.stderr.decode(errors="replace"))


if __name__ == "__main__":
    unittest.main()
