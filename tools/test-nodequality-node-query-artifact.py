#!/usr/bin/env python3
"""Verify official query packaging with fixed local sources and inert fixtures."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest

sys.dont_write_bytecode = True
import nodequality_node_query_artifact as artifact
import nodequality_rootfs_artifact as canonical
import release


class OfficialQueryArtifactTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        directory = os.environ.get("APSP_NODEQUALITY_CANONICAL_SOURCES")
        if not directory:
            raise unittest.SkipTest("requires verified local canonical NodeQuality sources")
        helper = canonical.module("official_query_fixture_sources", canonical.PLUGIN / "source-helper.py")
        lock = helper.decode(helper.ordinary(canonical.PLUGIN / "source-lock.json", 65536))
        cls.bundle = helper.pack(lock, Path(directory))
        cls.base = canonical.canonical_runner(cls.bundle)
        cls.runner = artifact.runner(cls.base)

    def test_retains_all_canonical_sources_and_full_licenses(self):
        for sentinel in canonical.MARKERS.values():
            self.assertEqual(canonical.embedded(self.runner, sentinel),
                             canonical.embedded(self.base, sentinel))
        self.assertEqual(canonical.embedded(self.runner, "APSP_NODEQUALITY_EXECUTION_ADMISSION"),
                         canonical.embedded(self.base, "APSP_NODEQUALITY_EXECUTION_ADMISSION"))
        self.assertEqual(canonical.embedded(self.runner, "APSP_OFFICIAL_IP_HELPER"),
                         canonical.embedded(self.base, "APSP_OFFICIAL_IP_HELPER"))
        helper = (canonical.PLUGIN / "node-query.py").read_bytes()
        self.assertEqual(canonical.embedded(self.runner, "APSP_OFFICIAL_NODE_QUERY"), helper)
        for arch in ("amd64", "arm64"):
            artifact.validate_files({artifact.BINARY: self.runner}, artifact.VERSION, arch)

    def test_version_and_full_gate_work_without_prerequisites_or_workspace_io(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory).resolve()
            script = directory / "nodequality"
            script.write_bytes(self.runner)
            environment = dict(os.environ, PATH="/nonexistent", BASH_ENV="", ENV="")
            version = subprocess.run(["/bin/bash", str(script), "--version"], env=environment,
                                     capture_output=True, timeout=3)
            self.assertEqual(version.returncode, 0, version.stderr)
            self.assertEqual(version.stdout.decode().strip(), "nodequality " + artifact.VERSION)
            workspace = directory / "never-created"
            result = subprocess.run(["/bin/bash", str(script), "--workspace", str(workspace),
                                     "--mode", "full"], env=environment, capture_output=True, timeout=3)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(b"new full diagnostics are paused", result.stderr)
            self.assertFalse(workspace.exists())

    def test_changed_sources_or_embedded_queries_cannot_receive_fixed_identity(self):
        for content in (self.base + b"# altered\n", self.base.replace(b"umask 077", b"umask 022", 1)):
            with self.assertRaises(ValueError):
                artifact.runner(content)
        for content in (self.runner + b"# altered\n", self.runner.replace(b"api.ipregistry.co", b"untrusted.invalid", 1)):
            with self.assertRaises(ValueError):
                artifact.validate_files({artifact.BINARY: content}, artifact.VERSION, "amd64")
        for version in (canonical.CANONICAL_VERSION, canonical.VERSION):
            with self.assertRaises(ValueError):
                artifact.validate_files({artifact.BINARY: self.runner}, version, "amd64")

    def test_deterministic_archive_has_only_signed_runner_and_rejects_extra_inventory(self):
        files = {artifact.BINARY: self.runner}
        data = artifact.pack(files)
        self.assertEqual(data, artifact.pack(files))
        self.assertEqual(artifact.archive_files(data), files)
        self.assertEqual(release.binary_bytes(data, "tar.gz", artifact.BINARY), self.runner)
        with self.assertRaises(ValueError):
            artifact.pack(dict(files, **{"credential.json": b"private"}))

    def test_immutable_packaging_preserves_other_architecture_checksum(self):
        specification = importlib.util.spec_from_file_location("official_query_builder",
                            canonical.ROOT / "tools/build-nodequality-node-query.py")
        builder = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(builder)
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory).resolve()
            args = SimpleNamespace(arch="amd64", sources=Path(os.environ["APSP_NODEQUALITY_CANONICAL_SOURCES"]), output=output)
            first = builder.build(args)
            initial = first.read_bytes()
            with self.assertRaises(ValueError):
                builder.build(args)
            self.assertEqual(first.read_bytes(), initial)
            args.arch = "arm64"
            second = builder.build(args)
            self.assertEqual(initial, second.read_bytes())
            self.assertEqual(builder.checksums(first.parent),
                             {"amd64": artifact.digest(initial), "arm64": artifact.digest(initial)})
            self.assertFalse((first.parent / ".build.lock").exists())


if __name__ == "__main__":
    unittest.main()
