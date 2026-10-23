#!/usr/bin/env python3
"""Exercise artifact guards without requiring a Linux compiler toolchain."""

import contextlib
import hashlib
import io
import os
import pathlib
import runpy
import struct
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest import mock


TOOLS = pathlib.Path(__file__).resolve().parent
NATIVE_AGENT = runpy.run_path(str(TOOLS / "build-agent.py"))


class BuildScriptTests(unittest.TestCase):
    @contextlib.contextmanager
    def independent_prebuilt_agent(self, reported_version="1.2.3"):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "Cargo.toml").write_text('[workspace.package]\nversion = "9.8.7"\n')
            agent = root / "crates/agent"
            agent.mkdir(parents=True)
            (agent / "Cargo.toml").write_text('[package]\nname = "apsp-agent"\nversion = "1.2.3"\n')
            binary = root / "apsp-agent"
            binary.write_text(
                f"#!{sys.executable}\n"
                "import sys\n"
                f"if sys.argv[1:] == ['--version']: print('apsp-agent {reported_version}')\n"
                "elif sys.argv[1:] == ['--help']: print('apsp-agent')\n"
                "elif sys.argv[1:] == ['run', '--help']: print('--monitor-only')\n"
                "else: sys.exit(2)\n"
            )
            binary.chmod(0o755)
            # Architecture headers have separate coverage. Keep executable calls,
            # version rejection, immutable packaging, and checksums real here.
            with mock.patch.dict(NATIVE_AGENT["main"].__globals__, {
                "REPOSITORY": root, "verify_architecture": mock.Mock(),
            }), mock.patch.object(sys, "argv", [
                "build-agent.py", "x86_64-unknown-freebsd", str(root / "artifacts"),
                "--binary", str(binary),
            ]), contextlib.redirect_stdout(io.StringIO()):
                yield root, binary

    def test_prebuilt_agent_packages_its_independent_version(self):
        with self.independent_prebuilt_agent() as (root, binary):
            NATIVE_AGENT["main"]()
            artifacts = root / "artifacts/agent"
            output = artifacts / "1.2.3"
            payload = binary.read_bytes()
            digest = hashlib.sha256(payload).hexdigest()
            self.assertEqual((output / "x86_64-unknown-freebsd/apsp-agent").read_bytes(), payload)
            self.assertEqual((output / "freebsd-amd64").read_bytes(), payload)
            self.assertEqual((output / "SHA256SUMS").read_text(), f"{digest}  freebsd-amd64\n")
            self.assertFalse((artifacts / "9.8.7").exists())

    def test_prebuilt_agent_rejects_binary_reporting_workspace_version(self):
        with self.independent_prebuilt_agent(reported_version="9.8.7") as (root, _):
            with self.assertRaisesRegex(ValueError, "unexpected Agent version: apsp-agent 9.8.7"):
                NATIVE_AGENT["main"]()
            self.assertFalse((root / "artifacts").exists())

    def test_freebsd_workflow_expects_the_independent_agent_version(self):
        for name in ("platforms.yml", "ci.yml"):
            with self.subTest(workflow=name):
                workflow = (TOOLS.parent / ".github/workflows" / name).read_text()
                _, job, steps = workflow.partition("\n  agent-freebsd:\n")
                if not job:
                    self.assertEqual(name, "ci.yml", "manual FreeBSD coverage must remain available")
                    continue
                source = textwrap.dedent(steps.split("python3 - <<'PY'\n", 1)[1].split("\n          PY", 1)[0])
                with self.independent_prebuilt_agent() as (root, _):
                    with mock.patch.dict(os.environ, {
                        "AGENT_TARGET": "x86_64-unknown-freebsd", "GITHUB_OUTPUT": str(root / "outputs"),
                    }), mock.patch("runpy.run_path", return_value={"verify_architecture": mock.Mock()}), \
                            contextlib.chdir(root):
                        exec(compile(source, f"{name}-freebsd-metadata", "exec"), {})
                    self.assertEqual((root / "outputs").read_text(), "version=1.2.3\n")

    def test_prebuilt_agent_rejects_invalid_architecture_without_rust(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            binary = root / "agent"
            binary.write_bytes(b"not an executable")
            for target in NATIVE_AGENT["TARGETS"]:
                with self.subTest(target=target):
                    artifact_root = root / target
                    result = subprocess.run(
                        [sys.executable, str(TOOLS / "build-agent.py"), target,
                         str(artifact_root), "--binary", str(binary)],
                        capture_output=True, text=True,
                        env={**os.environ, "PATH": str(root)},
                        timeout=30,
                    )
                    self.assertNotEqual(result.returncode, 0)
                    self.assertIn("ValueError", result.stderr)
                    self.assertNotIn("FileNotFoundError", result.stderr)
                    self.assertFalse(artifact_root.exists())

    def test_native_agent_rejects_wrong_binary_architecture(self):
        verify = NATIVE_AGENT["verify_architecture"]
        with tempfile.TemporaryDirectory() as directory:
            binary = pathlib.Path(directory) / "agent"
            for target in NATIVE_AGENT["TARGETS"]:
                with self.subTest(target=target):
                    header = bytearray(128)
                    arm = target.startswith("aarch64")
                    if "windows" in target:
                        header[:2] = b"MZ"
                        struct.pack_into("<I", header, 60, 64)
                        header[64:68] = b"PE\0\0"
                        struct.pack_into("<H", header, 68, 0xAA64 if arm else 0x8664)
                        machine_offset = 68
                    elif "apple" in target:
                        header[:4] = b"\xcf\xfa\xed\xfe"
                        struct.pack_into("<I", header, 4, 0x0100000C)
                        machine_offset = 4
                    else:
                        header[:6] = b"\x7fELF\x02\x01"
                        struct.pack_into("<H", header, 18, 183 if arm else 62)
                        machine_offset = 18
                    binary.write_bytes(header)
                    verify(binary, target)
                    header[machine_offset] ^= 1
                    binary.write_bytes(header)
                    with self.assertRaises(ValueError):
                        verify(binary, target)
                    binary.write_bytes(b"not an executable")
                    with self.assertRaises(ValueError):
                        verify(binary, target)

    def test_native_agent_artifacts_are_immutable_and_checksummed(self):
        package = NATIVE_AGENT["package_binary"]
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            binary = root / "binary"
            payload = b"verified native binary"
            binary.write_bytes(payload)
            for target in NATIVE_AGENT["TARGETS"]:
                with self.subTest(target=target):
                    output = package(binary, target, "0.1.0", root / "artifacts")
                    name = "apsp-agent.exe" if "windows" in target else "apsp-agent"
                    digest = hashlib.sha256(payload).hexdigest()
                    self.assertEqual((output / name).read_bytes(), payload)
                    self.assertEqual(
                        (output / "SHA256SUMS").read_bytes(),
                        f"{digest}  {name}\n".encode(),
                    )
                    with self.assertRaises(FileExistsError):
                        package(binary, target, "0.1.0", root / "artifacts")
                    self.assertEqual((output / name).read_bytes(), payload)

    def test_syntax_and_help(self):
        for name in ("build-agent.sh", "build-singbox.sh"):
            with self.subTest(script=name):
                script = TOOLS / name
                subprocess.run(["bash", "-n", str(script)], check=True)
                result = subprocess.run(
                    ["bash", str(script), "--help"],
                    check=True,
                    capture_output=True,
                    text=True,
                )
                self.assertIn("<amd64|arm64> <ARTIFACT_ROOT>", result.stdout)

    def test_existing_artifact_checksums(self):
        cases = (
            "empty",
            "valid",
            "tampered",
            "missing-sum",
            "missing-file",
            "duplicate",
            "unsafe-name",
            "symlink-file",
            "symlink-sums",
        )
        for name in ("build-agent.sh", "build-singbox.sh"):
            script = (TOOLS / name).read_text()
            source = script.split("<<'PY'\n", 1)[1].split("\nPY\n", 1)[0]
            for case in cases:
                with self.subTest(script=name, case=case):
                    with tempfile.TemporaryDirectory() as directory:
                        root = pathlib.Path(directory)
                        artifact = root / "amd64"
                        manifest = root / "SHA256SUMS"
                        payload = b"previous verified build"
                        digest = hashlib.sha256(payload).hexdigest()
                        if case != "empty":
                            artifact.write_bytes(payload)
                            manifest.write_text(f"{digest}  amd64\n")
                        if case == "tampered":
                            artifact.write_bytes(b"changed")
                        elif case == "missing-sum":
                            manifest.unlink()
                        elif case == "missing-file":
                            artifact.unlink()
                        elif case == "duplicate":
                            manifest.write_text(f"{digest}  amd64\n{digest}  amd64\n")
                        elif case == "unsafe-name":
                            manifest.write_text(f"{digest}  ../amd64\n")
                        elif case == "symlink-file":
                            artifact.rename(root / "original")
                            artifact.symlink_to(root / "original")
                        elif case == "symlink-sums":
                            manifest.rename(root / "original")
                            manifest.symlink_to(root / "original")
                        result = subprocess.run(
                            ["python3", "-", str(root)],
                            input=source,
                            text=True,
                            capture_output=True,
                        )
                        self.assertEqual(
                            result.returncode == 0,
                            case in ("empty", "valid"),
                            result.stderr,
                        )


if __name__ == "__main__":
    unittest.main()
