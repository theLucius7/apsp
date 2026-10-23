#!/usr/bin/env python3
"""Exercise credential preservation and validation through the real CLI."""

from pathlib import Path
import os
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).resolve().parents[1] / "scripts" / "init-env.py"


class InitEnvironmentTests(unittest.TestCase):
    def invoke(self, destination, *arguments):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--output", str(destination), *arguments],
            capture_output=True, text=True, check=False,
        )

    def test_private_valid_environment_and_unchanged_credentials_on_repeat(self):
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / ".env"
            arguments = ("--public-url", "https://panel.example.com/", "--port", "18080")
            result = self.invoke(destination, *arguments)
            self.assertEqual(result.returncode, 0, result.stderr)
            content = destination.read_bytes()
            values = dict(line.split("=", 1) for line in content.decode().splitlines())
            self.assertEqual(len(values), 7)
            self.assertEqual(values["APSP_RELEASE_PUBLIC_KEYS"], "[]")
            self.assertEqual(values["APSP_PUBLIC_URL"], "https://panel.example.com")
            self.assertEqual(values["APSP_PORT"], "18080")
            self.assertNotEqual(values["APSP_DB_PASSWORD"], values["APSP_ADMIN_PASSWORD"])
            for key in ("APSP_DB_PASSWORD", "APSP_ADMIN_PASSWORD"):
                self.assertRegex(values[key], r"^[0-9a-f]{64}$")
                self.assertNotIn(values[key], result.stdout + result.stderr)
            if os.name == "posix":
                self.assertEqual(destination.stat().st_mode & 0o777, 0o600)
            self.assertNotEqual(self.invoke(destination, *arguments).returncode, 0)
            self.assertEqual(destination.read_bytes(), content)

    def test_invalid_input_creates_no_file(self):
        invalid = [
            ("--public-url", "https://panel.example.com/path"),
            ("--public-url", "https://user:secret@panel.example.com"),
            ("--public-url", "https://panel.example.com?token=x"),
            ("--public-url", "https://panel.example.com/#fragment"),
            ("--public-url", "https://panel.example.com\nBAD=value"),
            ("--public-url", "https://panel.example.com:70000"),
            ("--public-url", "https://panel.example.com;unexpected"),
            ("--public-url", "https://panel.example.com", "--port", "0"),
            ("--public-url", "https://panel.example.com", "--port", "65536"),
        ]
        with tempfile.TemporaryDirectory() as directory:
            destination = Path(directory) / ".env"
            for arguments in invalid:
                with self.subTest(arguments=arguments):
                    self.assertNotEqual(self.invoke(destination, *arguments).returncode, 0)
                    self.assertFalse(destination.exists())

    @unittest.skipUnless(os.name == "posix", "symlink protection requires Unix")
    def test_symlink_is_never_followed(self):
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "private"
            target.write_text("keep existing credentials")
            destination = Path(directory) / ".env"
            destination.symlink_to(target)
            result = self.invoke(destination, "--public-url", "https://panel.example.com")
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(target.read_text(), "keep existing credentials")


if __name__ == "__main__":
    unittest.main()
