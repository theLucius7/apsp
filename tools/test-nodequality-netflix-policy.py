#!/usr/bin/env python3
"""Test only extracted Netflix functions; all HTTP traffic stays on loopback."""
import argparse
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / 'plugins/nodequality'
READONLY_SOURCES = None
TITLES = ('81280792', '70143836')
BASH = os.environ.get('APSP_NODEQUALITY_TEST_BASH', '/bin/bash')


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


policy = module('netflix_policy', PLUGIN / 'netflix-policy.py')
helper = module('netflix_source_helper', PLUGIN / 'source-helper.py')
sources = module('netflix_source_tests', ROOT / 'tools/test-nodequality-sources.py')


def body(content, name):
    start = re.search(r'(?m)^(?:function )?' + re.escape(name) + r'\(\)\{\n', content).start()
    return content[start:content.index('\n}\n', start) + 3]


def page(title, *, unavailable=False, region='US', padding=0):
    # Synthetic positive contracts, not captured evidence of a live provider.
    content = '<h1>Oh no!</h1>' if unavailable else (
        '<link rel="canonical" href="https://www.netflix.com/us/title/' + title + '">'
        '<script type="application/ld+json">{"@type":"Movie"}</script>')
    return ('<html><head><title>Fixture | Netflix</title></head><body>' + content
            + '<script>{"id":"' + region + '","countryName":"Fixture"}</script>'
            + ' ' * padding + '</body></html>')


PRELUDE = r'''
set -o pipefail
declare -A netflix sinfo smedia tiktok disney youtube amazon reddit chatgpt
sinfo=([media]=fixture [lmedia]=7)
smedia=([bad]=FAILED [yes]=SUCCESS [org]=ORIGINALS [no]=NO [nodata]=UNKNOWN)
ibar_step=0
CurlARG=''
UA_Browser='fixture-only'
show_progress_bar(){ :; }
kill_progress_bar(){ :; }
Check_DNS_1(){ :; }
Check_DNS_2(){ :; }
Check_DNS_3(){ :; }
Get_Unlock_Type(){ printf DIRECT; }
clean_ansi(){ printf '%s' "$1"; }
curl(){ python3 "$NQ_STUB" "$@"; }
'''

STUB = r'''import json, os, pathlib, subprocess, sys
args = sys.argv[1:]
title = args[-1].rsplit('/', 1)[-1]
assert title in ('81280792', '70143836'), 'unexpected request'
root = pathlib.Path(os.environ['NQ_CASE'])
with (root/'requests.jsonl').open('a') as out:
    out.write(json.dumps(args) + '\n')
if os.environ.get('NQ_LOOPBACK'):
    # Retain real curl's flags/exit/HTTP/body. Rewrite only the URL to our server.
    cmd = [os.environ['NQ_CURL'], '--disable', *args[:-1], os.environ['NQ_LOOPBACK'] + '/title/' + title]
    raise SystemExit(subprocess.run(cmd, stdin=subprocess.DEVNULL).returncode)
row = json.loads((root/'responses.json').read_text())[title]
sys.stdout.write(row.get('body', ''))
if '--write-out' in args:
    sys.stdout.write('\n' + row.get('status', '000'))
sys.stderr.write(row.get('stderr', ''))
raise SystemExit(row.get('code', 0))
'''


class BashAdmissionTests(unittest.TestCase):
    def test_unsupported_bash_skips_before_materialization_or_policy_execution(self):
        for major in ('0', '2', '3'):
            with self.subTest(major=major):
                case = NetflixTests('test_long_valid_page_survives_pipefail_and_region_is_not_interpreted')
                with mock.patch.object(subprocess, 'check_output', return_value=major) as version, \
                        mock.patch.object(case, 'production') as materialize, \
                        mock.patch.object(subprocess, 'run') as execute, \
                        self.assertRaisesRegex(unittest.SkipTest, 'require Bash 4'):
                    case.run_check(NetflixTests.valid_rows())
                version.assert_called_once_with([BASH, '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'], text=True, timeout=3)
                materialize.assert_not_called()
                execute.assert_not_called()

    def test_invalid_version_is_an_error_and_does_not_skip_the_contract(self):
        for value in ('', 'unknown', '4 extra'):
            with self.subTest(version=value):
                case = NetflixTests('test_long_valid_page_survives_pipefail_and_region_is_not_interpreted')
                with mock.patch.object(subprocess, 'check_output', return_value=value), \
                        mock.patch.object(case, 'production') as materialize, \
                        mock.patch.object(subprocess, 'run') as execute, \
                        self.assertRaisesRegex(RuntimeError, 'no valid major version'):
                    case.run_check(NetflixTests.valid_rows())
                materialize.assert_not_called()
                execute.assert_not_called()


