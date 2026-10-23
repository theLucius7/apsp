#!/usr/bin/env python3
"""Verify the OpenAI refusal against fixed sources without provider requests."""
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
BASH = os.environ.get('APSP_NODEQUALITY_TEST_BASH', '/bin/bash')


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


policy = module('openai_policy_test', PLUGIN / 'openai-policy.py')
helper = module('openai_source_helper_test', PLUGIN / 'native-source-helper.py')

PRELUDE = r'''
declare -A chatgpt smedia
smedia[nodata]=''
curl(){ printf called >> "$MARKER"; return 97; }
wget(){ printf called >> "$MARKER"; return 97; }
Check_DNS_1(){ printf called >> "$MARKER"; return 97; }
Check_DNS_2(){ printf called >> "$MARKER"; return 97; }
Check_DNS_3(){ printf called >> "$MARKER"; return 97; }
Get_Unlock_Type(){ printf called >> "$MARKER"; return 97; }
show_progress_bar(){ printf called >> "$MARKER"; return 97; }
kill_progress_bar(){ printf called >> "$MARKER"; return 97; }
head_updates=''
basic_updates=''
type_updates=''
score_updates=''
factor_updates=''
media_updates=''
mail_updates=''
'''


def definition(content, name):
    pattern = r'(?m)^(?:function )?' + re.escape(name) + r'\(\)\{\n[^\0]*?^\}\n'
    matches = list(re.finditer(pattern, content))
    if len(matches) != 1:
        raise AssertionError('requires unique complete fixture function')
    return matches[0].group()


