#!/usr/bin/env python3
"""OpenRC fixture trust provisioning and signed installer version contracts."""

import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import release

spec = importlib.util.spec_from_file_location("openrc_smoke", ROOT / "tools/openrc-smoke.py")
smoke = importlib.util.module_from_spec(spec)
spec.loader.exec_module(smoke)
PUBLIC_KEYS = ROOT / "crates/protocol/tests/fixtures/public-keys.json"


class OpenrcFixtureTests(unittest.TestCase):
    def test_public_trust_copy_is_independent_and_protected(self):
        source_before = PUBLIC_KEYS.stat()
        with tempfile.TemporaryDirectory() as temporary:
            destination = Path(temporary) / "trust"
            copied = smoke.provision_test_roots(destination)
            self.assertEqual(copied.read_bytes(), PUBLIC_KEYS.read_bytes())
            self.assertFalse(copied.samefile(PUBLIC_KEYS))
            self.assertEqual(copied.stat().st_uid, os.geteuid())
            for path in (destination, copied):
                self.assertFalse(path.is_symlink())
                self.assertEqual(path.stat().st_mode & 0o022, 0)
            with self.assertRaises(FileExistsError):
                smoke.provision_test_roots(destination)
        source_after = PUBLIC_KEYS.stat()
        self.assertEqual((source_before.st_uid, source_before.st_mode, source_before.st_mtime_ns),
                         (source_after.st_uid, source_after.st_mode, source_after.st_mtime_ns))

    @unittest.skipUnless(os.geteuid() == 0, "container root is needed to reproduce runner ownership")
    def test_runner_owned_checkout_is_rejected_but_container_copy_is_accepted(self):
        with tempfile.TemporaryDirectory(prefix="apsp-openrc-trust-", dir="/root") as temporary:
            directory = Path(temporary)
            checkout = directory / "checkout"
            fixture = checkout / "crates/protocol/tests/fixtures/public-keys.json"
            fixture.parent.mkdir(parents=True)
            fixture.write_bytes(PUBLIC_KEYS.read_bytes())
            os.chown(checkout, 1001, 1001)
            with self.assertRaisesRegex(ValueError, "root-owned and protected"):
                release.load_roots(fixture, require_protected=True)
            with patch.object(smoke, "ROOT", checkout):
                copied = smoke.provision_test_roots(directory / "trust")
            self.assertEqual(release.load_roots(copied, require_protected=True),
                             release.load_roots(PUBLIC_KEYS))
            self.assertEqual(checkout.stat().st_uid, 1001)

    def test_existing_symlink_is_never_replaced_or_followed(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            destination = directory / "trust"
            destination.symlink_to(directory, target_is_directory=True)
            with self.assertRaises(FileExistsError):
                smoke.provision_test_roots(destination)
            self.assertFalse((directory / "public-keys.json").exists())

    @unittest.skipUnless(shutil.which("minisign"), "signed fixture verification requires minisign")
    def test_agent_version_signed_manifest_and_bootstrap_tag_agree(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            bundle = directory / "bundle"
            bundle.mkdir(mode=0o700)
            trust_directory = directory / "trust"
            trust_file = smoke.provision_test_roots(trust_directory)
            with patch.object(smoke, "BUNDLE", bundle), \
                    patch.object(smoke, "TRUST_DIRECTORY", trust_directory), \
                    patch.object(smoke, "TRUST_FILE", trust_file):
                command = smoke.install_script("http://127.0.0.1:12345")
            expected_tag = f"agent-v{smoke.AGENT_VERSION}"
            self.assertEqual(command[command.index("--tag") + 1], expected_tag)
            self.assertEqual(command[command.index("--trusted-keys") + 1], str(trust_file))
            installer = trust_directory / "trusted-install.sh"
            self.assertEqual(command[command.index("--trusted-installer") + 1], str(installer))
            self.assertEqual(installer.read_bytes(), (bundle / "install.sh").read_bytes())
            self.assertFalse(installer.is_symlink())
            self.assertEqual(installer.stat().st_mode & 0o777, 0o600)
            self.assertEqual(trust_file.read_bytes(), PUBLIC_KEYS.read_bytes())
            release.verify_manifest(bundle, release.load_roots(PUBLIC_KEYS), "minisign", expected_tag)
            metadata = json.loads((bundle / "release.json").read_text())
            self.assertEqual(metadata["artifacts"][0]["version"], smoke.AGENT_VERSION)
            result = subprocess.run([sys.executable, "-c", smoke.AGENT.decode(), "--version"],
                                    check=True, capture_output=True, text=True)
            self.assertEqual(result.stdout.strip(), f"apsp-agent {smoke.AGENT_VERSION}")


if __name__ == "__main__":
    unittest.main()
