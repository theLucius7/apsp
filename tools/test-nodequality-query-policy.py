#!/usr/bin/env python3
"""Exercise pinned Netflix parsing against inert failures and owned HTTP only."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
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


policy = module('query_policy', PLUGIN / 'query-policy.py')
helper = module('query_helper', PLUGIN / 'native-source-helper.py')
sources = module('query_sources', ROOT / 'tools/test-nodequality-native-sources.py')
browser = module('query_browser', ROOT / 'tools/test-nodequality-native-browser-policy.py')

PRELUDE = r'''
declare -A netflix sinfo smedia
sinfo[media]=fixture
sinfo[lmedia]=1
smedia[yes]=YES
smedia[org]=ORIGINAL
smedia[no]=NO
smedia[bad]=BAD
smedia[nodata]=''
IP=192.0.2.1
CurlARG=''
show_progress_bar(){ :; }
kill_progress_bar(){ :; }
Check_DNS_1(){ :; }
Check_DNS_2(){ :; }
Check_DNS_3(){ :; }
Get_Unlock_Type(){ printf DIRECT; }
clean_ansi(){ printf '%s' "$1"; }
'''
KNOWN = '<html>{"id":"US","countryName":"United States"}</html>'


def body(text, name):
    start = text.index(name + '(){\n')
    return text[start:text.index('\n}\n', start) + 3]


def request():
    args = sys.argv[2:]
    allowed = ('https://www.netflix.com/title/81280792', 'https://www.netflix.com/title/70143836')
    if not args or args[-1] not in allowed:
        raise SystemExit('fixture rejects unexpected destination')
    path = Path(os.environ['ARGUMENTS'])
    records = path.read_text().splitlines() if path.exists() else []
    with path.open('a') as output:
        output.write(json.dumps(args) + '\n')
    scenarios = json.loads(os.environ['SCENARIOS'])
    scenario = scenarios[min(len(records), len(scenarios) - 1)]
    if scenario['kind'] == 'stub':
        # The original bug reads this stderr as if it were an unlocked page.
        print('curl: (' + str(scenario['code']) + ') fixture transport failure', file=sys.stderr)
        sys.stdout.write(scenario.get('body', ''))
        if '--write-out' in args:
            sys.stdout.write('\n' + str(scenario.get('status', '000')) + '\n0.001')
        raise SystemExit(scenario['code'])
    endpoint = os.environ['RECORDER']
    if not endpoint.startswith('http://127.0.0.1:'):
        raise SystemExit('fixture requires owned loopback')
    # The production request keeps its ten-second ceiling. Only this test
    # forwarding process shortens it to avoid a ten-second timeout fixture.
    environment = {k: v for k, v in os.environ.items()
                   if k.lower() not in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
    run = subprocess.run([os.environ['REAL_CURL'], '-q', *args[:-1], '--max-time', '0.2', endpoint],
                         env=environment, capture_output=True, timeout=3)
    sys.stdout.buffer.write(run.stdout)
    sys.stderr.buffer.write(run.stderr)
    raise SystemExit(run.returncode)


class QueryTests(unittest.TestCase):
    def runtime(self):
        major = subprocess.check_output(['/bin/bash', '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'], text=True)
        if int(major) < 4:
            self.skipTest('pinned associative arrays require Bash >= 4 on dedicated Debian')
        if not shutil.which('jq') or not shutil.which('curl'):
            self.skipTest('requires real jq and curl')

    def production(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires verified readonly 17-file source cache')
        with tempfile.TemporaryDirectory() as name:
            target = Path(name) / 'sources'
            helper.materialize(helper.decode(helper.pack(helper.decode((PLUGIN / 'source-lock.json').read_bytes()), READONLY_SOURCES)), target)
            return sources.fixture.serve_before_access(helper, target, ['-Ls', 'https://IP.Check.Place'])

    def query(self, scenarios, *, original=False, status=200, delay=False, payload=KNOWN):
        self.runtime()
        content = self.production()
        if original:
            content = sources.fixture.undo_queries('ip.sh', content)
        text = content.decode()
        recorder = browser.Recorder(status=status, delay=delay, payload=payload.encode())
        try:
            with tempfile.TemporaryDirectory() as name:
                directory = Path(name)
                binary = directory / 'curl'
                binary.write_text('#!/bin/sh\nexec "$PYTHON" "$TOOL" --request "$@"\n')
                binary.chmod(0o700)
                env = dict(os.environ, PATH=str(directory) + ':' + os.environ['PATH'],
                           PYTHON=sys.executable, TOOL=str(Path(__file__).resolve()), SCENARIOS=json.dumps(scenarios),
                           RECORDER=recorder.url, ARGUMENTS=str(directory / 'arguments.jsonl'), REAL_CURL=shutil.which('curl'))
                script = PRELUDE + ('' if original else policy.HELPERS.decode()) + body(text, 'MediaUnlockTest_Netflix')
                # Only the original function and four Netflix JSON fields execute.
                updates = ''.join(line + '\n' for line in text.splitlines() if line.startswith('media_updates+=') and 'Netflix' in line)
                script += '\nMediaUnlockTest_Netflix 4\nmedia_updates=""\n' + updates
                script += '\nprintf \'%s\' \'{"Media":{"Other":"retained"}}\' | jq -c "${media_updates} ."\n'
                run = subprocess.run(['/bin/bash'], input=script.encode(), env=env, capture_output=True, timeout=5)
                self.assertEqual(run.returncode, 0, run.stderr)
                self.assertEqual(run.stderr, b'')
                rows = run.stdout.decode().splitlines()
                result = json.loads(rows[-1])
                self.assertEqual(result['Media']['Other'], 'retained')
                args_path = directory / 'arguments.jsonl'
                args = [json.loads(line) for line in args_path.read_text().splitlines()]
                return result['Media']['Netflix'], rows[:-1], args, list(recorder.requests)
        finally:
            recorder.close()

    def test_original_403_is_success_but_dns_connect_tls_timeout_and_http_are_unknown(self):
        self.runtime()
        old, _, args, records = self.query([{'kind': 'stub', 'code': 22, 'status': 403}], original=True)
        self.assertEqual(old['Status'], 'YES')
        self.assertEqual(len(args), 2)
        self.assertEqual(records, [])
        for code, status, category in ((6, '000', 'dns'), (7, '000', 'connection'), (28, '000', 'timeout'),
                                       (35, '000', 'tls'), (60, '000', 'tls'), (22, 403, 'http_403'),
                                       (22, 429, 'http_429'), (22, 500, 'transport')):
            with self.subTest(code=code, status=status):
                result, text, args, records = self.query([{'kind': 'stub', 'code': code, 'status': status}])
                self.assertEqual(result['Status'], '未知')
                self.assertEqual(result['Error']['category'], category)
                self.assertIn('Netflix：未知', text[0])
                self.assertEqual(len(args), 1, 'stop on failure without retrying')
                self.assertEqual(records, [])
                attempt = result['Attempts'][0]
                self.assertEqual(attempt['error'], category)
                self.assertEqual(attempt['target_ip'], '192.0.2.1')
                self.assertGreater(attempt['attempted_at'], 0)
                self.assertEqual(attempt['elapsed_seconds'], 0.001)
                self.assertEqual(attempt['curl_exit'], code)

    def test_actual_curl_403_429_timeout_and_unrecognized_200_are_unknown(self):
        self.runtime()
        for status, delay, payload, category in ((403, False, KNOWN, 'http_403'), (429, False, KNOWN, 'http_429'),
                                                (200, True, KNOWN, 'timeout'), (200, False, '<html>login</html>', 'schema_mismatch'),
                                                (200, False, '', 'empty_response')):
            result, _, args, requests = self.query([{'kind': 'http'}], status=status, delay=delay, payload=payload)
            self.assertEqual(result['Status'], '未知')
            self.assertEqual(result['Error']['category'], category)
            expected_count = 2 if category == 'schema_mismatch' else 1
            self.assertEqual(len(args), expected_count)
            self.assertEqual(len(requests), expected_count)
            for argv in args:
                self.assertEqual(argv[argv.index('--max-time') + 1], '10')
                self.assertNotIn('--retry', argv)
                self.assertNotIn('--user-agent', argv)

    def test_valid_transport_preserves_existing_heuristic_and_later_failure_is_unknown(self):
        self.runtime()
        for payload, expected in ((KNOWN, 'YES'), (KNOWN + 'Oh no!', 'ORIGINAL')):
            result, _, args, requests = self.query([{'kind': 'http'}], payload=payload)
            self.assertEqual(result['Status'], expected)
            self.assertIsNone(result['Error'])
            self.assertEqual(len(result['Attempts']), 2)
            self.assertEqual(len(args), 2)
            self.assertEqual(len(requests), 2)
            self.assertEqual(result['Region'].strip(), 'US')
        for second, category in (({'kind': 'stub', 'code': 22, 'status': 429}, 'http_429'),
                                 ({'kind': 'stub', 'code': 0, 'status': 200, 'body': KNOWN.replace('US', 'JP')}, 'schema_mismatch')):
            result, _, args, _ = self.query([{'kind': 'http'}, second])
            self.assertEqual(result['Status'], '未知')
            self.assertEqual(result['Error']['category'], category)
            self.assertEqual(len(args), 2)

    def test_fixed_input_output_and_other_functions_unchanged(self):
        new = self.production()
        old = sources.fixture.undo_queries('ip.sh', new)
        self.assertEqual(hashlib.sha256(old).hexdigest(), policy.SOURCES['ip.sh']['source_sha256'])
        self.assertEqual(policy.transform('ip.sh', old), new)
        for name in ('db_ipinfo', 'db_ipregistry', 'MediaUnlockTest_DisneyPlus', 'show_score'):
            self.assertEqual(body(old.decode(), name), body(new.decode(), name))

    def test_identity_anchors_and_bounds(self):
        sample = b''.join(before for before, _ in policy.REPLACEMENTS)
        spec = {'source_sha256': hashlib.sha256(sample).hexdigest(), 'patched_sha256': hashlib.sha256(policy.patch(sample)).hexdigest()}
        with mock.patch.dict(policy.SOURCES, {'ip.sh': spec}):
            self.assertEqual(policy.transform('ip.sh', sample), policy.patch(sample))
            for bad in (sample + b'!', 'text', b'x' * (policy.MAX_SOURCE + 1)):
                with self.assertRaises(ValueError):
                    policy.transform('ip.sh', bad)
            with self.assertRaisesRegex(ValueError, 'unique'):
                policy.patch(sample + sample)
            with mock.patch.dict(spec, patched_sha256='0' * 64):
                with self.assertRaises(ValueError):
                    policy.transform('ip.sh', sample)
        with self.assertRaises(ValueError):
            policy.transform('net.sh', sample)

    def test_helper_file_and_return_boundaries(self):
        with tempfile.TemporaryDirectory() as name:
            path = Path(name) / 'query-policy.py'
            content = (PLUGIN / path.name).read_bytes()
            with mock.patch.object(helper, '__file__', str(path.with_name('native-source-helper.py'))):
                path.write_bytes(content)
                self.assertIn('transform', helper.query_policy())
                for data in (content + b'!', b'x' * 65537):
                    path.write_bytes(data)
                    with self.assertRaises(ValueError):
                        helper.query_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.query_policy()
                path.symlink_to(PLUGIN / path.name)
                with self.assertRaises(OSError):
                    helper.query_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.query_policy()
        for invalid in (b'changed', 'text', b'x' * (helper.MAX_FILE + 4097)):
            with mock.patch.object(helper, 'query_policy', return_value={'SOURCES': policy.SOURCES, 'transform': lambda role, data: invalid}):
                with self.assertRaisesRegex(ValueError, 'served query policy output'):
                    helper.validated_query_results('ip.sh', b'input')


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--request':
        request()
    parser = argparse.ArgumentParser()
    parser.add_argument('--readonly-upstream-dir', type=Path)
    args, remaining = parser.parse_known_args()
    READONLY_SOURCES = args.readonly_upstream_dir
    unittest.main(argv=[sys.argv[0], *remaining])
