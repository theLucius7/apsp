#!/usr/bin/env python3
"""Verify score-upload consent against inert probes and a loopback recorder."""
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
PLUGIN = ROOT / 'plugins/nodequality'
READONLY_SOURCES = None


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


policy = module('ranking_policy', PLUGIN / 'ranking-policy.py')
helper = module('ranking_source_helper', PLUGIN / 'source-helper.py')
sources = module('ranking_source_tests', ROOT / 'tools/test-nodequality-sources.py')
report = module('ranking_report_policy', PLUGIN / 'report-policy.py')


def request():
    arguments = sys.argv[2:]
    if (len(arguments) != 8 or arguments[:5] != ['-fsS', '--max-time', '10', '-H', 'Content-Type: application/json']
            or arguments[5] != '-d' or arguments[-1] != 'https://mark.check.place'):
        raise SystemExit('unexpected upstream request')
    endpoint = os.environ['RECORDER']
    if not endpoint.startswith('http://127.0.0.1:'):
        raise SystemExit('fixture allows only its own loopback recorder')
    Path(os.environ['REQUEST_ARGUMENTS']).write_text(json.dumps(arguments))
    # Exercise the real curl timeout/HTTP behavior without contacting upstream.
    environment = {key: value for key, value in os.environ.items()
                   if key.lower() not in ('http_proxy', 'https_proxy', 'all_proxy', 'no_proxy')}
    run = subprocess.run([os.environ['REAL_CURL'], *arguments[:-1], endpoint],
                         env=environment, capture_output=True, timeout=13)
    sys.stdout.buffer.write(run.stdout)
    sys.stderr.buffer.write(run.stderr)
    raise SystemExit(run.returncode)


class Recorder:
    def __init__(self, response='success'):
        self.response = response
        self.requests = []
        self.release = threading.Event()
        owner = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                length = int(self.headers.get('Content-Length', '0'))
                if not 0 < length <= 1024:
                    self.send_error(400)
                    return
                owner.requests.append(json.loads(self.rfile.read(length)))
                if response == 'timeout':
                    owner.release.wait(12)
                    return
                code = int(response) if response in ('403', '429') else 200
                data = ({k: {'percentile': i + 40.5} for i, k in enumerate(('cpu', 'gpu', 'mem', 'disk', 'total'))}
                        if response == 'success' else {})
                payload = b'not-json' if response == 'non-json' else json.dumps(data).encode()
                self.send_response(code)
                self.send_header('Content-Length', str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            def log_message(self, *args):
                pass

        self.server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.url = 'http://127.0.0.1:' + str(self.server.server_port) + '/percentile'

    def close(self):
        self.release.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=3)
        if self.thread.is_alive():
            raise AssertionError('loopback recorder did not stop')


