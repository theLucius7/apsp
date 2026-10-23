#!/usr/bin/env python3
"""Verify credential-free source policy with inert stubs and owned HTTP only."""
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
BASH = os.environ.get('APSP_NODEQUALITY_TEST_BASH', '/bin/bash')


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


policy = module('access_policy_test', PLUGIN / 'access-policy.py')
helper = module('access_helper_test', PLUGIN / 'native-source-helper.py')
browser = module('access_browser_test', ROOT / 'tools/test-nodequality-native-browser-policy.py')

PRELUDE = r'''
declare -A ipregistry dbip disney youtube smedia
smedia[yes]=YES
smedia[cn]=CN
smedia[noprem]=NO_PREMIUM
smedia[nodata]=''
IP=192.0.2.1
CurlARG=''
rawgithub=https://fixture.invalid/reference/
Check_DNS_1(){ :; }
Check_DNS_3(){ :; }
Get_Unlock_Type(){ printf DIRECT; }
clean_ansi(){ printf '%s' "$1"; }
'''
KNOWN = '<html><head><title>YouTube Premium - YouTube</title></head><body>{"contentRegion":"US"} ad-free</body></html>'


def body(text, name):
    matches = list(re.finditer(r'(?m)^(?:function )?' + re.escape(name) + r'\(\)\{\n[^\0]*?^\}\n', text))
    if len(matches) != 1:
        raise AssertionError('fixture requires unique function: ' + name)
    return matches[0].group()


