#!/usr/bin/env python3
"""Exercise bounded daily probes against private loopback fixtures."""
import importlib.util
import json
import pathlib
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import Mock, patch

ROOT = pathlib.Path(__file__).resolve().parent.parent
HELPER = ROOT / "crates/observability/upstream/nodequality/daily.py"
spec = importlib.util.spec_from_file_location("daily", HELPER)
daily = importlib.util.module_from_spec(spec)
spec.loader.exec_module(daily)


class DailyChecks(unittest.TestCase):
    def execute(self, root, targets, helper=HELPER, version="ipv4"):
        file = root / "daily-targets.json"
        file.write_text(json.dumps(targets))
        return subprocess.run([sys.executable, str(helper), str(root), str(file), version],
                              capture_output=True, timeout=10)

    def test_whitelist_rejects_large_unknown_and_shell_targets(self):
        valid = {"name": "private fixture", "target": "127.0.0.1", "port": 443}
        daily.validate([valid])
        for value in ([valid] * 5, [dict(valid, port=True)], [dict(valid, port=0)],
                      [dict(valid, target="$(id)")], [dict(valid, command="id")]):
            with self.subTest(value=value), self.assertRaises(ValueError):
                daily.validate(value)

    def test_loopback_checks_four_connections_and_keeps_a_single_chapter(self):
        with tempfile.TemporaryDirectory() as directory, socket.socket() as listener:
            root = pathlib.Path(directory)
            listener.bind(("127.0.0.1", 0))
            listener.listen(16)
            result = self.execute(root, [{"name": "private", "target": "127.0.0.1", "port": listener.getsockname()[1]}])
            self.assertEqual(result.returncode, 0, result.stderr)
            text = (root / "result.txt").read_text()
            self.assertIn("成功 4 / 4", text)
            chapter = json.loads((root / "section-net_quality.json").read_text())
            self.assertEqual(chapter["text"], text)
            self.assertTrue(chapter["complete"])
            self.assertEqual(chapter["revision"], 1)
            self.assertFalse((root / "report-url.txt").exists())
            self.assertEqual(list(root.glob("section-*.json")), [root / "section-net_quality.json"])

    def test_closed_port_is_unknown_latency_and_never_clean_or_zero(self):
        with tempfile.TemporaryDirectory() as directory, socket.socket() as reserved:
            root = pathlib.Path(directory)
            reserved.bind(("127.0.0.1", 0))
            result = self.execute(root, [{"name": "closed", "target": "127.0.0.1", "port": reserved.getsockname()[1]}])
            self.assertEqual(result.returncode, 0, result.stderr)
            text = (root / "result.txt").read_text()
            self.assertIn("成功 0 / 4", text)
            self.assertIn("未知（连接失败或超时）", text)

    def test_missing_targets_explicitly_stay_unknown(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            result = self.execute(root, [])
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("网络质量未知", (root / "result.txt").read_text())

    def test_stalled_dns_is_killed_after_two_seconds(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            helper = root / "stall.py"
            (root / "official-ip.py").write_bytes((HELPER.parent / "official-ip.py").read_bytes())
            helper.write_text(HELPER.read_text().replace(
                "values = socket.getaddrinfo", "time.sleep(60)\n        values = socket.getaddrinfo", 1))
            started = time.monotonic()
            result = self.execute(root, [{"name": "stall", "target": "127.0.0.1", "port": 443}], helper)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertLess(time.monotonic() - started, 6)
            self.assertIn("DNS 解析超过 2 秒", (root / "result.txt").read_text())

    def test_both_preserves_each_family_after_many_resolver_addresses(self):
        ipv4 = (socket.AF_INET, socket.SOCK_STREAM, 6, "", ("127.0.0.1", 443))
        ipv6 = (socket.AF_INET6, socket.SOCK_STREAM, 6, "", ("::1", 443, 0, 0))
        for first, second in ((ipv6, ipv4), (ipv4, ipv6)):
            with self.subTest(first=first[0]):
                sender = Mock()
                with patch.object(daily.socket, "getaddrinfo", return_value=[first] * 8 + [second]):
                    daily.resolve_child("private.invalid", 443, socket.AF_UNSPEC, sender)
                addresses = sender.send.call_args.args[0]
                self.assertEqual(dict(addresses), {first[0]: first[4], second[0]: second[4]})
                self.assertEqual(len(addresses), 2)
                sender.close.assert_called_once()

    def test_ipv6_whitelist_does_not_silently_use_ipv4(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            result = self.execute(root, [{"name": "ipv4 only", "target": "127.0.0.1", "port": 443}], version="ipv6")
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertIn("DNS/地址失败", (root / "result.txt").read_text())


if __name__ == "__main__":
    unittest.main()
