#!/usr/bin/env python3
"""Exercise pinned IP score parsers with inert responses and real jq/Bash."""
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
PLUGIN = ROOT / 'plugins/nodequality'
READONLY_SOURCES = None


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


policy = module('score_policy', PLUGIN / 'ip-score-policy.py')
helper = module('score_source_helper', PLUGIN / 'native-source-helper.py')
sources = module('score_source_tests', ROOT / 'tools/test-nodequality-native-sources.py')

PRELUDE = r'''
declare -A scamalytics abuseipdb ip2location ipqs ipapi dbip stype
# Only the extracted functions run; no upstream top-level initializer executes.
declare -A sinfo=([ldatabase]=1 [database]=fixture)
declare -A sscore=([low]=LOW [medium]=MEDIUM [high]=HIGH [verylow]=VERYLOW [elevated]=ELEVATED [veryhigh]=VERYHIGH)
IP=192.0.2.1
show_progress_bar(){ :; }
kill_progress_bar(){ :; }
curl(){ printf '%s' "$RESPONSE_FIXTURE"; }
'''


def body(content, name):
    start = content.index(name + '(){\n')
    # These pinned top-level functions terminate with an unindented standalone brace.
    end = content.index('\n}\n', start)
    return content[start:end + 3]



class ScoreTests(unittest.TestCase):
    def runtime(self):
        major = subprocess.check_output(['/bin/bash', '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'], text=True)
        if int(major) < 4:
            self.skipTest('pinned associative arrays need Bash >= 4; run on dedicated Debian')
        for tool in ('jq', 'bc', 'awk'):
            if shutil.which(tool) is None:
                self.skipTest('requires ' + tool)

    def production(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires previously verified readonly 17-file source cache')
        with tempfile.TemporaryDirectory() as name:
            target = Path(name) / 'sources'
            lock = helper.decode((PLUGIN / 'source-lock.json').read_bytes())
            helper.materialize(helper.decode(helper.pack(lock, READONLY_SOURCES)), target)
            return sources.fixture.serve_before_access(helper, target, ['-Ls', 'https://IP.Check.Place'])

    def value(self, response, path, kind):
        if shutil.which('jq') is None:
            self.skipTest('requires jq')
        run = subprocess.run(['/bin/bash'], input=policy.HELPERS + b'\napsp_ip_score_value "$VALUE" "$FIELD" "$KIND"\n',
                             env=dict(os.environ, VALUE=response, FIELD=path, KIND=kind), capture_output=True, timeout=3)
        self.assertEqual(run.returncode, 0, run.stderr)
        self.assertEqual(run.stderr, b'')
        return run.stdout.decode().strip()

    def test_json_type_range_and_error_envelopes(self):
        for score in (0, 20, 75, 100):
            self.assertEqual(self.value(json.dumps({'score': score}), 'score', 'integer'), str(score))
        for invalid in (None, False, True, '0', '99', [], {}, -1, 101, 0.5):
            with self.subTest(invalid=invalid):
                self.assertEqual(self.value(json.dumps({'score': invalid}), 'score', 'integer'), '')
        for data in ('', '{}', '[]', 'null', '<html>403</html>', '{"score":0}\n{}', '{"score":0}{',
                     '{"score":0,"success":false}', '{"score":0,"status":"fail"}',
                     '{"score":0,"status":"error"}', '{"score":0,"error":"429"}',
                     '{"score":0,"errors":["timeout"]}'):
            with self.subTest(response=data):
                self.assertEqual(self.value(data, 'score', 'integer'), '')

    def test_formatted_and_categorical_scores_are_allowlisted(self):
        for value in ('0 (Low)', '1 (Very High)', '0.001 (Very Low)', '0.5 (Elevated)', '1.0 (HIGH)'):
            self.assertEqual(self.value(json.dumps({'v': value}), 'v', 'ipapi'), value)
        for value in ('NaN (Low)', '1.1 (Low)', '-1 (Low)', '1e-3 (Low)', '0 (unknown)',
                      '0;system("touch marker") (Low)', '0 (Low)\n', '0', 0, None):
            self.assertEqual(self.value(json.dumps({'v': value}), 'v', 'ipapi'), '')
        for value in ('low', 'MEDIUM', 'High'):
            self.assertEqual(self.value(json.dumps({'v': value}), 'v', 'dbip'), value.lower())
        for value in ('null', 'unknown', 'clean', 0, None):
            self.assertEqual(self.value(json.dumps({'v': value}), 'v', 'dbip'), '')

    def query(self, content, name, response):
        script = policy.HELPERS.decode() + PRELUDE + body(content.decode(), 'db_' + name)
        script += '\ndb_' + name + ' 4\nprintf "%s|%s\\n" "${' + name + '[score]}" "${' + name + '[risk]}"\n'
        run = subprocess.run(['/bin/bash'], input=script.encode(), env=dict(os.environ, RESPONSE_FIXTURE=response),
                             capture_output=True, timeout=5)
        self.assertEqual(run.returncode, 0, run.stderr)
        return run.stdout.decode().strip()

    def test_original_failure_is_low_but_patched_numeric_sources_are_unknown(self):
        self.runtime()
        new = self.production()
        old = sources.fixture.undo_ip_scores('ip.sh', sources.fixture.undo_browser('ip.sh', new))
        for name in policy.NUMERIC:
            self.assertEqual(self.query(old, name, '{"error":"source unavailable"}'), 'null|LOW')
            for response in ('{}', '{"error":"source unavailable"}', '<html>403</html>', '', '{"success":false}'):
                with self.subTest(provider=name, response=response):
                    self.assertIn(self.query(new, name, response), ('|未知', '|'))

    def test_actual_numeric_parsers_preserve_thresholds_and_real_zero(self):
        self.runtime()
        new = self.production()
        old = sources.fixture.undo_ip_scores('ip.sh', sources.fixture.undo_browser('ip.sh', new))
        for name, (path, _) in policy.NUMERIC.items():
            for score in (0, 19, 20, 24, 25, 32, 33, 59, 60, 65, 66, 74, 75, 84, 85, 89, 90, 100):
                data = score
                for part in reversed(path.split('.')):
                    data = {part: data}
                response = json.dumps(data)
                with self.subTest(provider=name, score=score):
                    self.assertEqual(self.query(new, name, response), self.query(old, name, response))
            for score in ('0', None, False, -1, 101, 0.5):
                data = score
                for part in reversed(path.split('.')):
                    data = {part: data}
                self.assertEqual(self.query(new, name, json.dumps(data)), '|未知')

    def test_actual_ipapi_and_dbip_preserve_known_and_reject_unknown(self):
        self.runtime()
        new = self.production()
        for value, expected in [('0 (Low)', '0.00%|LOW'), ('0.001 (Very Low)', '0.10%|VERYLOW'),
                                ('1 (Very High)', '100.00%|VERYHIGH')]:
            self.assertEqual(self.query(new, 'ipapi', json.dumps({'company': {'abuser_score': value}})), expected)
        for value in ('low', 'medium', 'high'):
            self.assertEqual(self.query(new, 'dbip', json.dumps({'threatLevel': value})),
                             {'low': '0|LOW', 'medium': '50|MEDIUM', 'high': '100|HIGH'}[value])
        for name, response in [('ipapi', '{"company":{"abuser_score":"NaN (Low)"}}'),
                               ('ipapi', '{"company":{"abuser_score":"0 (Low)"},"error":"403"}'),
                               ('dbip', '{"threatLevel":"unknown"}'), ('dbip', '{"threatLevel":"low","success":false}')]:
            self.assertEqual(self.query(new, name, response), '|')
        with tempfile.TemporaryDirectory() as name:
            marker = Path(name) / 'must-not-exist'
            value = '0;system("touch ' + str(marker) + '");0 (Low)'
            self.assertEqual(self.query(new, 'ipapi', json.dumps({'company': {'abuser_score': value}})), '|')
            self.assertFalse(marker.exists())

    def test_text_unknown_rows_and_json_null_preserve_other_scores(self):
        self.runtime()
        text = self.production().decode()
        display = body(text, 'sscore_text') + body(text, 'show_score')
        updates = ''.join(re.findall(r'^score_updates\+=.*\n', text, re.M))
        self.assertEqual(len(updates.splitlines()), 6)
        for lite in (0, 1):
            script = policy.HELPERS.decode() + PRELUDE + display + '\nmode_lite=' + str(lite) + '\nshow_score\n:\n'
            run = subprocess.run(['/bin/bash'], input=script.encode(), capture_output=True, timeout=5)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertEqual(run.stderr, b'')
            rendered = run.stdout.decode()
            for label in ('Scamalytics', 'ipapi', 'DB-IP'):
                self.assertIn(label + '：未知', rendered)
            for label in ('IP2Location', 'AbuseIPDB', 'IPQS'):
                self.assertEqual(label + '：未知' in rendered, lite == 0)
            self.assertNotIn('LOW', rendered)
        for values, expected in [('', dict.fromkeys(('IP2LOCATION', 'SCAMALYTICS', 'ipapi', 'AbuseIPDB', 'IPQS', 'DBIP'))),
                                 ('scamalytics[score]=20; ipqs[score]=0; ipapi[ipqs]=99; ipapi[score]="0.10%"',
                                  {'IP2LOCATION': None, 'SCAMALYTICS': '20', 'ipapi': '0.10%', 'AbuseIPDB': None, 'IPQS': '0', 'DBIP': None})]:
            script = policy.HELPERS.decode() + PRELUDE + values + '\nscore_updates=""\n' + updates
            script += '\nprintf \'%s\' \'{"Score":{},"Other":"kept"}\' | jq "${score_updates} ."\n'
            run = subprocess.run(['/bin/bash'], input=script.encode(), capture_output=True, timeout=5)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertEqual(json.loads(run.stdout), {'Score': expected, 'Other': 'kept'})

    def test_production_identity_preserves_requests_and_other_functions(self):
        new = sources.fixture.undo_browser('ip.sh', self.production())
        old = sources.fixture.undo_ip_scores('ip.sh', new)
        self.assertEqual(hashlib.sha256(old).hexdigest(), policy.SOURCES['ip.sh']['source_sha256'])
        self.assertEqual(policy.transform('ip.sh', old), new)
        # The patch changes interpretation, display and serialization only.
        self.assertEqual(re.findall(rb'^.*curl .*$', old, re.M), re.findall(rb'^.*curl .*$', new, re.M))
        for name in ('db_ipinfo', 'db_ipregistry', 'show_type'):
            self.assertEqual(body(old.decode(), name), body(new.decode(), name))
        if subprocess.check_output(['/bin/bash', '-c', 'echo ${BASH_VERSINFO[0]}']).strip() != b'3':
            run = subprocess.run(['/bin/bash', '-n'], input=new, capture_output=True, timeout=5)
            self.assertEqual(run.returncode, 0, run.stderr)

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
            path = Path(name) / 'ip-score-policy.py'
            content = (PLUGIN / path.name).read_bytes()
            with mock.patch.object(helper, '__file__', str(path.with_name('native-source-helper.py'))):
                path.write_bytes(content)
                self.assertIn('transform', helper.ip_score_policy())
                for data in (content + b'!', b'x' * 65537):
                    path.write_bytes(data)
                    with self.assertRaises(ValueError):
                        helper.ip_score_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.ip_score_policy()
                path.symlink_to(PLUGIN / path.name)
                with self.assertRaises(OSError):
                    helper.ip_score_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.ip_score_policy()
        for invalid in (b'changed', 'text', b'x' * (helper.MAX_FILE + 4097)):
            with mock.patch.object(helper, 'ip_score_policy', return_value={
                    'SOURCES': policy.SOURCES, 'transform': lambda role, content: invalid}):
                with self.assertRaisesRegex(ValueError, 'served IP score policy output'):
                    helper.validated_ip_scores('ip.sh', b'input')

    def test_builder_rejects_missing_or_corrupt_helper(self):
        for missing in (True, False):
            fixture = sources.SourceTests(methodName='runTest')
            fixture.setUp()
            try:
                tree, env = fixture.build_tree()
                path = tree / 'plugins/nodequality/ip-score-policy.py'
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
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--readonly-upstream-dir', type=Path)
    args, remaining = parser.parse_known_args()
    READONLY_SOURCES = args.readonly_upstream_dir
    sys.argv = [sys.argv[0]] + remaining
    unittest.main()