def request():
    args = sys.argv[2:]
    if not args or args[-1] != 'https://www.youtube.com/premium':
        raise SystemExit('fixture rejects unexpected destination')
    with Path(os.environ['ARGUMENTS']).open('a') as output:
        output.write(json.dumps(args) + '\n')
    scenario = json.loads(os.environ['SCENARIO'])
    if scenario['kind'] == 'stub':
        print('curl: fixture transport failure', file=sys.stderr)
        sys.stdout.write(scenario.get('body', ''))
        if 'body_size' in scenario:
            sys.stdout.write('x' * scenario['body_size'])
        if '--write-out' in args and not scenario.get('omit_footer'):
            sys.stdout.write('\nAPSP_HTTP:' + str(scenario.get('status', '000')) + '\nAPSP_TIME:0.001')
        raise SystemExit(scenario['code'])
    endpoint = os.environ['RECORDER']
    if not endpoint.startswith('http://127.0.0.1:'):
        raise SystemExit('fixture requires owned loopback')
    environment = {key: value for key, value in os.environ.items()
                   if key.lower() not in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
    # Only the forwarding fixture shortens this timeout. The production argv
    # is captured above with its unchanged ten-second ceiling.
    forwarded = 0
    broken = False
    start = time.monotonic()
    timeout = '10' if scenario.get('streaming') else '0.2'
    forwarded_args = args[:-1]
    if scenario.get('ignore_filesize_for_fixture'):
        index = forwarded_args.index('--max-filesize')
        forwarded_args = forwarded_args[:index] + forwarded_args[index + 2:]
    child = subprocess.Popen([os.environ['REAL_CURL'], *forwarded_args, '--max-time', timeout, endpoint],
                             env=environment, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    with Path(os.environ['PROCESSES']).open('a') as output:
        output.write(json.dumps({'event': 'started', 'proxy_pid': os.getpid(), 'curl_pid': child.pid}) + '\n')
    try:
        while chunk := child.stdout.read(16384):
            # Forward as bytes arrive. Buffering the complete real curl result
            # here would hide a missing bound in the production reader.
            try:
                position = 0
                while position < len(chunk):
                    position += os.write(1, chunk[position:])
                forwarded += len(chunk)
            except BrokenPipeError:
                broken = True
                child.terminate()
                break
        code = child.wait(timeout=11)
    finally:
        child.stdout.close()
        if child.poll() is None:
            child.kill()
            child.wait(timeout=3)
    with Path(os.environ['PROCESSES']).open('a') as output:
        output.write(json.dumps({'event': 'reaped', 'proxy_pid': os.getpid(), 'curl_pid': child.pid,
                                 'curl_exit': code, 'broken_pipe': broken, 'forwarded_bytes': forwarded,
                                 'duration_seconds': time.monotonic() - start}) + '\n')
    raise SystemExit(23 if broken else code)


class ChunkedRecorder:
    """Send at most four MiB from an owned server with no Content-Length."""
    def __init__(self):
        self.requests = []
        self.bytes_written = 0
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def do_GET(self):
                owner.requests.append({'method': 'GET', 'headers': {k.lower(): v for k, v in self.headers.items()}, 'body': ''})
                self.send_response(200)
                self.send_header('Transfer-Encoding', 'chunked')
                self.send_header('Connection', 'close')
                self.end_headers()
                chunk = b'x' * 16384
                try:
                    for _ in range(256):
                        self.wfile.write(b'4000\r\n' + chunk + b'\r\n')
                        self.wfile.flush()
                        owner.bytes_written += len(chunk)
                    self.wfile.write(b'0\r\n\r\n')
                    self.wfile.flush()
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def log_message(self, *args):
                pass

        self.server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
        self.server.handle_error = lambda request, address: None
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.url = 'http://127.0.0.1:' + str(self.server.server_port) + '/chunked'

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        if self.thread.is_alive():
            raise AssertionError('owned chunked recorder did not stop')


class AccessTests(unittest.TestCase):
    def runtime(self):
        major = subprocess.check_output([BASH, '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'], text=True)
        if int(major) < 4:
            self.skipTest('fixed associative arrays require Bash >= 4 on dedicated Debian')
        if not shutil.which('jq') or not shutil.which('curl'):
            self.skipTest('requires real jq and curl')

    def production(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires verified readonly 17-file source cache')
        with tempfile.TemporaryDirectory() as name:
            target = Path(name) / 'sources'
            bundle = helper.decode(helper.pack(helper.decode((PLUGIN / 'source-lock.json').read_bytes()), READONLY_SOURCES))
            helper.materialize(bundle, target)
            result = browser.sources.fixture.serve_before_netflix(helper, target, ['-Ls', 'https://IP.Check.Place'])
            self.assertEqual(hashlib.sha256(result).hexdigest(), policy.SOURCES['ip.sh']['patched_sha256'])
            return result

    def youtube(self, scenario, *, status=200, delay=False, payload=KNOWN, child=False, chunked=False, fail_reader=None, strict=False):
        self.runtime()
        text = self.production().decode()
        recorder = ChunkedRecorder() if chunked else browser.Recorder(status=status, delay=delay, payload=payload.encode())
        # A size-limited client may close before this owned server writes the
        # entire response; suppress only the expected fixture socket traceback.
        recorder.server.handle_error = lambda request, address: None
        try:
            with tempfile.TemporaryDirectory() as name:
                directory = Path(name)
                binary = directory / 'curl'
                binary.write_text('#!/bin/sh\nexec "$PYTHON" "$TOOL" --request "$@"\n')
                binary.chmod(0o700)
                if fail_reader:
                    (directory / fail_reader).write_text('#!/bin/sh\nexit 73\n')
                    (directory / fail_reader).chmod(0o700)
                curl_home = directory / 'curl-home'
                curl_home.mkdir()
                (curl_home / '.curlrc').write_text('cookie = "fixture-curlrc-cookie=must-not-send"\nuser-agent = "fixture-browser"\n')
                environment = dict(os.environ, PATH=str(directory) + ':' + os.environ['PATH'],
                                   PYTHON=sys.executable, TOOL=str(Path(__file__).resolve()), SCENARIO=json.dumps(scenario),
                                   RECORDER=recorder.url, ARGUMENTS=str(directory / 'arguments.jsonl'),
                                   PROCESSES=str(directory / 'processes.jsonl'),
                                   REAL_CURL=shutil.which('curl'), CURL_HOME=str(curl_home))
                script = PRELUDE + browser.policy.WRAPPER.decode() + policy.HELPERS.decode()
                script += body(text, 'MediaUnlockTest_YouTube_Premium')
                # Reuse the actual report metadata insertion from the served
                # script; unrelated report fields must survive the operation.
                metadata = policy.JSON_ADDITION.decode()
                script += '\nMediaUnlockTest_YouTube_Premium 4\n'
                script += '\nipjson=\'{"Media":{"Youtube":{},"Other":"retained"},"Score":{"Other":42}}\'\n'
                updates = '\n'.join(line for line in text.splitlines() if line.startswith('media_updates+=') and 'Youtube' in line)
                script += 'media_updates=""\n' + updates + '\n'
                script += 'ipjson=$(printf "%s" "$ipjson"|jq "$media_updates.")\n'
                script += metadata + '\nprintf "%s" "$ipjson"|jq -c .\n'
                if child:
                    # The production native curl wrapper is exported to child
                    # shells; verify its identity separately below.
                    script = PRELUDE + browser.policy.WRAPPER.decode() + "bash -c 'curl -fsS --max-time 10 --write-out \"\\n%{http_code}\\n%{time_total}\" https://www.youtube.com/premium'\n"
                arguments = [BASH]
                if strict:
                    arguments += ['-e', '-o', 'pipefail']
                    script = 'if shopt -s inherit_errexit 2>/dev/null;then :;else :;fi\n' + script
                run = subprocess.run(arguments, input=script.encode(), env=environment, capture_output=True, timeout=15)
                self.assertEqual(run.returncode, 0, run.stderr)
                self.assertEqual(run.stderr, b'')
                arguments = directory / 'arguments.jsonl'
                rows = [json.loads(line) for line in arguments.read_text().splitlines()] if arguments.exists() else []
                process_path = directory / 'processes.jsonl'
                process_rows = [json.loads(line) for line in process_path.read_text().splitlines()] if process_path.exists() else []
                self.last_process_rows = process_rows
                self.last_chunked_bytes = recorder.bytes_written if chunked else None
                for started in (row for row in process_rows if row['event'] == 'started'):
                    completed = [row for row in process_rows if row['event'] == 'reaped' and row['curl_pid'] == started['curl_pid']]
                    self.assertEqual(len(completed), 1, process_rows)
                    for pid in (started['proxy_pid'], started['curl_pid']):
                        with self.assertRaises(ProcessLookupError):
                            os.kill(pid, 0)
                if child:
                    return None, [], rows, list(recorder.requests)
                lines = run.stdout.decode().splitlines()
                result = json.loads(lines[-1])
                self.assertEqual(result['Media']['Other'], 'retained')
                self.assertEqual(result['Score']['Other'], 42)
                return result['Media']['Youtube'], lines[:-1], rows, list(recorder.requests)
        finally:
            recorder.close()

    def assert_anonymous_once(self, argv, records):
        self.assertEqual(len(argv), 1)
        self.assertEqual(argv[0][0], '-q')
        self.assertEqual(argv[0][argv[0].index('--max-time') + 1], '10')
        if '--max-filesize' in argv[0]:
            self.assertEqual(argv[0][argv[0].index('--max-filesize') + 1], '2097152')
        for option in ('-b', '--cookie', '--cookie-jar', '--retry', '--user-agent'):
            self.assertNotIn(option, argv[0])
        if records:
            self.assertEqual(len(records), 1)
            headers = records[0]['headers']
            self.assertNotIn('cookie', headers)
            self.assertNotIn('authorization', headers)
            self.assertTrue(headers['user-agent'].startswith('curl/'))

    def test_unconfigured_sources_are_unknown_without_requests_or_false_scores(self):
        self.runtime()
        text = self.production().decode()
        with tempfile.TemporaryDirectory() as name:
            called = Path(name) / 'network-called'
            environment = dict(os.environ, CALLED=str(called))
            script = PRELUDE + policy.HELPERS.decode()
            script += 'curl(){ printf called > "$CALLED";return 97; }\n'
            for function in ('db_ipregistry', 'db_dbip', 'MediaUnlockTest_DisneyPlus', 'read_ref'):
                script += body(text, function)
            script += '\nread_ref\ndb_ipregistry 4\ndb_dbip 4\nMediaUnlockTest_DisneyPlus 4\n'
            self.assertNotIn('curl ', body(text, 'read_ref'))
            script += '[[ -z $Media_Cookie ]] || exit 98\n'
            score = next(line for line in text.splitlines() if line.startswith('score_updates+=') and 'DBIP' in line)
            factors = [line for line in text.splitlines() if line.startswith('factor_updates+=') and ('"DBIP"' in line or '"ipregistry"' in line)]
            media = [line for line in text.splitlines() if line.startswith('media_updates+=') and 'DisneyPlus' in line]
            script += body(text, 'factor_bool')
            script += body(text, 'apsp_ip_score_json')
            script += '\nscore_updates=""\nfactor_updates=""\nmedia_updates=""\n'
            script += score + '\n' + '\n'.join(factors + media) + '\n'
            script += 'ipjson=\'{"Media":{"Other":"retained"},"Score":{"Other":42},"Factor":{}}\'\n'
            script += 'ipjson=$(printf "%s" "$ipjson"|jq "$score_updates$factor_updates$media_updates.")\n'
            script += policy.JSON_ADDITION.decode() + '\nprintf "%s" "$ipjson"|jq -c .\n'
            result = subprocess.run([BASH], input=script.encode(), env=environment, capture_output=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stderr, b'')
            self.assertFalse(called.exists())
            rows = result.stdout.decode().splitlines()
            report = json.loads(rows[-1])
            self.assertEqual(report['Score']['DBIP'], None)
            self.assertEqual(report['Score']['Other'], 42)
            self.assertEqual(report['Media']['DisneyPlus']['Status'], '未知')
            self.assertEqual(report['Media']['Other'], 'retained')
            for values in report['Factor'].values():
                self.assertTrue(all(value is None for value in values.values()), values)
            for source in (report['Sources']['ipregistry'], report['Sources']['DBIP'], report['Media']['DisneyPlus']):
                self.assertEqual(source['status'], 'unknown')
                self.assertFalse(source['Attempted'])
                self.assertIsNone(source['last_attempt_at'])
                self.assertIsNone(source['elapsed_seconds'])
                self.assertEqual(source['Attempts'], [])
                self.assertEqual(source['Error']['category'], 'credential_not_configured')
                self.assertEqual(source['Error']['phase'], 'not_attempted')
                self.assertEqual(source['target_ip'], '192.0.2.1')
                self.assertGreater(source['checked_at'], 0)
            self.assertTrue(any('未配置经授权' in line for line in rows[:-1]))

    def test_dns_connect_tls_and_timeout_are_classified_without_retry(self):
        self.runtime()
        for code, category in ((6, 'dns'), (7, 'connection'), (28, 'timeout'), (35, 'tls'), (60, 'tls'), (56, 'transport'), (63, 'response_too_large')):
            with self.subTest(code=code):
                result, printed, argv, records = self.youtube({'kind': 'stub', 'code': code})
                self.assertEqual(result['status'], 'unknown')
                self.assertEqual(result['Status'], '未知')
                self.assertEqual(result['Error']['category'], category)
                self.assertTrue(result['Attempted'])
                self.assertGreater(result['last_attempt_at'], 0)
                self.assertEqual(result['elapsed_seconds'], 0.001)
                self.assertEqual(result['Attempts'][0]['error'], category)
                self.assertEqual(result['Attempts'][0]['curl_exit'], code)
                self.assertIsNone(result['Attempts'][0]['http_status'])
                self.assertEqual(records, [])
                self.assert_anonymous_once(argv, records)
                self.assertTrue(any('YouTube：未知' in line for line in printed))

    def test_owned_http_403_429_timeout_and_unrecognized_200_remain_unknown(self):
        self.runtime()
        for status, delay, payload, category in ((403, False, KNOWN, 'http_403'), (429, False, KNOWN, 'http_429'),
                                               (200, True, KNOWN, 'timeout'), (500, False, KNOWN, 'transport'),
                                               (200, False, '', 'empty_response'), (200, False, '<html>login</html>', 'schema_mismatch'),
                                               (200, False, ' \n\t ', 'empty_response'),
                                               (200, False, 'ad-free', 'schema_mismatch'),
                                               (200, False, '{"contentRegion":"US"}', 'schema_mismatch'),
                                               (200, False, '{"contentRegion":"US"} ad-free', 'schema_mismatch'),
                                               (200, False, KNOWN + '{"contentRegion":"JP"}', 'schema_mismatch'),
                                               (200, False, KNOWN.replace('YouTube Premium - YouTube', 'Sign in - Google Accounts'), 'schema_mismatch'),
                                               (200, False, KNOWN.replace('</body>', '<form action="https://accounts.google.com/login"></form></body>'), 'schema_mismatch'),
                                               (200, False, KNOWN.replace('</body>', '<form action="https://consent.youtube.com/save"></form></body>'), 'schema_mismatch'),
                                               (200, False, KNOWN.replace('</body>', '<form id="challenge-form"></form></body>'), 'schema_mismatch'),
                                               (200, False, 'x' * (2 * 1024 * 1024 + 1), 'response_too_large')):
            with self.subTest(status=status, delay=delay, payload_size=len(payload), payload_sha256=hashlib.sha256(payload.encode()).hexdigest()):
                result, printed, argv, records = self.youtube({'kind': 'http'}, status=status, delay=delay, payload=payload)
                self.assertEqual(result['status'], 'unknown')
                self.assertEqual(result['Status'], '未知')
                self.assertEqual(result['Error']['category'], category)
                self.assertTrue(result['Attempted'])
                self.assertEqual(result['Attempts'][0]['error'], category)
                self.assertEqual(result['target_ip'], '192.0.2.1')
                self.assertTrue(any('YouTube：未知' in line for line in printed))
                self.assert_anonymous_once(argv, records)

    def test_recognized_anonymous_page_preserves_heuristic_without_credentials(self):
        self.runtime()
        for payload, expected, region in ((KNOWN, 'YES', 'US'),
                                          (KNOWN.replace('ad-free', 'Premium is not available in your country'), 'NO_PREMIUM', ''),
                                          (KNOWN.replace('US', 'CN').replace('ad-free', 'www.google.cn'), 'CN', 'CN')):
            with self.subTest(expected=expected, payload_size=len(payload)):
                result, printed, argv, records = self.youtube({'kind': 'http'}, payload=payload)
                self.assertEqual(result['status'], 'succeeded')
                self.assertEqual(result['Status'], expected)
                self.assertEqual(result['Region'].strip(), region)
                # The original report represents an unavailable type with the
                # literal legacy value "null"; retain that contract here.
                self.assertEqual(result['Type'], 'DIRECT' if expected == 'YES' else 'null')
                self.assertIsNone(result['Error'])
                self.assertTrue(result['Attempted'])
                self.assertEqual(result['Attempts'][0]['http_status'], 200)
                self.assertEqual(result['Attempts'][0]['curl_exit'], 0)
                self.assertIsNone(result['Attempts'][0]['error'])
                self.assertEqual(printed, [])
                self.assert_anonymous_once(argv, records)
        _, _, argv, records = self.youtube({'kind': 'http'}, child=True)
        self.assert_anonymous_once(argv, records)

    def test_post_download_size_check_rejects_oversize_even_with_successful_transport(self):
        result, _, argv, records = self.youtube({'kind': 'stub', 'code': 0, 'status': 200, 'body_size': 2 * 1024 * 1024 + 1})
        self.assertEqual(result['status'], 'unknown')
        self.assertEqual(result['Status'], '未知')
        self.assertEqual(result['Error']['category'], 'response_too_large')
        self.assertEqual(result['Attempts'][0]['curl_exit'], 0)
        self.assertEqual(records, [])
        self.assert_anonymous_once(argv, records)

    def test_chunked_stream_is_bounded_before_capture_and_all_producers_are_reaped(self):
        self.runtime()
        # First exercise the literal production options. On curl 7.88 the
        # independent reader supplies the bound; newer curl can stop earlier.
        result, _, argv, records = self.youtube({'kind': 'http', 'streaming': True}, chunked=True)
        self.assertEqual(result['Error']['category'], 'response_too_large')
        attempt = result['Attempts'][0]
        self.assertLessEqual(attempt['captured_bytes'], 2 * 1024 * 1024 + 128)
        self.assertEqual(attempt['producer_deadline_seconds'], 10)
        self.assert_anonymous_once(argv, records)
        literal_result = {'captured_bytes': attempt['captured_bytes'], 'curl_exit': attempt['curl_exit'],
                          'processes': self.last_process_rows}
        # Then remove only the proxy's curl size option to exercise the hard
        # reader on every curl version. Production argv above remains intact.
        result, _, argv, records = self.youtube({'kind': 'http', 'streaming': True, 'ignore_filesize_for_fixture': True}, chunked=True)
        attempt = result['Attempts'][0]
        self.assertEqual(attempt['captured_bytes'], 2 * 1024 * 1024 + 128)
        self.assertEqual(result['Error']['category'], 'response_too_large')
        for field in ('curl_exit', 'http_status', 'elapsed_seconds'):
            self.assertIsNone(attempt[field], field)
        self.assertIsNone(result['elapsed_seconds'])
        self.assert_anonymous_once(argv, records)
        completed = next(row for row in self.last_process_rows if row['event'] == 'reaped')
        self.assertTrue(completed['broken_pipe'])
        # Kernel pipe buffering can accept a few additional chunks after the
        # receiver reaches its cap; the captured byte count above is exact.
        self.assertLess(completed['forwarded_bytes'], 4 * 1024 * 1024)
        self.assertLess(completed['duration_seconds'], 11)
        # Negative control: --max-filesize alone, without the production
        # receiver, gets only a finite four-MiB response from owned loopback.
        recorder = ChunkedRecorder()
        try:
            environment = {key: value for key, value in os.environ.items()
                           if key.lower() not in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
            start = time.monotonic()
            old = subprocess.Popen([shutil.which('curl'), '-q', '-sS', '--max-time', '10', '--max-filesize', '2097152', recorder.url],
                                   env=environment, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            received = 0
            try:
                while chunk := old.stdout.read(16384):
                    received += len(chunk)
                    self.assertLessEqual(received, 4 * 1024 * 1024)
                old_code = old.wait(timeout=11)
            finally:
                old.stdout.close()
                if old.poll() is None:
                    old.kill()
                    old.wait(timeout=3)
            with self.assertRaises(ProcessLookupError):
                os.kill(old.pid, 0)
            if old_code == 0:
                self.assertEqual(received, 4 * 1024 * 1024, 'older curl leaves chunked stdout unbounded by its size option')
            else:
                self.assertEqual(old_code, 63, 'newer curl can supply its own independent size bound')
                self.assertLessEqual(received, 2 * 1024 * 1024)
            print(json.dumps({'owned_chunked_acceptance': {'literal_production': literal_result,
                             'independent_reader': {'captured_bytes': attempt['captured_bytes'], 'null_transport_fields': True,
                                                    'processes': self.last_process_rows},
                             'finite_old_option_control': {'payload_bytes': 4 * 1024 * 1024, 'received_bytes': received,
                                                           'curl_exit': old_code, 'duration_seconds': time.monotonic() - start}}}))
        finally:
            recorder.close()

    def test_missing_footer_binary_bytes_and_reader_errors_never_invent_transport_success(self):
        self.runtime()
        for scenario, reader, expected in (({'kind': 'stub', 'code': 0, 'body': KNOWN, 'omit_footer': True}, None, 'incomplete_response'),
                                           ({'kind': 'stub', 'code': 0, 'status': 200, 'body': KNOWN + '\0'}, None, 'invalid_response'),
                                           ({'kind': 'http'}, 'head', 'reader_error'),
                                           ({'kind': 'http'}, 'base64', 'reader_error')):
            with self.subTest(reader=reader, category=expected):
                result, _, _, _ = self.youtube(scenario, fail_reader=reader)
                self.assertEqual(result['Status'], '未知')
                self.assertEqual(result['Error']['category'], expected)
                for field in ('curl_exit', 'http_status', 'elapsed_seconds'):
                    self.assertIsNone(result['Attempts'][0][field], field)

    def test_errexit_pipefail_and_inherited_errexit_preserve_unknown_and_other_report_data(self):
        self.runtime()
        for status, chunked, payload, expected in ((403, False, KNOWN, 'http_403'),
                                                   (200, True, KNOWN, 'response_too_large'),
                                                   (200, False, KNOWN.replace('{"contentRegion":"US"}', ''), 'schema_mismatch'),
                                                   (200, False, '<html>login</html>', 'schema_mismatch')):
            with self.subTest(status=status, chunked=chunked, category=expected):
                scenario = {'kind': 'http', 'streaming': chunked, 'ignore_filesize_for_fixture': chunked}
                result, _, _, _ = self.youtube(scenario, status=status, chunked=chunked, payload=payload, strict=True)
                self.assertEqual(result['Status'], '未知')
                self.assertEqual(result['Error']['category'], expected)
                if chunked:
                    for field in ('curl_exit', 'http_status', 'elapsed_seconds'):
                        self.assertIsNone(result['Attempts'][0][field], field)

    def test_exact_identity_unique_function_bounds_and_metadata_anchors(self):
        sample = b''.join((name + '(){\n:\n}\n').encode() for name in policy.FUNCTIONS) + policy.JSON_ANCHOR
        identity = {'source_sha256': hashlib.sha256(sample).hexdigest(),
                    'patched_sha256': hashlib.sha256(policy.patch(sample)).hexdigest()}
        with mock.patch.dict(policy.SOURCES, {'ip.sh': identity}):
            self.assertEqual(policy.transform('ip.sh', sample), policy.patch(sample))
            for bad in (sample + b'!', 'text', b'x' * (policy.MAX_SOURCE + 1)):
                with self.assertRaises(ValueError):
                    policy.transform('ip.sh', bad)
            with mock.patch.dict(identity, patched_sha256='0' * 64):
                with self.assertRaisesRegex(ValueError, 'output SHA256'):
                    policy.transform('ip.sh', sample)
            with mock.patch.dict(identity, source_sha256='0' * 64):
                with self.assertRaisesRegex(ValueError, 'input SHA256'):
                    policy.transform('ip.sh', sample)
        for malformed in (sample + sample, sample.replace(b'db_dbip(){\n', b'db_dbip_missing(){\n'),
                          sample.replace(policy.JSON_ANCHOR, b''), sample + policy.JSON_ANCHOR):
            with self.assertRaisesRegex(ValueError, 'unique'):
                policy.patch(malformed)
        with self.assertRaises(ValueError):
            policy.function_span('unknown', sample)
        with self.assertRaises(ValueError):
            policy.transform('net.sh', sample)

    def test_actual_fixed_sources_and_unrelated_functions_remain_unchanged(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires verified readonly 17-file source cache')
        original = helper.verified((READONLY_SOURCES / 'ip.sh').read_bytes(), helper.validate(helper.decode((PLUGIN / 'source-lock.json').read_bytes()))['ip.sh'])
        # Read the exact r15 layer through the real helper; later layers are
        # isolated only here to verify this stage's original byte changes.
        before = self.production_before_access()
        after = self.production()
        self.assertEqual(hashlib.sha256(before).hexdigest(), policy.SOURCES['ip.sh']['source_sha256'])
        self.assertEqual(policy.transform('ip.sh', before), after)
        self.assertEqual(hashlib.sha256(original).hexdigest(), 'b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf')
        for name in ('MediaUnlockTest_Netflix', 'MediaUnlockTest_TikTok', 'MediaUnlockTest_PrimeVideo_Region',
                     'MediaUnlockTest_Reddit', 'OpenAITest', 'db_ipinfo', 'db_ipqs', 'show_score'):
            self.assertEqual(body(before.decode(), name), body(after.decode(), name))
        for name in policy.FUNCTIONS:
            self.assertNotEqual(body(before.decode(), name), body(after.decode(), name))
        for name in ('db_ipregistry', 'db_dbip', 'MediaUnlockTest_DisneyPlus', 'read_ref'):
            self.assertNotIn('curl ', body(after.decode(), name))
        self.assertNotIn('--retry ', after.decode())
        self.assertNotRegex(body(after.decode(), 'MediaUnlockTest_YouTube_Premium'), r'(?:^|\s)(?:-b|--cookie)(?:\s|=)')

    def production_before_access(self):
        with tempfile.TemporaryDirectory() as name:
            target = Path(name) / 'sources'
            bundle = helper.decode(helper.pack(helper.decode((PLUGIN / 'source-lock.json').read_bytes()), READONLY_SOURCES))
            helper.materialize(bundle, target)
            return browser.sources.fixture.serve_before_access(helper, target, ['-Ls', 'https://IP.Check.Place'])

    def test_helper_file_type_size_hash_and_output_boundaries_fail_closed(self):
        with tempfile.TemporaryDirectory() as name:
            path = Path(name) / 'access-policy.py'
            content = (PLUGIN / path.name).read_bytes()
            with mock.patch.object(helper, '__file__', str(path.with_name('native-source-helper.py'))):
                path.write_bytes(content)
                self.assertIn('transform', helper.access_policy())
                for invalid in (content + b'!', b'x' * 65537):
                    path.write_bytes(invalid)
                    with self.assertRaises(ValueError):
                        helper.access_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.access_policy()
                path.symlink_to(PLUGIN / path.name)
                with self.assertRaises(OSError):
                    helper.access_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.access_policy()
        for invalid in (b'changed', 'text', b'x' * (helper.MAX_FILE + 8193)):
            with mock.patch.object(helper, 'access_policy', return_value={'SOURCES': policy.SOURCES, 'transform': lambda role, data: invalid}):
                with self.assertRaisesRegex(ValueError, 'served provider access policy output'):
                    helper.authorized_provider_access('ip.sh', b'input')


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--request':
        request()
    parser = argparse.ArgumentParser()
    parser.add_argument('--readonly-upstream-dir', type=Path)
    args, remaining = parser.parse_known_args()
    READONLY_SOURCES = args.readonly_upstream_dir
    unittest.main(argv=[sys.argv[0], *remaining])
