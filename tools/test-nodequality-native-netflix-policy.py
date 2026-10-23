#!/usr/bin/env python3
"""Verify the final fixed Netflix policy with inert stubs and owned HTTP only."""
import argparse
import hashlib
import http.server
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import stat
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
TITLES = ('81280792', '70143836')
HARD_CAP = 2097280
MAX_SCENARIOS_FILE = 8 * 1024 * 1024


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


policy = module('netflix_policy_test', PLUGIN / 'native-netflix-policy.py')
helper = module('netflix_helper_test', PLUGIN / 'native-source-helper.py')


def body(content, name):
    matches = list(re.finditer(r'(?m)^(?:function )?' + re.escape(name) + r'\(\)\{\n[^\0]*?^\}\n', content))
    if len(matches) != 1:
        raise AssertionError('fixture requires a unique complete function: ' + name)
    return matches[0].group()


def page(title, *, unavailable=False, region='US', padding=0, schema='Movie'):
    # Synthetic positive contracts, never evidence of actual provider playback.
    content = '<h1>Oh no!</h1>' if unavailable else (
        '<link rel="canonical" href="https://www.netflix.com/us/title/' + title + '">'
        '<script type="application/ld+json">{"@type":"' + schema + '"}</script>')
    return ('<html><head><title>Fixture | Netflix</title></head><body>' + content
            + '<script>{"id":"' + region + '","countryName":"Fixture"}</script>'
            + ' ' * padding + '</body></html>')


PRELUDE = r'''
set -o pipefail
declare -A netflix sinfo smedia tiktok disney youtube amazon reddit chatgpt
sinfo=([media]=fixture [lmedia]=7)
smedia=([bad]=FAILED [yes]=YES [org]=ORIGINALS [no]=NO [nodata]='')
ibar_step=0
CurlARG=''
IP=192.0.2.1
UA_Browser='fixture-only'
show_progress_bar(){ :; }
kill_progress_bar(){ :; }
Check_DNS_1(){ :; }
Check_DNS_2(){ :; }
Check_DNS_3(){ :; }
Get_Unlock_Type(){ printf DIRECT; }
clean_ansi(){ printf '%s' "$1"; }
'''


