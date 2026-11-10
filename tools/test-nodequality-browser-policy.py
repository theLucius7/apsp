#!/usr/bin/env python3
"""Verify native curl identity on an owned loopback server, never on providers."""
import argparse
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import threading
import unittest
from unittest import mock

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / 'crates/observability/upstream/nodequality'
READONLY_SOURCES = None


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


policy = module('browser_policy', PLUGIN / 'browser-policy.py')
helper = module('browser_source_helper', PLUGIN / 'source-helper.py')
sources = module('browser_source_tests', ROOT / 'tools/test-nodequality-sources.py')


def request():
    args = sys.argv[2:]
    allowed = ('https://ipregistry.co', 'https://bgp.he.net/whois/ip/192.0.2.1', 'https://fixture.invalid/query')
    if not args or args[-1] not in allowed:
        raise SystemExit('fixture rejects unexpected destination')
    endpoint = os.environ['RECORDER']
    if not endpoint.startswith('http://127.0.0.1:'):
        raise SystemExit('fixture recorder must be owned loopback')
    with Path(os.environ['ARGUMENTS']).open('a') as output:
        output.write(json.dumps(args) + '\n')
    environment = {key: value for key, value in os.environ.items()
                   if key.lower() not in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
    result = subprocess.run([os.environ['REAL_CURL'], *args[:-1], endpoint], env=environment,
                            capture_output=True, timeout=4)
    sys.stdout.buffer.write(result.stdout)
    sys.stderr.buffer.write(result.stderr)
    raise SystemExit(result.returncode)


class Recorder:
    def __init__(self, status=200, payload=b'fixture-body', delay=False):
        self.requests = []
        self.release = threading.Event()
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                self.handle_request()

            def do_POST(self):
                self.handle_request()

            def handle_request(self):
                length = int(self.headers.get('Content-Length', '0'))
                if not 0 <= length <= 1024:
                    self.send_error(400)
                    return
                owner.requests.append({'method': self.command, 'headers': {k.lower(): v for k, v in self.headers.items()},
                                       'body': self.rfile.read(length).decode()})
                if delay:
                    owner.release.wait(1)
                    return
                self.send_response(status)
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args):
                pass

        self.server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.url = 'http://127.0.0.1:' + str(self.server.server_port) + '/query'

    def close(self):
        self.release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        if self.thread.is_alive():
            raise AssertionError('loopback recorder failed to stop')


