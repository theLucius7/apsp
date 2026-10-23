#!/usr/bin/env python3
"""Verify explicit unavailable queries without contacting third-party services."""
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


policy = module('public_access_policy', PLUGIN / 'public-access-policy.py')
helper = module('public_access_source_helper', PLUGIN / 'source-helper.py')
sources = module('public_access_source_tests', ROOT / 'tools/test-nodequality-sources.py')


def definition(text, name):
    match = re.search(r'(?m)^(?:function )?' + re.escape(name) + r'\(\)\{\n.*?^\}\n', text, re.S)
    if not match:
        raise AssertionError('missing fixed function: ' + name)
    return match[0]


class PublicAccessTests(unittest.TestCase):
    def runtime(self, *, jq=False):
        if int(subprocess.check_output(['/bin/bash', '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'], text=True)) < 4:
            self.skipTest('fixed scripts require Bash >= 4; run on dedicated Debian')
        if jq and not shutil.which('jq'):
            self.skipTest('metadata verification requires jq')

    def production(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires the verified readonly 17-file upstream cache')
        with tempfile.TemporaryDirectory() as name:
            target = Path(name) / 'sources'
            lock = helper.decode((PLUGIN / 'source-lock.json').read_bytes())
            helper.materialize(helper.decode(helper.pack(lock, READONLY_SOURCES)), target)
            return helper.serve(target, ['-Ls', 'https://IP.Check.Place'])

    def shell(self, script):
        self.runtime()
        with tempfile.TemporaryDirectory() as name:
            marker = Path(name) / 'external-called'
            prelude = '''declare -A ipregistry dbip disney youtube chatgpt smedia
smedia=([bad]=FAILED [nodata]=UNKNOWN)
ibar_step=0
curl(){ printf called >> "$MARKER"; return 99; }
wget(){ printf called >> "$MARKER"; return 99; }
Check_DNS_1(){ printf called >> "$MARKER"; return 99; }
Check_DNS_2(){ printf called >> "$MARKER"; return 99; }
Check_DNS_3(){ printf called >> "$MARKER"; return 99; }
show_progress_bar(){ printf called >> "$MARKER"; return 99; }
'''
            run = subprocess.run(['/bin/bash'], input=prelude + script, text=True,
                                 env=dict(os.environ, MARKER=str(marker)), capture_output=True, timeout=4)
            self.assertEqual(run.returncode, 0, run.stderr)
            self.assertFalse(marker.exists(), 'unconfigured node query contacted a dependency')
            return run.stdout

    def test_each_production_query_clears_stale_result_and_returns_without_requests(self):
        text = self.production().decode()
        for function, array in policy.FUNCTIONS.items():
            for family in ('4', '6'):
                with self.subTest(function=function, family=family):
                    script = definition(text, function.removeprefix('function '))
                    script += array + '=([score]=99 [proxy]=true [ustatus]=SUCCESS [uregion]=STALE [utype]=STALE)\n'
                    script += function.removeprefix('function ') + ' ' + family + '\n'
                    script += "printf '%s\\n' \"${" + array + '[score]-}\" "${' + array + '[proxy]-}\" "${' + array + '[reason]}\"\n'
                    values = self.shell(script).splitlines()
                    self.assertEqual(values, ['', '', policy.REASONS[array]])

    def test_read_ref_does_not_fetch_cookies_or_retry_and_clears_inherited_material(self):
        text = self.production().decode()
        script = definition(text, 'read_ref')
        script += "Cookie=(PUBLIC_TEST_ONLY_OLD_COOKIE)\nread_ref\nprintf '%s\\n' \"${#Cookie[@]}\"\n"
        self.assertEqual(self.shell(script), '0\n')

    def test_json_reports_each_source_as_not_attempted_and_keeps_other_results(self):
        self.runtime(jq=True)
        prelude = ''
        for function, array in policy.FUNCTIONS.items():
            prelude += function + '(){\n' + policy.refusal(array).decode() + '}\n'
            prelude += function.removeprefix('function ') + ' 4\n'
        values = self.shell(prelude + policy.ACCESS_HELPER.decode() + '\napsp_provider_access_json\n')
        access = json.loads(values)
        self.assertEqual(set(access), {'ipregistry', 'DBIP', 'DisneyPlus', 'Youtube', 'ChatGPT'})
        for row in access.values():
            self.assertEqual(row['Status'], 'not_attempted')
            self.assertEqual(row['ErrorKind'], 'credential_not_configured')
            self.assertEqual(row['Execution'], 'node_self')
            self.assertIs(row['Attempted'], False)
            self.assertIn('未知', row['Reason'])
        original = {'Score': {'DBIP': None, 'IPQS': '0'}, 'Factor': {'Proxy': {'DBIP': None}},
                    'Media': {'Netflix': {'Status': 'SUCCESS', 'Region': 'US'}, 'DisneyPlus': {'Status': 'STALE'}},
                    'Type': {'Usage': {'ipregistry': 'STALE', 'IPinfo': 'business'}}}
        run = subprocess.run(['jq', '--argjson', 'access', values, policy.ACCESS_FILTER],
                             input=json.dumps(original), text=True, capture_output=True, timeout=3)
        self.assertEqual(run.returncode, 0, run.stderr)
        result = json.loads(run.stdout)
        self.assertEqual(result['Media']['Netflix'], original['Media']['Netflix'])
        self.assertEqual(result['Score'], original['Score'])
        self.assertEqual(result['Type']['Usage']['IPinfo'], 'business')
        self.assertIsNone(result['Type']['Usage']['ipregistry'])
        for provider in policy.MEDIA:
            self.assertEqual(result['Media'][provider]['Status'], '未知')
            self.assertIsNone(result['Media'][provider]['Region'])
            self.assertIsNone(result['Media'][provider]['Type'])
            self.assertEqual(result['Media'][provider]['Reason'], access[provider]['Reason'])

    def test_production_identity_and_original_sources_are_retained(self):
        new = self.production()
        prior = sources.fixture.undo_access('ip.sh', new)
        self.assertEqual(hashlib.sha256(prior).hexdigest(), policy.SOURCES['ip.sh']['source_sha256'])
        self.assertEqual(policy.transform('ip.sh', prior), new)
        for function in ('db_ipqs', 'db_abuseipdb', 'MediaUnlockTest_Netflix', 'MediaUnlockTest_Reddit'):
            self.assertEqual(definition(new.decode(), function), definition(prior.decode(), function))
        for name, array in policy.FUNCTIONS.items():
            self.assertIn(definition(prior.decode(), name.removeprefix('function ')).split('\n', 1)[1],
                          definition(new.decode(), name.removeprefix('function ')))
        run = subprocess.run(['/bin/bash', '-n'], input=new, capture_output=True, timeout=3)
        self.assertEqual(run.returncode, 0, run.stderr)

    def test_hashes_roles_anchors_and_result_fail_closed(self):
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
            policy.transform('hardware.sh', source)

    def test_helper_boundaries_and_invalid_output_are_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            path = Path(name) / 'public-access-policy.py'
            content = (PLUGIN / path.name).read_bytes()
            with mock.patch.object(helper, '__file__', str(path.with_name('source-helper.py'))):
                path.write_bytes(content)
                self.assertIn('transform', helper.public_access_policy())
                for value in (content + b'!', b'x' * 65537):
                    path.write_bytes(value)
                    with self.assertRaises(ValueError):
                        helper.public_access_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.public_access_policy()
                path.symlink_to(PLUGIN / path.name)
                with self.assertRaises(OSError):
                    helper.public_access_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.public_access_policy()
        for invalid in (b'changed', 'text', b'x' * (helper.MAX_FILE + 16385)):
            with mock.patch.object(helper, 'public_access_policy', return_value={
                    'SOURCES': policy.SOURCES, 'transform': lambda role, content: invalid}):
                with self.assertRaisesRegex(ValueError, 'served public access policy output'):
                    helper.authorized_access('ip.sh', b'input')

    def test_builder_rejects_missing_or_corrupt_helper(self):
        for missing in (True, False):
            fixture = sources.SourceTests(methodName='runTest')
            fixture.setUp()
            try:
                tree, env = fixture.build_tree()
                path = tree / 'plugins/nodequality/public-access-policy.py'
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