def read_scenarios(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or not 0 < metadata.st_size <= MAX_SCENARIOS_FILE:
            raise ValueError('fixture scenarios require a bounded ordinary JSON file')
        content = stream.read(MAX_SCENARIOS_FILE + 1)
    if not content or len(content) > MAX_SCENARIOS_FILE:
        raise ValueError('fixture scenarios exceed their byte limit')
    value = json.loads(content)
    if not isinstance(value, dict) or set(value) != set(TITLES):
        raise ValueError('fixture scenarios require both owned title cases')
    return value


def request():
    args = sys.argv[2:]
    if not args or args[-1] not in tuple('https://www.netflix.com/title/' + title for title in TITLES):
        raise SystemExit('fixture rejects unexpected destination')
    title = args[-1].rsplit('/', 1)[-1]
    arguments = Path(os.environ['ARGUMENTS'])
    prior = [json.loads(line) for line in arguments.read_text().splitlines()] if arguments.exists() else []
    with arguments.open('a') as output:
        output.write(json.dumps(args) + '\n')
    scenarios = read_scenarios(os.environ['SCENARIOS_FILE'])
    scenario = scenarios[title]
    if isinstance(scenario, list):
        count = sum(row[-1].rsplit('/', 1)[-1] == title for row in prior)
        scenario = scenario[min(count, len(scenario) - 1)]
    if scenario.get('kind', 'stub') == 'stub':
        sys.stderr.write(scenario.get('stderr', 'curl: fixture transport failure\n'))
        try:
            sys.stdout.write(scenario.get('body', ''))
            if 'body_size' in scenario:
                sys.stdout.write('x' * scenario['body_size'])
            if '--write-out' in args and not scenario.get('omit_footer'):
                footer = args[args.index('--write-out') + 1]
                footer = footer.replace('%{http_code}', str(scenario.get('status', '000')))
                footer = footer.replace('%{time_total}', str(scenario.get('elapsed', '0.001')))
                sys.stdout.write(footer)
            sys.stdout.flush()
        except BrokenPipeError:
            os._exit(23)
        raise SystemExit(scenario.get('code', 0))
    endpoint = os.environ['RECORDER']
    if not endpoint.startswith('http://127.0.0.1:'):
        raise SystemExit('fixture requires owned loopback')
    environment = {key: value for key, value in os.environ.items()
                   if key.lower() not in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
    forwarded_args = args[:-1]
    if os.environ.get('IGNORE_FILESIZE') == '1':
        index = forwarded_args.index('--max-filesize')
        forwarded_args = forwarded_args[:index] + forwarded_args[index + 2:]
    # Production argv is recorded above with its ten-second ceiling. Only
    # non-streaming timeout fixtures override it to keep the owned test short.
    timeout = '10' if os.environ.get('STREAMING') == '1' else '0.2'
    child = subprocess.Popen([os.environ['REAL_CURL'], *forwarded_args, '--max-time', timeout,
                              endpoint + '/title/' + title], env=environment,
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    processes = Path(os.environ['PROCESSES'])
    with processes.open('a') as output:
        output.write(json.dumps({'event': 'started', 'proxy_pid': os.getpid(), 'curl_pid': child.pid}) + '\n')
    forwarded = 0
    broken = False
    started = time.monotonic()
    try:
        while chunk := child.stdout.read(16384):
            # Forward immediately; capturing the whole real response in this
            # proxy would hide an unbounded production command substitution.
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
    with processes.open('a') as output:
        output.write(json.dumps({'event': 'reaped', 'proxy_pid': os.getpid(), 'curl_pid': child.pid,
                                 'curl_exit': code, 'broken_pipe': broken, 'forwarded_bytes': forwarded,
                                 'duration_seconds': time.monotonic() - started}) + '\n')
    raise SystemExit(23 if broken else code)


class Recorder:
    """Serve finite owned responses, including at most four MiB of chunked data."""
    def __init__(self, scenarios):
        self.requests = []
        self.bytes_written = 0
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = 'HTTP/1.1'

            def do_GET(self):
                title = self.path.rsplit('/', 1)[-1]
                if title not in TITLES:
                    self.send_error(404)
                    return
                owner.requests.append({'title': title, 'method': 'GET',
                                       'headers': {key.lower(): value for key, value in self.headers.items()}})
                row = scenarios[title]
                if isinstance(row, list):
                    count = sum(request['title'] == title for request in owner.requests)
                    row = row[min(count - 1, len(row) - 1)]
                time.sleep(row.get('delay', 0))
                self.send_response(row.get('status', 200))
                self.send_header('Content-Type', 'text/html')
                self.send_header('Connection', 'close')
                if row.get('chunked'):
                    self.send_header('Transfer-Encoding', 'chunked')
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
                    return
                data = row.get('body', page(title)).encode()
                self.send_header('Content-Length', str(len(data) + row.get('extra_length', 0)))
                self.end_headers()
                try:
                    self.wfile.write(data)
                    self.wfile.flush()
                    owner.bytes_written += len(data)
                except (BrokenPipeError, ConnectionResetError):
                    pass

            def log_message(self, *args):
                pass

        self.server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.handle_error = lambda request, address: None
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.url = 'http://127.0.0.1:' + str(self.server.server_port)

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        if self.thread.is_alive():
            raise AssertionError('owned recorder did not stop')


class NetflixTests(unittest.TestCase):
    def runtime(self):
        major = subprocess.check_output([BASH, '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'], text=True)
        if int(major) < 4:
            self.skipTest('fixed associative arrays require Bash >= 4 on dedicated Debian')
        if not shutil.which('jq') or not shutil.which('curl'):
            self.skipTest('requires real jq and curl')

    def production(self, *, before_openai=False):
        if READONLY_SOURCES is None:
            self.skipTest('requires the readonly verified 17-file upstream source cache')
        with tempfile.TemporaryDirectory() as name:
            target = Path(name) / 'sources'
            bundle = helper.decode(helper.pack(helper.decode((PLUGIN / 'source-lock.json').read_bytes()), READONLY_SOURCES))
            helper.materialize(bundle, target)
            # Every runtime case uses the complete source-helper chain. Only
            # historical identity/error controls isolate the later OpenAI layer.
            if before_openai:
                with mock.patch.object(helper, 'authorized_openai', side_effect=lambda role, content: content):
                    return helper.serve(target, ['-Ls', 'https://IP.Check.Place'])
            return helper.serve(target, ['-Ls', 'https://IP.Check.Place'])

    @staticmethod
    def undo(content):
        for before, after in reversed(policy.REPLACEMENTS):
            if content.count(after) != 1:
                raise AssertionError('fixture requires unique final Netflix output')
            content = content.replace(after, before, 1)
        if hashlib.sha256(content).hexdigest() != policy.SOURCES['ip.sh']['source_sha256']:
            raise AssertionError('fixture did not recover exact r16 input')
        return content

    @staticmethod
    def valid_rows():
        return {title: {'body': page(title), 'status': 200, 'code': 0} for title in TITLES}

    def query(self, scenarios, *, previous=False, canonical=False, http=None,
              strict=False, extra='', repeat=False, streaming=False, ignore_filesize=False):
        self.runtime()
        content = self.production(before_openai=previous)
        if previous:
            content = self.undo(content)
        if canonical:
            content = (READONLY_SOURCES / 'ip.sh').read_bytes()
            self.assertEqual(hashlib.sha256(content).hexdigest(),
                             'b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf')
        text = content.decode()
        recorder = Recorder(http) if http else None
        try:
            with tempfile.TemporaryDirectory() as name:
                directory = Path(name)
                binary = directory / 'curl'
                binary.write_text('#!/bin/sh\nexec "$PYTHON" "$TOOL" --request "$@"\n')
                binary.chmod(0o700)
                # Linux limits each argv/environment string independently.
                # Keep the large positive pages intact and transport them in
                # an owned bounded file, never through an environment value.
                scenario_path = directory / 'scenarios.json'
                scenario_content = json.dumps(scenarios).encode()
                self.assertLessEqual(len(scenario_content), MAX_SCENARIOS_FILE)
                with scenario_path.open('xb') as output:
                    os.fchmod(output.fileno(), 0o600)
                    output.write(scenario_content)
                env = dict(os.environ, PATH=str(directory) + ':' + os.environ['PATH'],
                           PYTHON=sys.executable, TOOL=str(Path(__file__).resolve()), SCENARIOS_FILE=str(scenario_path),
                           RECORDER=recorder.url if recorder else '', ARGUMENTS=str(directory / 'arguments.jsonl'),
                           PROCESSES=str(directory / 'processes.jsonl'), REAL_CURL=shutil.which('curl'),
                           STREAMING='1' if streaming else '', IGNORE_FILESIZE='1' if ignore_filesize else '')
                env.pop('SCENARIOS', None)
                script = PRELUDE
                if not canonical:
                    script += 'APSP_NATIVE_CURL=$(type -P curl)\n'
                    for function in ('apsp_browser_header', 'curl', 'apsp_netflix_unknown',
                                     'apsp_netflix_page_error', 'apsp_netflix_fetch'):
                        if function + '(){\n' in text:
                            script += body(text, function)
                script += body(text, 'MediaUnlockTest_Netflix') + '\n' + extra
                if strict:
                    script += 'set -e\nshopt -s inherit_errexit\n'
                script += 'netflix=([ustatus]=STALE [uregion]=STALE [utype]=STALE [error]=STALE [attempts]=\'["STALE"]\')\n'
                updates = ''.join(line + '\n' for line in text.splitlines()
                                  if line.startswith('media_updates+=') and 'Netflix' in line)
                for _ in range(2 if repeat else 1):
                    script += 'MediaUnlockTest_Netflix 4\nmedia_updates=""\n' + updates
                    script += '''printf '%s' '{"Media":{"Other":{"Status":"retained"}},"Score":{"IPQS":"0"}}' | jq -c "${media_updates} ."\n'''
                run = subprocess.run([BASH], input=script.encode(), env=env, capture_output=True, timeout=26)
                self.assertEqual(run.returncode, 0, run.stderr)
                self.assertEqual(run.stderr, b'')
                events_path = directory / 'processes.jsonl'
                events = [json.loads(row) for row in events_path.read_text().splitlines()] if events_path.exists() else []
                for start in [event for event in events if event['event'] == 'started']:
                    reaped = [event for event in events if event['event'] == 'reaped'
                              and event['proxy_pid'] == start['proxy_pid'] and event['curl_pid'] == start['curl_pid']]
                    self.assertEqual(len(reaped), 1, 'owned proxy must wait/reap its native curl')
                    self.assertLess(reaped[0]['duration_seconds'], 11)
                    for pid in (start['proxy_pid'], start['curl_pid']):
                        with self.assertRaises(ProcessLookupError):
                            os.kill(pid, 0)
                lines = run.stdout.decode().splitlines()
                reports = [json.loads(line) for line in lines if line.startswith('{')]
                self.assertEqual(len(reports), 2 if repeat else 1, run.stdout)
                for report in reports:
                    self.assertEqual(report['Media']['Other'], {'Status': 'retained'})
                    self.assertEqual(report['Score'], {'IPQS': '0'})
                arguments = directory / 'arguments.jsonl'
                calls = [json.loads(line) for line in arguments.read_text().splitlines()]
                if not canonical:
                    for args in calls:
                        self.assertEqual(args[0], '-q')
                        self.assertEqual(args[args.index('--max-time') + 1], '10')
                        self.assertNotIn('--retry', args)
                        self.assertNotIn('--user-agent', args)
                        self.assertNotIn('--cookie', args)
                        self.assertNotIn('-b', args)
                return (reports[-1]['Media']['Netflix'], calls,
                        list(recorder.requests) if recorder else [], events, reports)
        finally:
            if recorder:
                recorder.close()

    def test_original_403_is_success_but_r17_stops_and_preserves_error_evidence(self):
        self.runtime()
        rows = {title: {'code': 22, 'status': 403, 'stderr': 'curl: (22) HTTP 403\n'} for title in TITLES}
        old, calls, _, _, _ = self.query(rows, canonical=True)
        new, calls_new, _, _, _ = self.query(rows)
        self.assertEqual(old['Status'], 'YES')
        self.assertEqual(len(calls), 2)
        self.assertEqual(new['Status'], '未知')
        self.assertEqual(new['Error']['category'], 'http_403')
        self.assertEqual(len(calls_new), 1)
        self.assertEqual(new['Attempts'][0]['curl_exit'], 22)

    def test_each_transfer_failure_is_unknown_without_reusing_state(self):
        self.runtime()
        cases = [(6, '000', 'dns'), (7, '000', 'connection'), (18, 200, 'incomplete_response'),
                 (28, '000', 'timeout'), (35, '000', 'tls'), (60, '000', 'tls'),
                 (63, 200, 'response_too_large'), (22, 429, 'http_429'), (22, 500, 'transport')]
        for failed_title in TITLES:
            for code, status, category in cases:
                with self.subTest(title=failed_title, code=code):
                    rows = self.valid_rows()
                    rows[failed_title] = {'code': code, 'status': status, 'body': page(failed_title)}
                    result, calls, requests, _, _ = self.query(rows)
                    self.assertEqual(result['Status'], '未知')
                    self.assertEqual(result['Error']['category'], category)
                    self.assertNotIn('STALE', json.dumps(result))
                    expected = 1 if failed_title == TITLES[0] else 2
                    self.assertEqual(len(calls), expected, 'stop at the first failure without retries')
                    self.assertEqual(requests, [])
                    self.assertEqual(len(result['Attempts']), expected)
                    attempt = result['Attempts'][-1]
                    self.assertEqual(attempt['error'], category)
                    self.assertEqual(attempt['target_ip'], '192.0.2.1')
                    self.assertGreater(attempt['attempted_at'], 0)
                    self.assertEqual(attempt['elapsed_seconds'], 0.001)
                    self.assertEqual(attempt['curl_exit'], code)
                    self.assertEqual(attempt['reader_limit_bytes'], HARD_CAP)
                    self.assertEqual(attempt['producer_deadline_seconds'], 10)

    def test_main_page_shapes_stop_at_the_first_schema_failure(self):
        self.runtime()
        valid = page(TITLES[0])
        invalid = [valid[:-7], valid.replace('Fixture | Netflix', 'Other site'),
                   valid.replace(TITLES[0], TITLES[1]), valid.replace(TITLES[0], TITLES[0] + 'abc'),
                   valid.replace('"Movie"', '"WebSite"'),
                   '<html><title>Netflix</title><p>Oh no!</p></html>',
                   valid.replace('</body>', '<form action="/login">Sign in</form></body>'),
                   valid.replace('</body>', '<form id="challenge-form">Challenge</form></body>')]
        for payload in invalid:
            with self.subTest(payload=payload[:90]):
                rows = self.valid_rows()
                rows[TITLES[0]]['body'] = payload
                result, calls, _, _, _ = self.query(rows)
                self.assertEqual(result['Status'], '未知')
                self.assertEqual(result['Error']['category'], 'schema_mismatch')
                self.assertEqual(len(calls), 1)
                self.assertEqual(result['Attempts'][0]['error'], 'schema_mismatch')
                self.assertEqual(result['Attempts'][0]['curl_exit'], 0)
                self.assertEqual(result['Attempts'][0]['http_status'], 200)
        for payload in ('', ' \t\n'):
            rows = self.valid_rows()
            rows[TITLES[0]]['body'] = payload
            result, calls, _, _, _ = self.query(rows)
            self.assertEqual(result['Error']['category'], 'empty_response')
            self.assertEqual(len(calls), 1)
        for status in ('000', '204', '302', '403', '429', 'abc', ''):
            rows = self.valid_rows()
            rows[TITLES[0]]['status'] = status
            result, calls, _, _, _ = self.query(rows)
            self.assertEqual(result['Status'], '未知')
            self.assertEqual(len(calls), 1)

    def test_valid_titles_originals_and_mixed_results_keep_r15_classifier(self):
        self.runtime()
        for unavailable in ((False, False), (True, True), (True, False), (False, True)):
            rows = {title: {'body': page(title, unavailable=missing), 'status': 200, 'code': 0}
                    for title, missing in zip(TITLES, unavailable)}
            old, _, _, _, _ = self.query(rows, previous=True)
            new, calls, _, _, _ = self.query(rows)
            self.assertEqual(new['Status'], old['Status'])
            self.assertEqual(new['Region'], old['Region'])
            self.assertEqual(new['Type'], old['Type'])
            self.assertEqual(new['Status'], 'ORIGINALS' if all(unavailable) else 'YES')
            self.assertEqual(new['Region'].strip(), 'US')
            self.assertIsNone(new['Error'])
            self.assertEqual(len(new['Attempts']), 2)
            self.assertTrue(all(attempt['error'] is None for attempt in new['Attempts']))
            for args in calls:
                self.assertEqual(args[args.index('-X') + 1], 'GET')
                self.assertEqual(args[args.index('--max-filesize') + 1], '2097152')
        for schema in ('Movie', 'TVSeries', 'TVSeason', 'TVEpisode', 'VideoObject'):
            rows = {title: {'body': page(title, schema=schema), 'status': 200} for title in TITLES}
            self.assertEqual(self.query(rows)[0]['Status'], 'YES')

        for prefix, suffix in (('', ''), ('en/', '?fixture=1'), ('pt-BR/', '#fixture'), ('us/', '/')):
            rows = {title: {'body': page(title).replace('us/title/' + title,
                    prefix + 'title/' + title + suffix), 'status': 200} for title in TITLES}
            result, _, _, _, _ = self.query(rows)
            self.assertEqual(result['Status'], 'YES')
            self.assertIsNone(result['Error'])

    def test_strict_direct_call_preserves_valid_title_and_originals_results(self):
        self.runtime()
        # Direct invocation with no outer conditional or ||: those would make
        # Bash ignore errexit throughout the tested function and hide defects.
        for unavailable in ((False, False), (True, True), (True, False), (False, True)):
            rows = {title: {'body': page(title, unavailable=missing, padding=100000), 'status': 200}
                    for title, missing in zip(TITLES, unavailable)}
            result, calls, _, _, reports = self.query(rows, strict=True)
            self.assertEqual(result['Status'], 'ORIGINALS' if all(unavailable) else 'YES')
            self.assertIsNone(result['Error'])
            self.assertEqual(len(result['Attempts']), 2)
            self.assertEqual(len(calls), 2)
            self.assertEqual(reports[0]['Media']['Other'], {'Status': 'retained'})

    def test_long_valid_pages_and_invalid_regions_under_pipefail(self):
        self.runtime()
        rows = {title: {'body': page(title, padding=100000), 'status': 200} for title in TITLES}
        result, _, _, _, _ = self.query(rows)
        self.assertEqual(result['Status'], 'YES')
        self.assertIsNone(result['Error'])
        rows = {title: {'body': page(title, region='NOT-A-COUNTRY'), 'status': 200} for title in TITLES}
        result, calls, _, _, _ = self.query(rows)
        self.assertEqual(result['Status'], '未知')
        self.assertEqual(result['Error']['category'], 'schema_mismatch')
        self.assertNotIn('NOT-A-COUNTRY', result['Region'])
        self.assertEqual(len(calls), 2)

    def test_failed_new_task_preserves_successful_history_and_other_chapters(self):
        report = module('netflix_report_persistence', PLUGIN / 'native-report.py')
        with tempfile.TemporaryDirectory() as name:
            previous, current = Path(name) / 'previous-task', Path(name) / 'current-task'
            previous.mkdir()
            current.mkdir()
            def files(status):
                return {'ip_quality.log': ('Netflix: ' + status).encode(),
                        'ip_quality.json': json.dumps({'Media': {'Netflix': {'Status': status}}}).encode(),
                        'hardware_quality.log': b'existing hardware chapter',
                        'hardware_quality.json': b'{"fixture":true}'}
            report.publish_sections(previous, files('YES'), archive=True)
            saved = {path.name: path.read_bytes() for path in previous.iterdir()}
            report.publish_sections(current, files('unknown'), archive=True)
            self.assertEqual({path.name: path.read_bytes() for path in previous.iterdir()}, saved)
            self.assertEqual(json.loads((previous / 'section-ip_quality.json').read_bytes())['text'], 'Netflix: YES')
            self.assertEqual(json.loads((current / 'section-ip_quality.json').read_bytes())['text'], 'Netflix: unknown')
            for task in (previous, current):
                hardware = json.loads((task / 'section-hardware_quality.json').read_bytes())
                self.assertEqual(hardware['text'], 'existing hardware chapter')
                self.assertTrue(hardware['complete'])

    def test_real_http_403_429_partial_empty_and_schema_failures_are_unknown(self):
        self.runtime()
        previous_payload = '<html>{"id":"US","countryName":"Fixture"}</html>'
        scenarios = {title: {'kind': 'http'} for title in TITLES}
        http = {title: {'body': previous_payload} for title in TITLES}
        old, _, requests, _, _ = self.query(scenarios, previous=True, http=http)
        self.assertEqual(old['Status'], 'YES', 'negative control retains the r15 weak page heuristic')
        self.assertEqual(len(requests), 2)
        for row, category in (({'status': 403}, 'http_403'), ({'status': 429}, 'http_429'),
                              ({'extra_length': 100}, 'incomplete_response'),
                              ({'body': ''}, 'empty_response'), ({'body': previous_payload}, 'schema_mismatch')):
            with self.subTest(category=category):
                http = {title: {} for title in TITLES}
                http[TITLES[0]] = row
                result, calls, requests, _, _ = self.query(scenarios, http=http)
                self.assertEqual(result['Status'], '未知')
                self.assertEqual(result['Error']['category'], category)
                self.assertEqual(len(calls), 1)
                self.assertEqual(len(requests), 1)
                self.assertEqual(requests[0]['method'], 'GET')
                self.assertNotIn('cookie', requests[0]['headers'])

    def test_real_timeout_records_actual_elapsed_and_ten_second_production_budget(self):
        self.runtime()
        scenarios = {title: {'kind': 'http'} for title in TITLES}
        http = {TITLES[0]: {'delay': 0.4}, TITLES[1]: {}}
        result, calls, requests, _, _ = self.query(scenarios, http=http)
        self.assertEqual(result['Status'], '未知')
        self.assertEqual(result['Error']['category'], 'timeout')
        self.assertEqual(result['Attempts'][0]['curl_exit'], 28)
        self.assertGreater(result['Attempts'][0]['elapsed_seconds'], 0.1)
        self.assertLess(result['Attempts'][0]['elapsed_seconds'], 1)
        self.assertEqual(result['Attempts'][0]['producer_deadline_seconds'], 10)
        self.assertEqual(calls[0][calls[0].index('--max-time') + 1], '10')
        self.assertEqual(len(requests), 1)

    def test_real_success_then_failure_resets_current_state_and_attempts(self):
        self.runtime()
        scenarios = {title: {'kind': 'http'} for title in TITLES}
        http = {TITLES[0]: [{}, {'status': 429}], TITLES[1]: {}}
        result, calls, _, _, reports = self.query(scenarios, http=http, repeat=True)
        self.assertEqual(reports[0]['Media']['Netflix']['Status'], 'YES')
        self.assertIsNone(reports[0]['Media']['Netflix']['Error'])
        self.assertEqual(result['Status'], '未知')
        self.assertEqual(result['Error']['category'], 'http_429')
        self.assertEqual(len(result['Attempts']), 1)
        self.assertEqual(len(calls), 3)

    def test_real_chunked_readers_are_bounded_reaped_and_old_filesize_is_a_control(self):
        self.runtime()
        scenarios = {title: {'kind': 'http'} for title in TITLES}
        for ignore in (False, True):
            with self.subTest(ignore_filesize=ignore):
                http = {TITLES[0]: {'chunked': True}, TITLES[1]: {}}
                result, calls, requests, events, _ = self.query(scenarios, http=http, streaming=True,
                                                              ignore_filesize=ignore, strict=True)
                self.assertEqual(result['Status'], '未知')
                self.assertEqual(result['Error']['category'], 'response_too_large')
                self.assertEqual(len(calls), 1)
                self.assertEqual(len(requests), 1)
                attempt = result['Attempts'][0]
                self.assertLessEqual(attempt['captured_bytes'], HARD_CAP)
                if ignore or attempt['captured_bytes'] == HARD_CAP:
                    self.assertIsNone(attempt['curl_exit'])
                    self.assertIsNone(attempt['http_status'])
                    self.assertIsNone(attempt['elapsed_seconds'])
                else:
                    self.assertEqual(attempt['curl_exit'], 63)
                self.assertEqual(len([event for event in events if event['event'] == 'reaped']), 1)
        # A finite four-MiB owned response is a negative control, never a load
        # test. Drain stdout as it arrives so this fixture cannot conceal the
        # older curl --max-filesize behavior by buffering the whole response.
        recorder = Recorder({TITLES[0]: {'chunked': True}, TITLES[1]: {}})
        environment = {key: value for key, value in os.environ.items()
                       if key.lower() not in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
        try:
            started = time.monotonic()
            child = subprocess.Popen([shutil.which('curl'), '-q', '-fsL', '--max-time', '10',
                                      '--max-filesize', '2097152', recorder.url + '/title/' + TITLES[0]],
                                     env=environment, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            count = 0
            try:
                while chunk := child.stdout.read(16384):
                    count += len(chunk)
                    self.assertLessEqual(count, 4 * 1024 * 1024)
                code = child.wait(timeout=11)
            finally:
                child.stdout.close()
                if child.poll() is None:
                    child.kill()
                    child.wait(timeout=3)
            self.assertLess(time.monotonic() - started, 11)
            with self.assertRaises(ProcessLookupError):
                os.kill(child.pid, 0)
            if code == 0:
                self.assertEqual(count, 4 * 1024 * 1024, 'curl 7.88 alone accepts chunked over-limit output')
            else:
                self.assertEqual(code, 63, 'newer curl may enforce its own independent limit')
                self.assertLessEqual(count, 2097152)
        finally:
            recorder.close()

    def test_reader_failures_nul_missing_footer_and_cap_fail_closed(self):
        self.runtime()
        for override in (
            'head(){ command head "$@";return 42; }\n',
            'base64(){ command base64 "$@";return 42; }\n',
            'base64(){ if [[ $1 == --decode ]];then command base64 "$@";return 42;fi;command base64 "$@"; }\n',
            'wc(){ printf invalid; }\n',
        ):
            with self.subTest(override=override):
                result, calls, _, _, _ = self.query(self.valid_rows(), extra=override)
                self.assertEqual(result['Status'], '未知')
                self.assertEqual(result['Error']['category'], 'reader_error')
                self.assertEqual(len(calls), 1)
                self.assertIsNone(result['Attempts'][0]['curl_exit'])
        for changed, category in (({'body': 'a\0b'}, 'invalid_response'),
                                  ({'omit_footer': True}, 'incomplete_response'),
                                  ({'body_size': HARD_CAP}, 'response_too_large')):
            rows = self.valid_rows()
            rows[TITLES[0]].update(changed)
            result, calls, _, _, _ = self.query(rows)
            self.assertEqual(result['Error']['category'], category)
            self.assertEqual(len(calls), 1)
            for name in ('curl_exit', 'http_status', 'elapsed_seconds'):
                self.assertIsNone(result['Attempts'][0][name])

    def test_errexit_pipefail_keeps_http_failure_and_other_json(self):
        self.runtime()
        rows = self.valid_rows()
        rows[TITLES[0]] = {'code': 22, 'status': 403}
        result, calls, _, _, reports = self.query(rows, strict=True)
        self.assertEqual(result['Status'], '未知')
        self.assertEqual(result['Attempts'][0]['curl_exit'], 22)
        self.assertEqual(len(calls), 1)
        self.assertEqual(reports[0]['Media']['Other'], {'Status': 'retained'})

    def test_production_hashes_syntax_and_unrelated_functions_are_unchanged(self):
        new = self.production(before_openai=True)
        old = self.undo(new)
        self.assertTrue(policy.transform('ip.sh', old) == new, 'fixed Netflix layer changed')
        final = self.production()
        self.assertEqual(body(final.decode(), 'MediaUnlockTest_Netflix'),
                         body(new.decode(), 'MediaUnlockTest_Netflix'))
        for function in ('db_ipqs', 'db_ipregistry', 'MediaUnlockTest_YouTube_Premium',
                         'apsp_netflix_unknown', 'show_media', 'save_json'):
            self.assertEqual(body(new.decode(), function), body(old.decode(), function))
        old_function = body(old.decode(), 'MediaUnlockTest_Netflix').encode()
        expected_function = old_function.replace(policy.CLASSIFIER, policy.CHECKED_CLASSIFIER, 1)
        self.assertEqual(body(new.decode(), 'MediaUnlockTest_Netflix').encode(), expected_function)
        run = subprocess.run([BASH, '-n'], input=new, capture_output=True, timeout=5)
        self.assertEqual(run.returncode, 0, run.stderr)
        canonical = (READONLY_SOURCES / 'ip.sh').read_bytes()
        self.assertEqual(hashlib.sha256(canonical).hexdigest(),
                         'b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf')

    def test_helper_identity_output_and_ordinary_file_boundaries_fail_closed(self):
        with tempfile.TemporaryDirectory() as name:
            path = Path(name) / 'native-netflix-policy.py'
            with mock.patch.object(helper, '__file__', str(path.with_name('native-source-helper.py'))):
                path.write_bytes((PLUGIN / path.name).read_bytes())
                self.assertIn('transform', helper.netflix_policy())
                for content in (path.read_bytes() + b'!', b'x' * 65537):
                    path.write_bytes(content)
                    with self.assertRaises(ValueError):
                        helper.netflix_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.netflix_policy()
                path.symlink_to(PLUGIN / path.name)
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

    def test_unique_boundaries_source_and_output_identities(self):
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
        with self.assertRaises(ValueError):
            policy.transform('net.sh', source)
        run = subprocess.run([BASH, '-n'], input=policy.patch(source), capture_output=True, timeout=5)
        self.assertEqual(run.returncode, 0, run.stderr)
        # This is larger than Linux's per-environment-string limit. Exercise
        # the actual child fixture without requiring Bash associative arrays.
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            path = directory / 'scenarios.json'
            rows = {title: {'body': page(title, padding=100000), 'status': 200, 'code': 0}
                    for title in TITLES}
            path.write_bytes(json.dumps(rows).encode())
            path.chmod(0o600)
            self.assertGreater(path.stat().st_size, 131072)
            self.assertEqual(read_scenarios(path), rows)
            env = dict(os.environ, SCENARIOS_FILE=str(path), ARGUMENTS=str(directory / 'arguments.jsonl'))
            env.pop('SCENARIOS', None)
            run = subprocess.run([sys.executable, str(Path(__file__).resolve()), '--request',
                                  'https://www.netflix.com/title/' + TITLES[0]],
                                 env=env, capture_output=True, timeout=5)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertEqual(run.stdout.decode(), rows[TITLES[0]]['body'])
            link = directory / 'scenarios-link.json'
            link.symlink_to(path)
            with self.assertRaises(OSError):
                read_scenarios(link)
            fifo = directory / 'scenarios-fifo.json'
            os.mkfifo(fifo, 0o600)
            with self.assertRaisesRegex(ValueError, 'ordinary'):
                read_scenarios(fifo)
            with path.open('wb') as output:
                output.truncate(MAX_SCENARIOS_FILE + 1)
            with self.assertRaisesRegex(ValueError, 'bounded'):
                read_scenarios(path)

    def test_packaging_rejects_missing_or_corrupt_helper(self):
        sources = module('netflix_source_tests', ROOT / 'tools/test-nodequality-native-sources.py')
        for missing in (True, False):
            fixture = sources.SourceTests(methodName='runTest')
            fixture.setUp()
            try:
                tree, env = fixture.build_tree()
                path = tree / 'plugins/nodequality/native-netflix-policy.py'
                if missing:
                    path.unlink()
                else:
                    path.write_bytes(path.read_bytes() + b'\n# altered helper bytes\n')
                run = fixture.build(tree, env, 'arm64')
                self.assertNotEqual(run.returncode, 0)
                if not missing:
                    self.assertIn(b'signed Netflix policy helper SHA256 mismatch', run.stderr)
                output = fixture.root / 'artifacts/nodequality' / sources.VERSION
                self.assertFalse((output / 'arm64').exists())
                self.assertFalse((output / 'SHA256SUMS').exists())
            finally:
                fixture.tearDown()


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--request':
        request()
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--readonly-upstream-dir', type=Path)
    args, remaining = parser.parse_known_args()
    READONLY_SOURCES = args.readonly_upstream_dir
    sys.argv = [sys.argv[0], *remaining]
    unittest.main()
