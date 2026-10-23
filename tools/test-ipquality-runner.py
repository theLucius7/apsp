#!/usr/bin/env python3
"""Exercise receipt preservation without a rootfs, mount or external request."""
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import ipquality_artifact as artifact


def runner():
    namespace = {'__name__': 'apsp_ipquality_runner_fixture'}
    exec(compile(artifact.runner(), '<packaged-IPQuality-fixture>', 'exec'), namespace)
    return SimpleNamespace(**namespace)


class RunnerTests(unittest.TestCase):
    def setUp(self):
        self.runner = runner()
        self.temporary = tempfile.TemporaryDirectory(prefix='apsp-ipquality-runner-test-')
        self.workspace = Path(self.temporary.name).resolve()
        self.args = SimpleNamespace(job_id='00000000-0000-4000-8000-000000000001',
                                    ip_version='4', artifact_sha256='a' * 64)

    def tearDown(self):
        self.temporary.cleanup()

    def receipt(self, status='succeeded'):
        return {'seq': 1, 'provider': 'egress-discovery', 'dataset': 'egress',
                'target_ip': '1.1.1.1' if status == 'succeeded' else None,
                'url': 'https://api64.ipify.org', 'status': status,
                'attempted_at': 1, 'elapsed_ms': 3, 'http_status': 200 if status == 'succeeded' else 403,
                'curl_exit': 0, 'response_bytes': 7, 'error_kind': None if status == 'succeeded' else 'http_403',
                'error_message': None if status == 'succeeded' else 'request refused'}

    def test_discovery_survives_before_first_upstream_chapter_and_stays_partial(self):
        receipt = self.receipt()
        (self.workspace / 'attempts.jsonl').write_text(json.dumps(receipt) + '\n')
        self.runner.snapshot(self.workspace, self.args, 1, None, 1)
        result = json.loads((self.workspace / 'result.json').read_text())
        section = json.loads((self.workspace / 'section-ipquality_result.json').read_text())
        self.assertEqual(result['egress_ip'], '1.1.1.1')
        self.assertIsNone(result['upstream'])
        self.assertIsNone(result['finished_at'])
        self.assertEqual(result['artifact_sha256'], self.args.artifact_sha256)
        self.assertFalse(section['complete'])
        self.assertEqual(json.loads(section['text']), result)

    def test_failed_discovery_has_receipt_and_does_not_create_clean_or_zero(self):
        (self.workspace / 'attempts.jsonl').write_text(json.dumps(self.receipt('failed')) + '\n')
        self.runner.snapshot(self.workspace, self.args, 1, 2, 1)
        result = json.loads((self.workspace / 'result.json').read_text())
        self.assertIsNone(result['egress_ip'])
        self.assertIsNone(result['upstream'])
        self.assertEqual(result['attempts'][0]['error_kind'], 'http_403')

    def test_wrong_upstream_target_is_not_projected_as_success(self):
        (self.workspace / 'attempts.jsonl').write_text(json.dumps(self.receipt()) + '\n')
        (self.workspace / 'partial.json').write_text(json.dumps({'Head': {'IP': '8.8.8.8', 'Version': 'v2026-09-16'}}))
        self.runner.snapshot(self.workspace, self.args, 1, None, 2)
        result = json.loads((self.workspace / 'result.json').read_text())
        self.assertIsNone(result['upstream'])
        self.assertEqual(result['egress_ip'], '1.1.1.1')

    def test_malformed_head_preserves_failed_receipts_without_exception(self):
        for head in (None, [], 'unknown'):
            (self.workspace / 'partial.json').write_text(json.dumps({'Head': head}))
            (self.workspace / 'attempts.jsonl').write_text(json.dumps(self.receipt('failed')) + '\n')
            self.runner.snapshot(self.workspace, self.args, 1, 2, 1)
            result = json.loads((self.workspace / 'result.json').read_text())
            self.assertIsNone(result['upstream'])
            self.assertEqual(result['attempts'][0]['error_kind'], 'http_403')

    def test_partial_json_and_receipts_remain_when_stdout_is_incomplete(self):
        upstream = {'Head': {'IP': '1.1.1.1', 'Version': 'v2026-09-16'}, 'Score': {'IPQS': None}}
        (self.workspace / 'partial.json').write_text(json.dumps(upstream))
        (self.workspace / 'upstream.json').write_text('{')
        (self.workspace / 'attempts.jsonl').write_text(json.dumps(self.receipt()) + '\n{"unfinished":')
        self.runner.snapshot(self.workspace, self.args, 1, None, 2)
        result = json.loads((self.workspace / 'result.json').read_text())
        self.assertEqual(result['upstream'], upstream)
        self.assertEqual(len(result['attempts']), 1)

    def test_root_namespace_refuses_before_any_mount_or_child(self):
        with patch.object(self.runner.os, 'geteuid', return_value=0), patch.object(self.runner.sys, 'platform', 'linux'), \
                patch.object(self.runner.os, 'readlink', return_value='mnt:[same]'), \
                patch.object(self.runner.subprocess, 'Popen', side_effect=AssertionError('must not execute')):
            with self.assertRaisesRegex(ValueError, 'private mount namespace'):
                self.runner.execute(self.args, self.workspace)


if __name__ == '__main__':
    unittest.main()
