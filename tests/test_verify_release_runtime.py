#!/usr/bin/env python3
"""Checks for runtime archive and build metadata inspection."""

import gzip
import hashlib
import importlib.util
import io
from pathlib import Path
import struct
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "runtime_inspection", Path(__file__).resolve().parents[1] / "tools/verify-release-runtime.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


def metadata(arch, libc="gnu"):
    settings = {
        "GOOS": "linux", "GOARCH": arch, "CGO_ENABLED": "1", "-trimpath": "true",
        "vcs": "git", "vcs.revision": runtime.REVISION, "vcs.modified": "false",
        "-buildmode": "exe", "-compiler": "gc",
        "-tags": ",".join(sorted(runtime.TAGS | ({"with_musl"} if libc == "musl" else set()))),
    }
    return "runtime: go1.26.8\n" + "\n".join("\tbuild\t" + k + "=" + v for k, v in settings.items())


class RuntimeInspectionTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.directory = self.root / "sing-box" / runtime.VERSION
        self.directory.mkdir(parents=True)

    def tearDown(self):
        self.temporary.cleanup()

    def archive(self, arch, member_name="sing-box", member_type=tarfile.REGTYPE, extra=False, libc=None):
        binary = bytearray(64)
        binary[:7] = b"\x7fELF\x02\x01\x01"
        struct.pack_into("<H", binary, 18, {"amd64": 62, "arm64": 183}[arch])
        contents = io.BytesIO()
        with tarfile.open(fileobj=contents, mode="w") as package:
            member = tarfile.TarInfo(member_name)
            member.type = member_type
            member.size = len(binary) if member_type == tarfile.REGTYPE else 0
            member.linkname = "outside"
            package.addfile(member, io.BytesIO(binary))
            if extra:
                package.addfile(tarfile.TarInfo("extra"))
        archive = gzip.compress(contents.getvalue(), mtime=0)
        target = arch if libc is None else f"linux-{libc}-{arch}"
        (self.directory / target).write_bytes(archive)
        (self.directory / "SHA256SUMS").write_text(hashlib.sha256(archive).hexdigest() + "  " + target + "\n")

    def test_musl_cache_requires_its_build_tag_and_static_elf_on_both_architectures(self):
        for arch in ("amd64", "arm64"):
            self.archive(arch, libc="musl")
            for headers, dynamic, valid in (("LOAD", "No dynamic section", True),
                                             ("INTERP", "No dynamic section", False),
                                             ("LOAD", "(NEEDED) libc.so.6", False)):
                with self.subTest(arch=arch, headers=headers, dynamic=dynamic), \
                        patch.object(runtime.subprocess, "check_output", side_effect=[headers, dynamic]) as reader:
                    if valid:
                        runtime.verify(self.root, arch, lambda _: metadata(arch, "musl"), libc="musl")
                    else:
                        with self.assertRaisesRegex(ValueError, "statically linked"):
                            runtime.verify(self.root, arch, lambda _: metadata(arch, "musl"), libc="musl")
                    self.assertEqual([call.args[0][:2] for call in reader.call_args_list],
                                     [["readelf", "-lW"], ["readelf", "-dW"]])
            with self.assertRaisesRegex(ValueError, "build tags"):
                runtime.verify(self.root, arch, lambda _: metadata(arch), libc="musl")
            with self.assertRaisesRegex(ValueError, "build tags"):
                runtime.verify_metadata(metadata(arch, "musl"), arch)

    def test_musl_archive_cannot_borrow_a_legacy_checksum_identity(self):
        self.archive("amd64")
        (self.directory / "amd64").rename(self.directory / "linux-musl-amd64")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            runtime.verify(self.root, "amd64", libc="musl")

    def test_both_architectures_read_metadata_without_executing_runtime(self):
        for arch in ("amd64", "arm64"):
            with self.subTest(arch=arch):
                self.archive(arch)
                with patch.object(runtime.subprocess, "check_output", return_value=metadata(arch)) as reader:
                    runtime.verify(self.root, arch)
                self.assertEqual(reader.call_args.args[0][:3], ["go", "version", "-m"])

    def test_wrong_elf_architecture_rejected_before_metadata_tool(self):
        self.archive("amd64")
        (self.directory / "amd64").rename(self.directory / "arm64")
        sums = self.directory / "SHA256SUMS"
        sums.write_text(sums.read_text().replace("amd64", "arm64"))
        with patch.object(runtime.subprocess, "check_output") as reader:
            with self.assertRaisesRegex(ValueError, "ELF architecture"):
                runtime.verify(self.root, "arm64")
            reader.assert_not_called()

    def test_required_metadata_fields(self):
        original = metadata("amd64")
        for previous, changed in (
            ("go1.26.8", "go1.26.1"), ("GOOS=linux", "GOOS=darwin"),
            ("GOARCH=amd64", "GOARCH=arm64"), ("CGO_ENABLED=1", "CGO_ENABLED=0"),
            ("-trimpath=true", "-trimpath=false"), (runtime.REVISION, "0" * 40),
            ("vcs.modified=false", "vcs.modified=true"),
            ("with_v2ray_api", "missing_statistics_api"),
        ):
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                runtime.verify_metadata(original.replace(previous, changed), "amd64")
        with self.assertRaisesRegex(ValueError, "duplicate"):
            runtime.verify_metadata(original + "\n\tbuild\tGOOS=linux", "amd64")

    def test_archive_member_boundaries(self):
        for name, kind, extra in (("../sing-box", tarfile.REGTYPE, False),
                                  ("sing-box", tarfile.SYMTYPE, False),
                                  ("sing-box", tarfile.LNKTYPE, False),
                                  ("sing-box", tarfile.REGTYPE, True)):
            self.archive("amd64", name, kind, extra)
            with self.subTest(name=name, kind=kind, extra=extra), self.assertRaises(ValueError):
                runtime.verify(self.root, "amd64", lambda _: metadata("amd64"))

    def test_file_member_and_checksum_limits(self):
        self.archive("amd64")
        with patch.object(runtime, "MAX_ARCHIVE", 1), self.assertRaisesRegex(ValueError, "size"):
            runtime.verify(self.root, "amd64")
        with patch.object(runtime, "MAX_BINARY", 1), self.assertRaisesRegex(ValueError, "size"):
            runtime.verify(self.root, "amd64")
        sums = self.directory / "SHA256SUMS"
        sums.write_text("0" * 64 + "  amd64\n")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            runtime.verify(self.root, "amd64")
        sums.write_text("0" * 64 + "  amd64\n" + "0" * 64 + "  amd64\n")
        with self.assertRaisesRegex(ValueError, "duplicate"):
            runtime.verify(self.root, "amd64")
        sums.unlink()
        sums.symlink_to(self.directory / "amd64")
        with self.assertRaisesRegex(ValueError, "ordinary"):
            runtime.verify(self.root, "amd64")


if __name__ == "__main__":
    unittest.main()