class BrowserTests(unittest.TestCase):
    def runtime(self):
        major = subprocess.check_output(['/bin/bash', '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'], text=True)
        if int(major) < 4:
            self.skipTest('fixed scripts require Bash >= 4; run on dedicated Debian')
        if not shutil.which('curl'):
            self.skipTest('requires real curl')

    def production(self, role):
        if READONLY_SOURCES is None:
            self.skipTest('requires previously verified readonly 17-file cache')
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name) / 'sources'
            helper.materialize(helper.decode(helper.pack(helper.decode((PLUGIN / 'source-lock.json').read_bytes()), READONLY_SOURCES)), directory)
            return helper.serve(directory, ['-Ls', {'ip.sh': 'https://IP.Check.Place', 'net.sh': 'https://Net.Check.Place'}[role]])

    def run_curl(self, arguments, *, wrapped=True, child=False, status=200, delay=False, expression=None):
        self.runtime()
        recorder = Recorder(status=status, delay=delay)
        try:
            with tempfile.TemporaryDirectory() as name:
                directory = Path(name)
                binary = directory / 'curl'
                binary.write_text('#!/bin/sh\nexec "$PYTHON" "$TOOL" --request "$@"\n')
                binary.chmod(0o700)
                curl_home = directory / 'curl-home'
                curl_home.mkdir()
                (curl_home / '.curlrc').write_text('user-agent = "fixture-curlrc-browser"\nheader = "Sec-CH-UA: fixture-curlrc-browser"\n')
                environment = dict(os.environ, PATH=str(directory) + ':' + os.environ['PATH'],
                                   PYTHON=sys.executable, TOOL=str(Path(__file__).resolve()), RECORDER=recorder.url,
                                   ARGUMENTS=str(directory / 'arguments.jsonl'), REAL_CURL=shutil.which('curl'), CURL_HOME=str(curl_home))
                script = policy.WRAPPER if wrapped else b''
                script += b'UA_Browser="Mozilla/5.0 fixture-browser"\nIP=192.0.2.1\nCurlARG=""\n'
                if expression is not None:
                    script += expression + b'\n'
                else:
                    script += b"bash -c 'curl \"$@\"' child \"$@\"\n" if child else b'curl "$@"\n'
                run = subprocess.run(['/bin/bash', '-s', '--', *arguments], input=script, env=environment,
                                     capture_output=True, timeout=8)
                path = directory / 'arguments.jsonl'
                args = [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []
                return run, list(recorder.requests), args
        finally:
            recorder.close()

    def assert_native(self, records):
        self.assertEqual(len(records), 1)
        headers = records[0]['headers']
        self.assertTrue(headers['user-agent'].startswith('curl/'), headers)
        self.assertFalse(any(key.startswith(('sec-ch-ua', 'sec-fetch-')) for key in headers), headers)

    def test_actual_ip_and_net_request_expressions_keep_arguments_with_native_identity(self):
        self.runtime()
        for role, prefix in [('ip.sh', b'REGISTRY_HTML=$(curl '), ('net.sh', b'RESPONSE=$(curl $CurlARG -$1 --user-agent "$UA_Browser"')]:
            text = self.production(role)
            line = next(line for line in text.splitlines() if line.startswith(prefix) and (role == 'ip.sh' or b'https://bgp.he.net/' in line))
            expression = line[line.index(b'$(curl ') + 2:-1]
            before, old_records, old_args = self.run_curl(['4'], wrapped=False, expression=expression)
            after, records, args = self.run_curl(['4'], expression=expression)
            self.assertEqual(before.returncode, 0, before.stderr)
            self.assertEqual(after.returncode, 0, after.stderr)
            self.assertEqual(before.stdout, after.stdout)
            self.assertIn('Mozilla/', old_records[0]['headers']['user-agent'])
            self.assert_native(records)
            self.assertEqual(args[0][0], '-q')
            expected = old_args[0].copy()
            flag = '-H' if role == 'ip.sh' else '--user-agent'
            index = expected.index(flag)
            del expected[index:index + 2]
            self.assertEqual(args[0][1:], expected)

    def test_options_headers_curlrc_and_child_shell_keep_method_body_and_other_headers(self):
        self.runtime()
        arguments = ['-sS', '--max-time', '2', '-X', 'POST', '--data-binary', '{"fixture":"with spaces"}',
                     '--user-agent', 'Mozilla/first', '--user-agent=Mozilla/second', '-A', 'Mozilla/third', '-AMozilla/fourth',
                     '-H', 'User-Agent: Mozilla/header', '--header=uSeR-aGeNt: Mozilla/header2', '-HUser-Agent;',
                     '-H', 'Sec-CH-UA: fixture-browser', '--header', 'Sec-CH-UA-Platform: fixture-os',
                     '-Hsec-fetch-site:same-origin', '-H', 'Content-Type: application/json',
                     '-H', 'X-Fixture: retained with spaces', 'https://fixture.invalid/query']
        for child in (False, True):
            run, records, args = self.run_curl(arguments, child=child)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertEqual(run.stdout, b'fixture-body')
            self.assert_native(records)
            self.assertEqual(records[0]['method'], 'POST')
            self.assertEqual(records[0]['body'], '{"fixture":"with spaces"}')
            self.assertEqual(records[0]['headers']['x-fixture'], 'retained with spaces')
            self.assertEqual(records[0]['headers']['content-type'], 'application/json')
            self.assertEqual(len(args), 1)

    def test_http_failure_and_timeout_keep_curl_result_without_new_attempts(self):
        self.runtime()
        for status, delay, expected in ((403, False, 22), (429, False, 22), (200, True, 28), (200, False, 0)):
            arguments = ['-fsS', '--max-time', '0.2' if delay else '2', '--user-agent', 'Mozilla/fixture', 'https://fixture.invalid/query']
            run, records, args = self.run_curl(arguments, status=status, delay=delay)
            self.assertEqual(run.returncode, expected, run.stderr)
            self.assert_native(records)
            self.assertEqual(len(args), 1)
            self.assertEqual(run.stdout, b'fixture-body' if expected == 0 else b'')
            if expected:
                self.assertIn(str(expected).encode(), run.stderr)

    def test_missing_values_header_files_config_and_missing_native_binary_fail_before_network(self):
        self.runtime()
        for arguments, expected in [(['--user-agent'], 2), (['--header'], 2), (['-H', '@fixture'], 70),
                                    (['--header=@fixture'], 70), (['--config', 'fixture'], 70), (['-Kfixture'], 70)]:
            run, records, args = self.run_curl(arguments)
            self.assertEqual(run.returncode, expected, run.stderr)
            self.assertEqual(records, [])
            self.assertEqual(args, [])
        run = subprocess.run(['/bin/bash'], input=policy.WRAPPER, env={'PATH': ''}, capture_output=True, timeout=3)
        self.assertEqual(run.returncode, 70)
        self.assertIn(b'native curl is required', run.stderr)

    def test_production_identity_and_generator_disable_leave_other_bytes_unchanged(self):
        for role in policy.SOURCES:
            new = self.production(role)
            if role == 'ip.sh':
                new = sources.fixture.undo_access(role, new)
            prior = sources.fixture.undo_browser(role, new)
            self.assertEqual(hashlib.sha256(prior).hexdigest(), policy.SOURCES[role]['source_sha256'])
            self.assertEqual(policy.transform(role, prior), new)
            self.assertNotIn(b'\ngenerate_random_user_agent\n', new)
            self.assertEqual(new.count(policy.WRAPPER), 1)
            # Existing generator definition is retained but never called. No request URL,
            # credential, cookie, response parser or non-browser header is rewritten.
            self.assertEqual([s for s in prior.splitlines() if b'curl ' in s], [s for s in new.replace(policy.WRAPPER, b'').splitlines() if b'curl ' in s])

    def test_identity_anchors_and_output_fail_closed(self):
        source = b''.join(before for before, _ in policy.REPLACEMENTS)
        spec = {'source_sha256': hashlib.sha256(source).hexdigest(), 'patched_sha256': hashlib.sha256(policy.patch(source)).hexdigest()}
        with mock.patch.dict(policy.SOURCES, {'ip.sh': spec}):
            self.assertEqual(policy.transform('ip.sh', source), policy.patch(source))
            for invalid in (source + b'!', 'text', b'x' * (policy.MAX_SOURCE + 1)):
                with self.assertRaises(ValueError):
                    policy.transform('ip.sh', invalid)
            with self.assertRaisesRegex(ValueError, 'unique'):
                policy.patch(source + source)
            with mock.patch.dict(spec, patched_sha256='0' * 64):
                with self.assertRaisesRegex(ValueError, 'output SHA256'):
                    policy.transform('ip.sh', source)
        with self.assertRaises(ValueError):
            policy.transform('unknown', b'')

    def test_helper_file_boundaries_and_invalid_return(self):
        with tempfile.TemporaryDirectory() as name:
            path = Path(name) / 'browser-policy.py'
            content = (PLUGIN / path.name).read_bytes()
            with mock.patch.object(helper, '__file__', str(path.with_name('source-helper.py'))):
                path.write_bytes(content)
                self.assertIn('transform', helper.browser_policy())
                for data in (content + b'!', b'x' * 65537):
                    path.write_bytes(data)
                    with self.assertRaises(ValueError):
                        helper.browser_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.browser_policy()
                path.symlink_to(PLUGIN / path.name)
                with self.assertRaises(OSError):
                    helper.browser_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.browser_policy()
        for invalid in (b'changed', 'text', b'x' * (helper.MAX_FILE + 8193)):
            with mock.patch.object(helper, 'browser_policy', return_value={
                    'SOURCES': policy.SOURCES, 'transform': lambda role, content: invalid}):
                with self.assertRaisesRegex(ValueError, 'served browser policy output'):
                    helper.native_curl_identity('ip.sh', b'input')

    def test_builder_rejects_missing_or_corrupt_helper(self):
        for missing in (True, False):
            fixture = sources.SourceTests(methodName='runTest')
            fixture.setUp()
            try:
                tree, env = fixture.build_tree()
                path = tree / 'crates/observability/upstream/nodequality/browser-policy.py'
                if missing:
                    path.unlink()
                else:
                    path.write_bytes(path.read_bytes() + b'!')
                run = fixture.build(tree, env, 'arm64')
                self.assertNotEqual(run.returncode, 0)
                output = fixture.root / 'artifacts/nodequality' / sources.VERSION
                self.assertFalse((output / 'arm64').exists())
                self.assertFalse((output / 'SHA256SUMS').exists())
            finally:
                fixture.tearDown()


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--request':
        request()
    else:
        parser = argparse.ArgumentParser(add_help=False)
        parser.add_argument('--readonly-upstream-dir', type=Path)
        args, remaining = parser.parse_known_args()
        READONLY_SOURCES = args.readonly_upstream_dir
        sys.argv = [sys.argv[0]] + remaining
        unittest.main()