class NetflixTests(unittest.TestCase):
    def production(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires the readonly, verified 17-file upstream source cache')
        with tempfile.TemporaryDirectory() as name:
            target = Path(name) / 'sources'
            lock = helper.decode((PLUGIN / 'source-lock.json').read_bytes())
            helper.materialize(helper.decode(helper.pack(lock, READONLY_SOURCES)), target)
            return helper.serve(target, ['-Ls', 'https://IP.Check.Place']).decode()

    def run_check(self, rows, *, original=False, repeat=False, extra='', loopback=None):
        major = subprocess.check_output([BASH, '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'],
                                        text=True, timeout=3).strip()
        if not major.isascii() or not major.isdigit():
            raise RuntimeError('selected Bash returned no valid major version')
        if int(major) < 4:
            self.skipTest('extracted upstream associative arrays require Bash 4+; selected ' + BASH + ' major ' + major)
        text = self.production()
        if original:
            text = sources.fixture.undo_netflix('ip.sh', text.encode()).decode()
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            (root/'responses.json').write_text(json.dumps(rows))
            (root/'stub.py').write_text(STUB)
            script = PRELUDE + ('' if original else policy.HELPERS.decode())
            script += body(text, 'MediaUnlockTest_Netflix') + '\n'
            script += 'netflix=([ustatus]=STALE [uregion]=STALE [utype]=STALE [reason]=STALE)\n'
            script += 'MediaUnlockTest_Netflix 4\n'
            if repeat:
                script += 'MediaUnlockTest_Netflix 4\n'
            script += r'''printf '%s\0' "${netflix[ustatus]}" "${netflix[uregion]}" "${netflix[utype]}" "${netflix[reason]}"
'''
            script += extra
            env = dict(os.environ, NQ_CASE=str(root), NQ_STUB=str(root/'stub.py'),
                       NQ_LOOPBACK=loopback or '', NQ_CURL=shutil.which('curl') or '')
            run = subprocess.run([BASH], input=script, text=True, capture_output=True, env=env, timeout=28)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertEqual(run.stderr, '')
            values = run.stdout.split('\0', 4)
            self.assertEqual(len(values), 5, run.stdout)
            calls = [json.loads(line) for line in (root/'requests.jsonl').read_text().splitlines()]
            self.assertEqual([args[-1].rsplit('/', 1)[-1] for args in calls], list(TITLES) * (2 if repeat else 1))
            return values, calls

    @staticmethod
    def valid_rows():
        return {title: {'body': page(title), 'status': '200'} for title in TITLES}

    def test_original_403_is_success_but_new_check_is_failed(self):
        rows = {title: {'code': 22, 'status': '403', 'stderr': 'curl: (22) HTTP 403\n'} for title in TITLES}
        old, _ = self.run_check(rows, original=True)
        new, _ = self.run_check(rows)
        self.assertEqual(old[0], 'SUCCESS')
        self.assertEqual(new[:3], ['FAILED', 'UNKNOWN', 'UNKNOWN'])
        self.assertEqual(new[3].count('HTTP 403'), 2)

    def test_each_transfer_failure_is_never_unlock_and_does_not_reuse_state(self):
        cases = [(6, '000', 'DNS'), (7, '000', '连接'), (18, '200', '完整'),
                 (28, '000', '超时'), (35, '000', 'TLS'), (60, '000', 'TLS'),
                 (63, '200', '大小'), (22, '429', 'HTTP 429'), (22, '500', 'HTTP 500')]
        for title in TITLES:
            for code, status, reason in cases:
                with self.subTest(title=title, code=code):
                    rows = self.valid_rows()
                    rows[title] = {'code': code, 'status': status, 'body': page(title), 'stderr': 'not HTML'}
                    values, _ = self.run_check(rows)
                    self.assertEqual(values[:3], ['FAILED', 'UNKNOWN', 'UNKNOWN'])
                    self.assertIn(reason, values[3])
                    self.assertNotIn('STALE', ''.join(values))

    def test_valid_title_originals_and_mixed_results_preserve_old_classifier(self):
        for unavailable in ((False, False), (True, True), (True, False), (False, True)):
            rows = {title: {'body': page(title, unavailable=missing), 'status': '200'}
                    for title, missing in zip(TITLES, unavailable)}
            old, _ = self.run_check(rows, original=True)
            new, calls = self.run_check(rows)
            self.assertEqual(new[:3], old[:3])
            self.assertEqual(new[0], 'ORIGINALS' if all(unavailable) else 'SUCCESS')
            self.assertIn('[US]', new[1])
            self.assertEqual(new[3], '')
            for args in calls:
                self.assertEqual(args.count('--max-time'), 1)
                self.assertEqual(args[args.index('--max-time') + 1], '10')
                self.assertEqual(args[args.index('-X') + 1], 'GET')
                self.assertNotIn('--retry', args)
                self.assertNotIn('--cookie', args)

    def test_unexpected_empty_truncated_and_wrong_title_pages_are_unknown(self):
        valid = page(TITLES[0])
        pages = ['', '  \n', '<html>403</html>', valid[:-7],
                 '<html><head><title>Netflix</title></head><body>Login</body></html>',
                 valid.replace(TITLES[0], TITLES[1]), valid.replace('"Movie"', '"WebSite"'),
                 '<html><title>Netflix</title><p>Oh no!</p></html>']
        for content in pages:
            with self.subTest(body=content[:90]):
                rows = self.valid_rows()
                rows[TITLES[0]]['body'] = content
                values, _ = self.run_check(rows)
                self.assertEqual(values[0], 'FAILED')
                self.assertTrue(values[3])
        for status in ('000', '204', '302', '403', '429', 'abc', ''):
            rows = self.valid_rows()
            rows[TITLES[0]]['status'] = status
            self.assertEqual(self.run_check(rows)[0][0], 'FAILED')

    def test_long_valid_page_survives_pipefail_and_region_is_not_interpreted(self):
        rows = {title: {'body': page(title, padding=100000, region='NOT-A-COUNTRY'), 'status': '200'} for title in TITLES}
        values, _ = self.run_check(rows)
        self.assertEqual(values[0], 'SUCCESS')
        self.assertNotIn('NOT-A-COUNTRY', values[1])
        self.assertEqual(values[3], '')

    def test_text_json_reason_and_other_media_are_retained(self):
        if shutil.which('jq') is None:
            self.skipTest('requires jq')
        text = self.production()
        updates = ''.join(re.findall(r'^media_updates\+=.*Netflix.*\n', text, re.M))
        self.assertEqual(len(updates.splitlines()), 4)
        extra = body(text, 'show_media') + '\nshow_media\n'
        extra += 'media_updates=""\n' + updates
        extra += '''printf '%s' '{"Media":{"Other":{"Status":"kept"}},"Score":{"IPQS":"0"}}' | jq "${media_updates} ."
'''
        rows = self.valid_rows()
        rows[TITLES[1]] = {'status': '429', 'code': 22}
        values, _ = self.run_check(rows, extra=extra)
        self.assertIn('Netflix：影片 70143836：HTTP 429', values[4])
        obj = json.loads(values[4][values[4].index('{'):])
        self.assertEqual(obj['Media']['Netflix']['Reason'], '影片 70143836：HTTP 429')
        self.assertEqual(obj['Media']['Netflix']['Status'], 'FAILED')
        self.assertEqual(obj['Media']['Other']['Status'], 'kept')
        self.assertEqual(obj['Score'], {'IPQS': '0'})

    def test_failed_new_task_retains_old_successful_report_and_other_chapters(self):
        report = module('netflix_report_persistence', PLUGIN / 'report.py')
        with tempfile.TemporaryDirectory() as name:
            previous, current = Path(name)/'previous-task', Path(name)/'current-task'
            previous.mkdir()
            current.mkdir()
            def files(status):
                return {'ip_quality.log': ('Netflix: ' + status).encode(),
                        'ip_quality.json': json.dumps({'Media': {'Netflix': {'Status': status}}}).encode(),
                        'hardware_quality.log': b'existing hardware chapter',
                        'hardware_quality.json': b'{"fixture":true}'}
            report.publish_sections(previous, files('SUCCESS'), archive=True)
            saved = {path.name: path.read_bytes() for path in previous.iterdir()}
            report.publish_sections(current, files('FAILED'), archive=True)
            self.assertEqual({path.name: path.read_bytes() for path in previous.iterdir()}, saved)
            self.assertEqual(json.loads((previous/'section-ip_quality.json').read_bytes())['text'], 'Netflix: SUCCESS')
            self.assertEqual(json.loads((current/'section-ip_quality.json').read_bytes())['text'], 'Netflix: FAILED')
            for task in (previous, current):
                hardware = json.loads((task/'section-hardware_quality.json').read_bytes())
                self.assertEqual(hardware['text'], 'existing hardware chapter')
                self.assertTrue(hardware['complete'])

    def http_case(self, scenarios, *, original=False, repeat=False):
        if shutil.which('curl') is None:
            self.skipTest('requires real curl')
        requests = []

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                title = self.path.rsplit('/', 1)[-1]
                requests.append(title)
                case = scenarios[title]
                if isinstance(case, list):
                    case = case[min(requests.count(title) - 1, len(case) - 1)]
                time.sleep(case.get('delay', 0))
                data = case.get('body', page(title)).encode()
                self.send_response(case.get('status', 200))
                self.send_header('Content-Type', 'text/html')
                self.send_header('Content-Length', str(len(data) + case.get('extra_length', 0)))
                self.end_headers()
                try:
                    self.wfile.write(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass

        server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            values, _ = self.run_check({}, original=original, repeat=repeat,
                                       loopback='http://127.0.0.1:' + str(server.server_port))
            self.assertEqual(requests, list(TITLES) * (2 if repeat else 1))
            return values
        finally:
            server.shutdown()
            server.server_close()
            thread.join(timeout=2)
            self.assertFalse(thread.is_alive())

    def test_real_http_negative_control_403_429_and_incomplete_body(self):
        cases = {title: {'status': 403} for title in TITLES}
        # Real curl -s suppresses stderr here, unlike the issue's diagnostic stub.
        # Retain this distinction instead of adding -S to manufacture a failure.
        self.assertEqual(self.http_case(cases, original=True)[0], 'FAILED')
        challenge = {title: {'body': '<html>Unexpected access page</html>'} for title in TITLES}
        self.assertEqual(self.http_case(challenge, original=True)[0], 'SUCCESS')
        self.assertEqual(self.http_case(challenge)[0], 'FAILED')
        for title in TITLES:
            for case, reason in [({'status': 403}, 'HTTP 403'), ({'status': 429}, 'HTTP 429'),
                                 ({'extra_length': 100}, '响应未传输完整')]:
                with self.subTest(title=title, case=case):
                    rows = {name: {} for name in TITLES}
                    rows[title] = case
                    values = self.http_case(rows)
                    self.assertEqual(values[0], 'FAILED')
                    self.assertIn(reason, values[3])

    def test_real_timeout_uses_unchanged_ten_second_budget(self):
        rows = {TITLES[0]: {'delay': 10.5}, TITLES[1]: {}}
        started = time.monotonic()
        values = self.http_case(rows)
        self.assertGreaterEqual(time.monotonic() - started, 9.5)
        self.assertEqual(values[0], 'FAILED')
        self.assertIn('请求超时', values[3])

    def test_real_success_then_failure_clears_previous_current_result(self):
        self.assertEqual(self.http_case({title: {} for title in TITLES})[0], 'SUCCESS')
        rows = {title: [{}, {'status': 429}] for title in TITLES}
        values = self.http_case(rows, repeat=True)
        self.assertEqual(values[:3], ['FAILED', 'UNKNOWN', 'UNKNOWN'])
        self.assertEqual(values[3].count('HTTP 429'), 2)

    def test_production_hashes_syntax_and_unrelated_functions_remain_unchanged(self):
        new = sources.fixture.undo_browser('ip.sh', self.production().encode())
        old = sources.fixture.undo_netflix('ip.sh', new)
        self.assertEqual(hashlib.sha256(old).hexdigest(), policy.SOURCES['ip.sh']['source_sha256'])
        self.assertEqual(policy.transform('ip.sh', old), new)
        for name in ('db_ipqs', 'db_ipregistry', 'MediaUnlockTest_YouTube_Premium', 'show_score'):
            self.assertEqual(body(new.decode(), name), body(old.decode(), name))
        run = subprocess.run([BASH, '-n'], input=new, capture_output=True, timeout=5)
        self.assertEqual(run.returncode, 0, run.stderr)
        canonical = (READONLY_SOURCES/'ip.sh').read_bytes()
        self.assertEqual(hashlib.sha256(canonical).hexdigest(), 'b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf')
        self.assertEqual(body(canonical.decode(), 'MediaUnlockTest_Netflix'), body(old.decode(), 'MediaUnlockTest_Netflix'))

    def test_helper_identity_and_output_are_fail_closed(self):
        with tempfile.TemporaryDirectory() as name:
            path = Path(name)/'netflix-policy.py'
            with mock.patch.object(helper, '__file__', str(path.with_name('source-helper.py'))):
                path.write_bytes((PLUGIN/path.name).read_bytes())
                self.assertIn('transform', helper.netflix_policy())
                for content in (path.read_bytes() + b'!', b'x' * 65537):
                    path.write_bytes(content)
                    with self.assertRaises(ValueError):
                        helper.netflix_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.netflix_policy()
                path.symlink_to(PLUGIN/path.name)
                with self.assertRaises(OSError):
                    helper.netflix_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.netflix_policy()
        for invalid in (b'wrong', 'text', b'x' * (helper.MAX_FILE + 8193)):
            with mock.patch.object(helper, 'netflix_policy', return_value={
                    'SOURCES': policy.SOURCES, 'transform': lambda role, content: invalid}):
                with self.assertRaisesRegex(ValueError, 'served Netflix policy output'):
                    helper.validated_netflix('ip.sh', b'input')

    def test_unique_anchors_source_and_output_identity(self):
        source = b''.join(before for before, _ in policy.REPLACEMENTS)
        spec = {'source_sha256': hashlib.sha256(source).hexdigest(),
                'patched_sha256': hashlib.sha256(policy.patch(source)).hexdigest()}
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

    def test_packaging_rejects_missing_or_corrupt_helper(self):
        for missing in (True, False):
            fixture = sources.SourceTests(methodName='runTest')
            fixture.setUp()
            try:
                tree, env = fixture.build_tree()
                path = tree/'plugins/nodequality/netflix-policy.py'
                if missing:
                    path.unlink()
                else:
                    # Keep valid Python and the required final newline so the
                    # fixed helper identity, rather than embedding syntax, rejects it.
                    path.write_bytes(path.read_bytes() + b'\n# altered helper bytes\n')
                run = fixture.build(tree, env, 'arm64')
                self.assertNotEqual(run.returncode, 0)
                if not missing:
                    self.assertIn(b'signed Netflix policy helper SHA256 mismatch', run.stderr)
                output = fixture.root/'artifacts/nodequality'/sources.VERSION
                self.assertFalse((output/'arm64').exists())
                self.assertFalse((output/'SHA256SUMS').exists())
            finally:
                fixture.tearDown()


if __name__ == '__main__':
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--readonly-upstream-dir', type=Path)
    args, remaining = parser.parse_known_args()
    READONLY_SOURCES = args.readonly_upstream_dir
    sys.argv = [sys.argv[0]] + remaining
    unittest.main()
