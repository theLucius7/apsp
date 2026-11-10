#!/usr/bin/env python3
"""Verify offline reference bytes through the actual signed-source serving path."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / 'crates/observability/upstream/nodequality'
READONLY_SOURCES = None


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


policy = module('data_policy', PLUGIN / 'data-policy.py')
helper = module('source_helper', PLUGIN / 'source-helper.py')
source_tests = module('source_tests', ROOT / 'tools/test-nodequality-sources.py')
report = module('report_policy', PLUGIN / 'report-policy.py')
dependency = module('dependency_policy', PLUGIN / 'dependency-policy.py')


class DataTests(unittest.TestCase):
    def fixture(self):
        fixture = source_tests.SourceTests(methodName='runTest')
        fixture.setUp()
        self.addCleanup(fixture.tearDown)
        return fixture

    def test_served_references_keep_shell_metacharacters_as_data_without_curl_or_external_tools(self):
        fixture = self.fixture()
        contents = {name: (fixture.sources / name).read_bytes() for name in helper.FILES}
        payload = b"literal '; touch \"$MARKER\"; #\n$(touch \"$MARKER\")\n`touch \"$MARKER\"`\n%s \\n \\t \xe4\xb8\xad\xe6\x96\x87\n"
        for row in fixture.lock['files']:
            if row['name'] in policy.DATA:
                value = row['name'].encode() + b'\n' + payload
                contents[row['name']] = value
                (fixture.sources / row['name']).write_bytes(value)
                row.update(size=len(value), sha256=hashlib.sha256(value).hexdigest())
        source_tests.fixture.prepare_policy(fixture.fixture_plugin, contents)
        private = module('private_serving', fixture.fixture_plugin / 'source-helper.py')
        target = fixture.root / 'data-sources'
        helper.materialize(helper.decode(helper.pack(fixture.lock, fixture.sources)), target)
        marker = fixture.root / 'unexpected-command'
        trace = fixture.root / 'curl-called'
        for role, requests in policy.REQUESTS.items():
            url = next(url for url, value in helper.ALIASES.items() if value == role)
            served = private.serve(target, ['-Ls', url])
            for request, name in requests.items():
                with self.subTest(role=role, reference=name):
                    expression = policy.data_command(contents[name])
                    self.assertIn(expression, served)
                    self.assertNotIn(request, served)
                    prefix = b'curl(){ printf called > "$TRACE"; printf old-network-value; }\n'
                    env = {'PATH': '', 'MARKER': str(marker), 'TRACE': str(trace)}
                    old = subprocess.run(['/bin/bash'], input=prefix + request, env=env, capture_output=True, timeout=3)
                    self.assertEqual(old.returncode, 0, old.stderr)
                    self.assertEqual(old.stdout, b'old-network-value')
                    self.assertTrue(trace.exists())
                    trace.unlink()
                    new = subprocess.run(['/bin/bash'], input=prefix + expression, env=env, capture_output=True, timeout=3)
                    self.assertEqual(new.returncode, 0, new.stderr)
                    self.assertEqual(new.stdout, contents[name])
                    self.assertFalse(trace.exists())
                    self.assertFalse(marker.exists())

    def test_missing_corrupt_fifo_and_symlink_data_fail_before_serving_any_script(self):
        for role, requests in policy.REQUESTS.items():
            for name in requests.values():
                for kind in ('missing', 'corrupt', 'symlink', 'fifo'):
                    with self.subTest(reference=name, kind=kind):
                        fixture = self.fixture()
                        path = fixture.materialized / name
                        content = path.read_bytes()
                        path.unlink()
                        if kind == 'corrupt':
                            path.write_bytes(content + b'!')
                        elif kind == 'symlink':
                            path.symlink_to(fixture.sources / name)
                        elif kind == 'fifo':
                            os.mkfifo(path, 0o600)
                        url = next(url for url, value in helper.ALIASES.items() if value == role)
                        run = fixture.shim('-Ls', url)
                        self.assertNotEqual(run.returncode, 0)
                        self.assertEqual(run.stdout, b'')
                        self.assertFalse(fixture.called.exists())
                        self.assertFalse(fixture.executed.exists())

    def test_helper_file_is_bounded_ordinary_and_exactly_verified(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            path = root / 'data-policy.py'
            with mock.patch.object(helper, '__file__', str(root / 'source-helper.py')):
                path.write_bytes((PLUGIN / path.name).read_bytes())
                self.assertIn('transform', helper.data_policy())
                for content in (path.read_bytes() + b'!', b'x' * 65537):
                    path.write_bytes(content)
                    with self.assertRaises(ValueError):
                        helper.data_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.data_policy()
                path.symlink_to(PLUGIN / path.name)
                with self.assertRaises(OSError):
                    helper.data_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.data_policy()

    def test_invalid_text_roles_inputs_anchors_and_outputs_are_rejected(self):
        for content in (b'', b'\0', b'\xff', b'x' * (policy.MAX_SOURCE + 1), 'text'):
            with self.assertRaises((ValueError, UnicodeError)):
                policy.data_command(content)
        role = 'ip.sh'
        source = source_tests.fixture.data_anchors(role)
        files = {name: name.encode() for name in policy.REQUESTS[role].values()}
        specs = {name: {'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()} for name, data in files.items()}
        identity = {'source_sha256': hashlib.sha256(source).hexdigest(),
                    'patched_sha256': hashlib.sha256(policy.patch(role, source, files)).hexdigest()}
        with mock.patch.dict(policy.DATA, specs), mock.patch.dict(policy.SOURCES, {role: identity}):
            self.assertEqual(policy.transform(role, source, files), policy.patch(role, source, files))
            for invalid in ({}, dict(files, unexpected=b'x'), {name: b'x' for name in files}):
                with self.assertRaises(ValueError):
                    policy.transform(role, source, invalid)
            with self.assertRaises(ValueError):
                policy.transform(role, source + b'!', files)
            with mock.patch.dict(identity, patched_sha256='0' * 64):
                with self.assertRaisesRegex(ValueError, 'output SHA256'):
                    policy.transform(role, source, files)
            with self.assertRaisesRegex(ValueError, 'exactly once'):
                policy.patch(role, source + source, files)
        with self.assertRaises(ValueError):
            policy.transform('unknown', b'', {})

    def test_production_serve_preserves_every_other_byte_and_emits_each_original_reference(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires previously verified readonly fixed sources and seven reference files')
        bundle = helper.decode(helper.pack(json.loads((PLUGIN / 'source-lock.json').read_bytes()), READONLY_SOURCES))
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name) / 'sources'
            helper.materialize(bundle, directory)
            contents = {path.name: path.read_bytes() for path in directory.iterdir()}
            for role, requests in policy.REQUESTS.items():
                prior = dependency.transform(role, report.transform(role, contents[role]))
                url = next(url for url, value in helper.ALIASES.items() if value == role)
                served = helper.serve(directory, ['-Ls', url])
                prior_netflix = source_tests.fixture.undo_netflix(role, served)
                prior_score = source_tests.fixture.undo_ip_scores(role, prior_netflix)
                self.assertEqual(hashlib.sha256(prior_score).hexdigest(), policy.SOURCES[role]['patched_sha256'])
                self.assertEqual(source_tests.fixture.undo_data(role, prior_score, contents), prior)
                self.assertEqual((directory / role).read_bytes(), contents[role])
                syntax = subprocess.run(['/bin/bash', '-n'], input=served, capture_output=True, timeout=3)
                self.assertEqual(syntax.returncode, 0, syntax.stderr)
                for request, filename in requests.items():
                    self.assertNotIn(request, served)
                    expression = policy.data_command(contents[filename])
                    self.assertIn(expression, served)
                    read = subprocess.run(['/bin/bash'], input=expression, env={'PATH': ''}, capture_output=True, timeout=3)
                    self.assertEqual(read.returncode, 0, read.stderr)
                    self.assertEqual(read.stdout, contents[filename])

    def test_builder_rejects_changed_or_missing_policy_and_data_without_publishing(self):
        for relative in ('crates/observability/upstream/nodequality/data-policy.py', 'inputs/ip-dnsbl.list'):
            for missing in (True, False):
                with self.subTest(path=relative, missing=missing):
                    fixture = self.fixture()
                    tree, env = fixture.build_tree()
                    path = tree / relative if relative.startswith('plugins/') else fixture.sources / 'ip-dnsbl.list'
                    if missing:
                        path.unlink()
                    else:
                        path.write_bytes(path.read_bytes() + b'!')
                    run = fixture.build(tree, env, 'arm64')
                    self.assertNotEqual(run.returncode, 0)
                    output = fixture.root / 'artifacts/nodequality' / source_tests.VERSION
                    self.assertFalse((output / 'arm64').exists())
                    self.assertFalse((output / 'SHA256SUMS').exists())


if __name__ == '__main__':
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--readonly-upstream-dir', type=Path)
    args, remaining = parser.parse_known_args()
    READONLY_SOURCES = args.readonly_upstream_dir
    sys.argv = [sys.argv[0]] + remaining
    unittest.main()