class OpenAIProfileTests(unittest.TestCase):
    def runtime(self):
        if int(subprocess.check_output([BASH, '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'], text=True)) < 4:
            self.skipTest('fixed scripts require Bash >= 4; run on dedicated Debian')
        if not shutil.which('jq'):
            self.skipTest('metadata verification requires jq')

    def production(self, *, before=False):
        if READONLY_SOURCES is None:
            self.skipTest('requires the verified readonly 17-file upstream cache')
        with tempfile.TemporaryDirectory(prefix='apsp-openai-source-') as temporary:
            directory = Path(temporary) / 'sources'
            lock = helper.decode((PLUGIN / 'source-lock.json').read_bytes())
            # The actual signed source loader checks every raw source and license;
            # serving transforms bytes without running the upstream script.
            helper.materialize(helper.decode(helper.pack(lock, READONLY_SOURCES)), directory)
            if before:
                with mock.patch.object(helper, 'authorized_openai', side_effect=lambda role, content: content):
                    return helper.serve(directory, ['-Ls', 'https://IP.Check.Place'])
            return helper.serve(directory, ['-Ls', 'https://IP.Check.Place'])

    def shell(self, script, *, strict=False, ip='192.0.2.1'):
        with tempfile.TemporaryDirectory(prefix='apsp-openai-no-request-') as temporary:
            marker = Path(temporary) / 'external-request'
            options = 'set -e -o pipefail\nshopt -s inherit_errexit\n' if strict else ''
            environment = {'PATH': os.environ['PATH'], 'LC_ALL': 'C', 'MARKER': str(marker), 'IP': ip}
            run = subprocess.run([BASH], input=options + PRELUDE + script, text=True,
                                 env=environment, capture_output=True, timeout=5)
            self.assertEqual(run.returncode, 0, 'owned refusal fixture did not complete')
            self.assertFalse(marker.exists(), 'OpenAI refusal contacted a dependency')
            return run.stdout

    def function(self):
        final = self.production().decode()
        return definition(final, 'apsp_openai_not_attempted') + definition(final, 'OpenAITest')

    def metadata(self, output):
        rows = [json.loads(line) for line in output.splitlines() if line.startswith('{')]
        self.assertEqual(len(rows), 1)
        return rows[0]

    def assert_unknown(self, row, target):
        self.assertEqual(row['provider'], 'openai')
        self.assertEqual(row['status'], 'unknown')
        self.assertEqual(row['target_ip'], target)
        self.assertIs(type(row['checked_at']), int)
        self.assertGreater(row['checked_at'], 0)
        self.assertIs(row['Attempted'], False)
        self.assertIsNone(row['last_attempt_at'])
        self.assertIsNone(row['elapsed_seconds'])
        self.assertEqual(row['Attempts'], [])
        self.assertEqual(row['Error']['category'], 'credential_not_configured')
        self.assertEqual(row['Error']['phase'], 'not_attempted')
        self.assertIn('未知', row['Error']['message'])
        for name in ('score', 'clean', 'http_status', 'curl_exit'):
            self.assertNotIn(name, row)

    def test_fixed_canonical_bytes_are_retained_and_only_the_executor_and_metadata_change(self):
        before = self.production(before=True)
        after = self.production()
        lock = helper.validate(helper.decode((PLUGIN / 'source-lock.json').read_bytes()))
        original = helper.verified(helper.ordinary(READONLY_SOURCES / 'ip.sh', helper.MAX_FILE), lock['ip.sh'])
        # Never put an upstream credential-bearing block into an assertion diff.
        original_function = definition(original.decode(), 'OpenAITest')
        self.assertTrue('cookie' in original_function.lower(), 'fixed original boundary changed')
        self.assertEqual(hashlib.sha256(before).hexdigest(), policy.SOURCES['ip.sh']['source_sha256'])
        self.assertEqual(hashlib.sha256(after).hexdigest(), policy.SOURCES['ip.sh']['patched_sha256'])
        self.assertTrue(policy.transform('ip.sh', before) == after, 'final OpenAI layer differs')
        start, stop = policy.function_span('OpenAITest', before)
        expected = before[:start] + policy.HELPERS + policy.FUNCTION_REPLACEMENTS['OpenAITest'] + before[stop:]
        for old, new in policy.METADATA_REPLACEMENTS:
            self.assertEqual(expected.count(old), 1)
            expected = expected.replace(old, new, 1)
        self.assertTrue(expected == after, 'policy changed bytes outside the declared boundaries')
        for name in ('MediaUnlockTest_YouTube_Premium', 'apsp_access_youtube_fetch',
                     'MediaUnlockTest_Netflix', 'apsp_netflix_fetch', 'db_ipinfo',
                     'db_ipqs', 'show_score', 'show_media'):
            self.assertTrue(definition(before.decode(), name) == definition(after.decode(), name),
                            'policy changed an unrelated function: ' + name)
        self.assertTrue(helper.ordinary(READONLY_SOURCES / 'ip.sh', helper.MAX_FILE) == original,
                        'readonly canonical source was modified')

    def test_ipv4_and_ipv6_calls_clear_stale_results_without_a_request(self):
        self.runtime()
        program = self.function()
        for family, target in (('4', '192.0.2.1'), ('6', '2001:db8::1')):
            with self.subTest(family=family):
                script = program + '''chatgpt=([ustatus]=SUCCESS [uregion]=STALE [utype]=STALE [score]=0 [clean]=true [attempts]='[{"error":null}]')
OpenAITest ''' + family + '''
printf '%s\n' "${chatgpt[access]}"
printf '%s\n' "${chatgpt[ustatus]}" "${chatgpt[uregion]}" "${chatgpt[utype]}" "${chatgpt[score]-}" "${chatgpt[clean]-}"
'''
                output = self.shell(script, ip=target)
                self.assert_unknown(self.metadata(output), target)
                self.assertEqual(output.splitlines()[-5:], ['未知', '', '', '', ''])

    def test_strict_direct_call_keeps_the_rest_of_the_report(self):
        self.runtime()
        program = self.function() + '''OpenAITest 4
printf '%s\n' "${chatgpt[access]}"
printf '%s\n' COMPLETED_OTHER_CHAPTER
'''
        output = self.shell(program, strict=True)
        self.assert_unknown(self.metadata(output), '192.0.2.1')
        self.assertEqual(output.splitlines()[-1], 'COMPLETED_OTHER_CHAPTER')

    def test_repeated_calls_never_reuse_a_success_or_invent_an_attempt(self):
        self.runtime()
        program = self.function() + '''OpenAITest 4
chatgpt[access]='{"Attempted":true,"last_attempt_at":1,"score":0,"clean":true}'
OpenAITest 4
printf '%s\n' "${chatgpt[access]}"
'''
        self.assert_unknown(self.metadata(self.shell(program)), '192.0.2.1')

    def test_save_json_publishes_the_reason_and_retains_other_valid_zero_and_false_results(self):
        self.runtime()
        final = self.production()
        self.assertIn(policy.JSON_ANCHOR + policy.JSON_ADDITION, final)
        program = self.function() + 'save_fixture(){\n' + (policy.JSON_ANCHOR + policy.JSON_ADDITION).decode() + '}\n'
        original = {'Score': {'IPQS': 0}, 'Factor': {'Proxy': {'IPQS': False}},
                    'Media': {'ChatGPT': {'Status': 'SUCCESS', 'Region': 'STALE', 'Type': 'STALE'},
                              'Netflix': {'Status': 'YES', 'Region': 'US'},
                              'Youtube': {'Status': 'YES', 'Region': 'US'}},
                    'Sources': {'Other': {'status': 'succeeded', 'value': False}},
                    'CompletedChapter': {'complete': True, 'revision': 7}}
        program += "ipjson='" + json.dumps(original) + "'\nOpenAITest 4\nsave_fixture\nprintf '%s\\n' \"$ipjson\"|jq -c\n"
        result = self.metadata(self.shell(program, strict=True))
        self.assert_unknown(result['Sources']['OpenAI'], '192.0.2.1')
        self.assertEqual(result['Media']['ChatGPT']['Status'], '未知')
        self.assertIsNone(result['Media']['ChatGPT']['Region'])
        self.assertIsNone(result['Media']['ChatGPT']['Type'])
        self.assertEqual(result['Media']['ChatGPT']['Error']['category'], 'credential_not_configured')
        for key in ('Score', 'Factor', 'CompletedChapter'):
            self.assertEqual(result[key], original[key])
        for key in ('Netflix', 'Youtube'):
            self.assertEqual(result['Media'][key], original['Media'][key])
        self.assertEqual(result['Sources']['Other'], original['Sources']['Other'])

    def test_an_unselected_media_probe_still_has_honest_not_attempted_metadata(self):
        self.runtime()
        program = self.function() + 'save_fixture(){\n' + (policy.JSON_ANCHOR + policy.JSON_ADDITION).decode() + '}\n'
        program += '''ipjson='{"Media":{"Youtube":{"Status":"retained"}}}'
save_fixture
printf '%s\n' "$ipjson"|jq -c
'''
        result = self.metadata(self.shell(program, strict=True))
        self.assert_unknown(result['Sources']['OpenAI'], '192.0.2.1')
        self.assertEqual(result['Media']['Youtube'], {'Status': 'retained'})

    def test_executable_replacement_contains_no_network_or_authorization_path(self):
        replacement = policy.HELPERS + policy.FUNCTION_REPLACEMENTS['OpenAITest']
        self.assertNotRegex(replacement.decode(), r'(?i)curl|wget|cookie|user-agent|sec-ch-ua|retry|Check_DNS')
        self.assertEqual(policy.FUNCTIONS, ('OpenAITest',))

    def test_unique_boundaries_hashes_roles_and_byte_limits_fail_closed(self):
        content = b'function OpenAITest(){\n:;\n}\n' + policy.JSON_ANCHOR
        identity = {'source_sha256': hashlib.sha256(content).hexdigest(),
                    'patched_sha256': hashlib.sha256(policy.patch(content)).hexdigest()}
        with mock.patch.dict(policy.SOURCES, {'ip.sh': identity}):
            self.assertEqual(policy.transform('ip.sh', content), policy.patch(content))
            for invalid in (content + b'!', 'text', b'x' * (policy.MAX_SOURCE + 1)):
                with self.assertRaises(ValueError):
                    policy.transform('ip.sh', invalid)
            with mock.patch.dict(identity, patched_sha256='0' * 64):
                with self.assertRaisesRegex(ValueError, 'output SHA256'):
                    policy.transform('ip.sh', content)
        for invalid in (content + content, content.replace(b'\n}\n', b'\n\0}\n'),
                        content.replace(policy.JSON_ANCHOR, b''), content + policy.JSON_ANCHOR):
            with self.assertRaises(ValueError):
                policy.patch(invalid)
        with self.assertRaises(ValueError):
            policy.function_span('unrelated', content)
        with self.assertRaises(ValueError):
            policy.transform('hardware.sh', content)

    def test_helper_requires_the_exact_bounded_ordinary_file(self):
        with tempfile.TemporaryDirectory(prefix='apsp-openai-helper-') as temporary:
            path = Path(temporary) / 'openai-policy.py'
            original = (PLUGIN / path.name).read_bytes()
            with mock.patch.object(helper, '__file__', str(path.with_name('native-source-helper.py'))):
                path.write_bytes(original)
                self.assertIn('transform', helper.openai_policy())
                for value in (original + b'!', b'x' * 65537):
                    path.write_bytes(value)
                    with self.assertRaises(ValueError):
                        helper.openai_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.openai_policy()
                path.symlink_to(PLUGIN / path.name)
                with self.assertRaises(OSError):
                    helper.openai_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.openai_policy()

    def test_helper_rejects_invalid_outputs_and_leaves_non_ip_roles_untouched(self):
        for invalid in (b'changed', 'text', b'x' * (helper.MAX_FILE + 4097)):
            with mock.patch.object(helper, 'openai_policy', return_value={
                    'SOURCES': policy.SOURCES, 'transform': lambda role, content: invalid}):
                with self.assertRaisesRegex(ValueError, 'served OpenAI policy output'):
                    helper.authorized_openai('ip.sh', b'input')
        with mock.patch.object(helper, 'openai_policy', side_effect=AssertionError('unexpected policy read')):
            self.assertEqual(helper.authorized_openai('net.sh', b'unchanged'), b'unchanged')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--readonly-upstream-dir', type=Path)
    args, remaining = parser.parse_known_args()
    READONLY_SOURCES = args.readonly_upstream_dir
    sys.argv = [sys.argv[0]] + remaining
    unittest.main()