class RankingTests(unittest.TestCase):
    def production(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires the previously verified readonly 17-file source cache')
        lock = helper.decode((PLUGIN / 'source-lock.json').read_bytes())
        with tempfile.TemporaryDirectory() as name:
            target = Path(name) / 'sources'
            helper.materialize(helper.decode(helper.pack(lock, READONLY_SOURCES)), target)
            return helper.serve(target, ['-Ls', 'https://Hardware.Check.Place'])

    def require_runtime(self):
        major = subprocess.check_output(['/bin/bash', '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'], text=True)
        if int(major) < 4:
            self.skipTest('upstream associative arrays require Bash >= 4; run on dedicated Debian')
        for tool in ('jq', 'bc', 'curl'):
            if shutil.which(tool) is None:
                self.skipTest('controlled runtime requires ' + tool)

    @staticmethod
    def body(text, name, next_name):
        return text[text.index(name + '(){\n'):text.index('\n' + next_name + '(){\n')] + '\n'

    def run_mark(self, content, upload, response='success'):
        self.require_runtime()
        recorder = Recorder(response)
        self.addCleanup(recorder.close)
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            text = content.decode()
            calculation = self.body(text, 'get_mark', 'show_os')
            display = self.body(text, 'show_mark', 'show_tail')
            # The last save_json fragment updates only Benchmark with real jq.
            end = text.index('\n}\ncheck_Hardware(){\n')
            start = text.rindex('_hwjson="$hwjson"\n', 0, end)
            serializer = text[start:end]
            prelude = r'''
declare -A cpuinfo=([geekbench_multi]=fixture) gpuinfo=([geekbench]=fixture)
declare -A meminfo=([mem_total_kb]=fixture) diskinfo=([total]=fixture)
declare -A markinfo=([cpu_pct]=99 [gpu_pct]=99 [mem_pct]=99 [disk_pct]=99 [total_pct]=99)
declare -A smark=([title]=fixture-ranking)
mode_skip=''
mark_cpu(){ printf cpu >> "$PROBES"; printf 10; }
mark_gpu(){ printf gpu >> "$PROBES"; printf 20; }
mark_mem(){ printf mem >> "$PROBES"; printf 30; }
mark_disk(){ printf disk >> "$PROBES"; printf 40; }
curl(){ "$PYTHON" "$TOOL" --request "$@"; }
'''
            script = report.POLICY.decode() + prelude + calculation + display + '''
get_mark
show_mark > "$DISPLAY"
hwjson='{"CPU":{"retained":true},"Memory":{"fixture":123}}'
''' + serializer + '\nprintf "%s\\n" "$hwjson"\n'
            environment = dict(os.environ, PYTHON=sys.executable, TOOL=str(Path(__file__).resolve()),
                               RECORDER=recorder.url, REAL_CURL=shutil.which('curl'),
                               REQUEST_ARGUMENTS=str(directory / 'request.json'),
                               PROBES=str(directory / 'probes'), DISPLAY=str(directory / 'display'))
            environment.pop('APSP_UPLOAD_REPORT', None)
            if upload is not None:
                environment['APSP_UPLOAD_REPORT'] = upload
            run = subprocess.run(['/bin/bash'], input=script.encode(), env=environment, capture_output=True, timeout=15)
            return run, list(recorder.requests), {
                key: (directory / filename).read_text() if (directory / filename).exists() else None
                for key, filename in [('probes', 'probes'), ('display', 'display'), ('request', 'request.json')]}

    def assert_scores(self, value):
        self.assertEqual(value['CPU'], {'retained': True})
        self.assertEqual(value['Memory'], {'fixture': 123})
        expected = {'total': 100, 'cpu': 10, 'gpu': 20, 'memory': 30, 'disk': 40}
        self.assertEqual({k: value['Benchmark'][k] for k in expected}, expected)

    def test_default_and_false_make_zero_posts_and_keep_local_scores(self):
        self.require_runtime()
        new = self.production()
        old = sources.fixture.undo_ranking('hardware.sh', new)
        for upload in (None, 'false'):
            with self.subTest(upload=upload):
                before, old_posts, _ = self.run_mark(old, upload)
                after, posts, details = self.run_mark(new, upload)
                self.assertEqual(before.returncode, 0, before.stderr)
                self.assertEqual(after.returncode, 0, after.stderr)
                self.assertEqual(len(old_posts), 1)
                self.assertEqual(posts, [])
                self.assertIsNone(details['request'])
                self.assertEqual(details['probes'], 'cpugpumemdisk')
                data = json.loads(after.stdout)
                self.assert_scores(data)
                self.assertFalse(data['Benchmark']['percentile_upload_allowed'])
                self.assertTrue(all(data['Benchmark'][k] is None for k in ('total_pct', 'cpu_pct', 'gpu_pct', 'memory_pct', 'disk_pct')))
                self.assertIn('百分位未知：未允许上传本机评分；本地评分已保留。', details['display'])
                self.assertNotIn('99%', details['display'])
                self.assertIn('N/A', details['display'])

    def test_explicit_true_preserves_payload_local_scores_and_percentiles(self):
        self.require_runtime()
        new = self.production()
        old = sources.fixture.undo_ranking('hardware.sh', new)
        baseline, old_posts, _ = self.run_mark(old, 'true')
        run, posts, details = self.run_mark(new, 'true')
        self.assertEqual(baseline.returncode, 0, baseline.stderr)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(posts, old_posts)
        self.assertEqual(posts, [{'cpu_score': 10, 'gpu_score': 20, 'mem_score': 30, 'disk_score': 40, 'total_score': 100}])
        data = json.loads(run.stdout)
        self.assertTrue(data['Benchmark'].pop('percentile_upload_allowed'))
        self.assertEqual(data, json.loads(baseline.stdout))
        self.assert_scores(data)
        self.assertEqual(data['Benchmark']['total_pct'], 44.5)
        self.assertEqual(details['probes'], 'cpugpumemdisk')
        self.assertNotIn('百分位未知', details['display'])

    def test_http_errors_timeout_and_unusable_response_keep_scores_with_unknown_percentiles(self):
        self.require_runtime()
        new = self.production()
        for response in ('403', '429', 'timeout', 'non-json', 'missing-fields'):
            with self.subTest(response=response):
                run, posts, details = self.run_mark(new, 'true', response)
                self.assertEqual(run.returncode, 0, run.stderr)
                self.assertEqual(len(posts), 1)
                data = json.loads(run.stdout)
                self.assert_scores(data)
                self.assertTrue(all(data['Benchmark'][k] is None for k in ('total_pct', 'cpu_pct', 'gpu_pct', 'memory_pct', 'disk_pct')))
                self.assertNotIn('99%', details['display'])
                self.assertIn('N/A', details['display'])

    def test_invalid_policy_stops_before_scores_or_requests(self):
        self.require_runtime()
        new = self.production()
        for upload in ('', 'TRUE', 'true '):
            with self.subTest(upload=upload):
                run, posts, details = self.run_mark(new, upload)
                self.assertEqual(run.returncode, 2)
                self.assertEqual(run.stdout, b'')
                self.assertIn(b'APSP_UPLOAD_REPORT must be true or false', run.stderr)
                self.assertEqual(posts, [])
                self.assertIsNone(details['probes'])

    def test_production_transformation_preserves_all_other_bytes(self):
        new = self.production()
        old = sources.fixture.undo_ranking('hardware.sh', new)
        self.assertEqual(hashlib.sha256(old).hexdigest(), policy.SOURCES['hardware.sh']['source_sha256'])
        self.assertEqual(hashlib.sha256(new).hexdigest(), policy.SOURCES['hardware.sh']['patched_sha256'])
        self.assertEqual(policy.transform('hardware.sh', old), new)
        self.assertEqual(new.count(b'https://mark.check.place'), 1)
        # The actual request and CPU/GPU/memory/disk probes are byte-identical.
        for name, end in [('mark_cpu', 'mark_gpu'), ('mark_gpu', 'mark_mem'), ('mark_mem', 'mark_disk'), ('mark_disk', 'get_mark')]:
            self.assertEqual(self.body(old.decode(), name, end), self.body(new.decode(), name, end))

    def test_identity_anchors_and_output_are_fail_closed(self):
        source = b''.join(before for before, _ in policy.REPLACEMENTS)
        spec = {'source_sha256': hashlib.sha256(source).hexdigest(), 'patched_sha256': hashlib.sha256(policy.patch(source)).hexdigest()}
        with mock.patch.dict(policy.SOURCES, {'hardware.sh': spec}):
            self.assertEqual(policy.transform('hardware.sh', source), policy.patch(source))
            for invalid in (source + b'!', 'text', b'x' * (policy.MAX_SOURCE + 1)):
                with self.assertRaises(ValueError):
                    policy.transform('hardware.sh', invalid)
            with self.assertRaisesRegex(ValueError, 'unique'):
                policy.patch(source + source)
            with mock.patch.dict(spec, patched_sha256='0' * 64):
                with self.assertRaisesRegex(ValueError, 'output SHA256'):
                    policy.transform('hardware.sh', source)
        with self.assertRaises(ValueError):
            policy.transform('unknown', b'')

    def test_helper_file_boundaries_and_invalid_return_are_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            path = Path(name) / 'ranking-policy.py'
            content = (PLUGIN / path.name).read_bytes()
            with mock.patch.object(helper, '__file__', str(path.with_name('source-helper.py'))):
                path.write_bytes(content)
                self.assertIn('transform', helper.ranking_policy())
                for data in (content + b'!', b'x' * 65537):
                    path.write_bytes(data)
                    with self.assertRaises(ValueError):
                        helper.ranking_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.ranking_policy()
                path.symlink_to(PLUGIN / path.name)
                with self.assertRaises(OSError):
                    helper.ranking_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.ranking_policy()
        for invalid in (b'changed', 'text', b'x' * (helper.MAX_FILE + 4097)):
            with mock.patch.object(helper, 'ranking_policy', return_value={
                    'SOURCES': policy.SOURCES, 'transform': lambda role, content: invalid}):
                with self.assertRaisesRegex(ValueError, 'served ranking policy output'):
                    helper.percentile_policy('hardware.sh', b'input')

    def test_builder_rejects_missing_or_changed_ranking_helper(self):
        for missing in (True, False):
            fixture = sources.SourceTests(methodName='runTest')
            fixture.setUp()
            try:
                tree, env = fixture.build_tree()
                path = tree / 'plugins/nodequality/ranking-policy.py'
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
