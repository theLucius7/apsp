#!/usr/bin/env python3
"""Reject unusable cache entries before they can become installation artifacts."""

import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch


SPEC = importlib.util.spec_from_file_location("verify_runtime", Path(__file__).with_name("verify-e2e-runtime.py"))
VERIFIER = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VERIFIER)


class CacheContracts(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.artifacts = self.root / "sing-box" / "1.14.2-apsp.1"
        self.artifacts.mkdir(parents=True)

    def package(self, name="sing-box", link=False):
        archive = self.artifacts / "amd64"
        with tarfile.open(archive, "w:gz") as output:
            member = tarfile.TarInfo(name)
            if link:
                member.type = tarfile.SYMTYPE
                member.linkname = "/unrelated/runtime"
                output.addfile(member)
            else:
                member.size = 7
                output.addfile(member, io.BytesIO(b"fixture"))
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.artifacts / "SHA256SUMS").write_text(digest + "  amd64\n")

    def test_modified_cached_archive_is_rejected_before_execution(self):
        self.package()
        with (self.artifacts / "amd64").open("ab") as output:
            output.write(b"unexpected mutation")
        with patch.object(VERIFIER.subprocess, "check_output") as command:
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                VERIFIER.verify(self.root, self.root / "runtime")
            command.assert_not_called()

    def test_checksum_alone_does_not_allow_archive_paths_or_symlinks(self):
        for name, link in (("../sing-box", False), ("sing-box", True)):
            with self.subTest(name=name, link=link):
                self.package(name, link)
                with patch.object(VERIFIER.subprocess, "check_output") as command:
                    with self.assertRaisesRegex(ValueError, "archive members|binary member"):
                        VERIFIER.verify(self.root, self.root / ("runtime-link" if link else "runtime-path"))
                    command.assert_not_called()

    def test_right_version_with_missing_default_tag_is_rejected(self):
        self.package()
        tags = VERIFIER.TAGS - {"with_naive_outbound"}
        version = f"sing-box version 1.14.2-apsp.1\nTags: {','.join(sorted(tags))}\nRevision: {VERIFIER.REVISION}\nCGO: enabled\n"
        with patch.object(VERIFIER.subprocess, "check_output", side_effect=["Advanced Micro Devices X86-64", version]):
            with self.assertRaisesRegex(ValueError, "tags mismatch"):
                VERIFIER.verify(self.root, self.root / "runtime")


if __name__ == "__main__":
    unittest.main()
