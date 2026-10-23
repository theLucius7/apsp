#!/usr/bin/env python3
"""Check byte-preserving signed fixtures, including Windows text-mode behavior."""

from pathlib import Path
import runpy
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import release

FIXTURE = runpy.run_path(str(ROOT / "tools/ci-release-fixture.py"))
PATH_OPEN = Path.open


def windows_open(path, mode="r", buffering=-1, encoding=None, errors=None, newline=None):
    # Reproduce Windows newline translation without requiring a Windows test host.
    if "b" not in mode and newline is None:
        newline = "\r\n"
    return PATH_OPEN(path, mode, buffering, encoding, errors, newline)


class SignedFixtureTests(unittest.TestCase):
    def test_installed_proof_preserves_signed_bytes_under_windows_text_mode(self):
        proof = FIXTURE["proof"]("agent", "0.3.0", "apsp-agent.exe", b"TEST ONLY Agent",
                                 arch="windows-arm64")
        with tempfile.TemporaryDirectory(prefix="apsp-proof-bytes-") as temporary:
            root = Path(temporary)
            with patch.object(Path, "open", windows_open):
                FIXTURE["install"](root, proof)
            for name, field in (("release.json", "metadata_json"), ("SHA256SUMS", "checksums"),
                                ("SHA256SUMS.minisig", "signature")):
                self.assertEqual((root / name).read_bytes(), proof[field].encode("utf-8"))
            roots = release.load_roots(ROOT / "crates/protocol/tests/fixtures/public-keys.json")
            release.verify_signature(root, roots, "minisign")
            with (root / "SHA256SUMS").open("ab") as output:
                output.write(b"tampered\n")
            with self.assertRaisesRegex(ValueError, "no trusted key"):
                release.verify_signature(root, roots, "minisign")


if __name__ == "__main__":
    unittest.main()
