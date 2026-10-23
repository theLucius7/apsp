#!/usr/bin/env python3
"""Check fixed dependency refusal without running installers or benchmarks."""
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
PLUGIN = ROOT / 'plugins/nodequality'
READONLY_SOURCES = None


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


policy = module('dependency_policy', PLUGIN / 'dependency-policy.py')
helper = module('source_helper', PLUGIN / 'source-helper.py')
report = module('report_policy', PLUGIN / 'report-policy.py')
swap = module('swap_policy', PLUGIN / 'swap-policy.py')
sources = module('source_tests', ROOT / 'tools/test-nodequality-sources.py')


class DependencyTests(unittest.TestCase):
    def shell(self, script, directory, **extra):
        env = {'PATH': str(directory / 'bin'), 'TRACE': str(directory / 'trace'),
               'LC_ALL': 'C', **extra}
        return subprocess.run(['/bin/bash'], input=script, env=env,
                              capture_output=True, timeout=4)

    def binaries(self, directory, names):
        binaries = directory / 'bin'
        binaries.mkdir()
        for name in names:
            path = binaries / name
            path.write_text('#!/bin/bash\nprintf executed >> "$TRACE"\n')
            path.chmod(0o700)

    def test_required_tools_are_checked_without_execution_and_each_missing_tool_is_named(self):
        for role, required in policy.REQUIREMENTS.items():
            required = list(required) + (['sysbench', 'geekbench5'] if role == 'hardware.sh' else [])
            for missing in [None, *required]:
                with self.subTest(role=role, missing=missing), tempfile.TemporaryDirectory() as name:
                    root = Path(name)
                    self.binaries(root, [tool for tool in required if tool != missing])
                    run = self.shell(policy.checks(role) + b'install_dependencies\nprintf continued\n', root)
                    self.assertEqual(run.returncode, 0 if missing is None else 70, run.stderr)
                    self.assertEqual(run.stdout, b'continued' if missing is None else b'')
                    if missing is not None:
                        self.assertIn(('missing offline dependencies: ' + missing + ';').encode(), run.stderr)
                    self.assertFalse((root / 'trace').exists(), 'presence checks must not run tools')

    def test_hardware_modes_keep_original_required_capabilities_without_silently_selecting_fast_mode(self):
        combinations = [(0, 0, 0, ['sysbench', 'geekbench5']),
                        (0, 0, 1, ['sysbench', 'geekbench5', 'curl-impersonate', 'update-ca-certificates']),
                        (0, 1, 0, ['sysbench']), (1, 0, 0, [])]
        for fast, privacy, verbose, optional in combinations:
            with self.subTest(fast=fast, privacy=privacy, verbose=verbose), tempfile.TemporaryDirectory() as name:
                root = Path(name)
                self.binaries(root, [*policy.REQUIREMENTS['hardware.sh'], *optional])
                prelude = f'mode_fast={fast}; mode_privacy={privacy}; mode_verbose={verbose}\n'.encode()
                run = self.shell(prelude + policy.checks('hardware.sh') + b'install_dependencies\nprintf "%s:%s:%s" "$mode_fast" "$mode_privacy" "$mode_verbose"\n', root)
                self.assertEqual(run.returncode, 0, run.stderr)
                self.assertEqual(run.stdout, f'{fast}:{privacy}:{verbose}'.encode())
                self.assertFalse((root / 'trace').exists())

    def test_direct_calls_to_all_installers_fail_without_commands_or_downloads(self):
        for role, installers in policy.INSTALLERS.items():
            for installer in installers:
                with self.subTest(role=role, installer=installer), tempfile.TemporaryDirectory() as name:
                    root = Path(name)
                    self.binaries(root, ('curl', 'wget', 'apt', 'apt-get', 'dnf', 'yum', 'apk', 'brew', 'chmod', 'tar'))
                    run = self.shell(policy.checks(role) + (installer + '\nprintf continued\n').encode(), root)
                    self.assertEqual(run.returncode, 70)
                    self.assertIn(b'runtime dependency installation is disabled', run.stderr)
                    self.assertEqual(run.stdout, b'')
                    self.assertFalse((root / 'trace').exists())

    def test_no_flag_cannot_bypass_a_missing_dependency(self):
        for role in policy.REQUIREMENTS:
            for mode_no in (0, 1):
                with self.subTest(role=role, mode_no=mode_no), tempfile.TemporaryDirectory() as name:
                    root = Path(name)
                    self.binaries(root, [])
                    run = self.shell(f'mode_no={mode_no}\n'.encode() + policy.checks(role) + policy.REQUIRED_CALL + b'printf continued\n', root)
                    self.assertEqual(run.returncode, 70)
                    self.assertEqual(run.stdout, b'')

    def test_root_loader_checks_trace_tool_instead_of_downloading_or_chmod(self):
        prelude = b'''chroot_run(){
printf '%s\\n' "$*" >> "$TRACE"
[[ $1 != test ]] || return "$AVAILABLE"
}
load_3rd_program(){
'''
        for old in (True, False):
            for available in ('0', '1'):
                with self.subTest(old=old, available=available), tempfile.TemporaryDirectory() as name:
                    root = Path(name)
                    self.binaries(root, [])
                    lines = b''.join(pair[0 if old else 1] for pair in policy.ENTRY_REPLACEMENTS[:2])
                    run = self.shell(prelude + lines + b'}\n' + policy.ENTRY_REPLACEMENTS[2][1] + b'printf continued\n', root, AVAILABLE=available)
                    denied = not old and available == '1'
                    self.assertEqual(run.returncode, 70 if denied else 0, run.stderr)
                    calls = (root / 'trace').read_text()
                    if old:
                        self.assertIn('wget https://github.com/', calls)
                        self.assertIn('chmod u+x', calls)
                    else:
                        self.assertEqual(calls, 'test -x /usr/local/bin/nexttrace\n')
                    self.assertEqual(run.stdout, b'' if denied else b'continued')

    def test_chapter_failure_reaches_real_exit_observer_without_a_false_complete_marker(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires the already verified readonly pinned source cache')
        entry = helper.verified((READONLY_SOURCES / 'NodeQuality.sh').read_bytes(),
                                helper.validate(json.loads((PLUGIN / 'source-lock.json').read_bytes()))['NodeQuality.sh'])
        lines = entry.splitlines(keepends=True)
        self.assertEqual(lines[439], b'function post_cleanup(){\n')
        cleanup = b'\n' * 439 + b''.join(lines[439:462])
        for before, after in policy.ENTRY_REPLACEMENTS[3:]:
            function = before.split()[0].decode()
            for old, status in ((True, 70), (False, 70), (False, 7), (False, 0)):
                with self.subTest(function=function, old=old, status=status), tempfile.TemporaryDirectory() as name:
                    root = Path(name)
                    (root / '.runner').mkdir()
                    path = root / 'upstream-fixture.sh'
                    prelude = b'''work_dir=$TEST_DIRECTORY/.nodequality-fixture
result_directory=$TEST_DIRECTORY
ip_quality_filename=ip.log
net_quality_filename=net.log
backroute_trace_filename=trace.log
chroot_run(){ :; }
clear_mount(){ :; }
post_check_mount(){ :; }
rm(){ :; }
_green_bold(){ :; }
L(){ printf cleanup; }
'''
                    prelude += (function + '(){ printf fixture; return '+str(status)+'; }\nmain(){\n').encode()
                    prelude += b"trap 'sig_cleanup' INT TERM SIGHUP EXIT\n"
                    path.write_bytes(cleanup + prelude + (before if old else after) + b'printf next > "$TEST_DIRECTORY/next"\npost_cleanup\n}\nmain\n')
                    run = subprocess.run(['/bin/bash', str(path)], env={'PATH':os.environ['PATH'], 'TEST_DIRECTORY':name,
                        'BASH_ENV':str(PLUGIN/'exit-observer.sh'), 'APSP_REPORT_UPSTREAM':str(path), 'APSP_REPORT_WORKSPACE':name},
                        capture_output=True, timeout=4)
                    self.assertEqual(run.returncode, 1, run.stderr)
                    completed = old or status == 0
                    self.assertEqual((root/'next').exists(), completed)
                    self.assertEqual((root/'.runner/upstream-completed').exists(), completed)

    def test_exact_production_transforms_keep_every_byte_outside_installers_guards_and_calls(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires the already verified readonly pinned source cache')
        lock = json.loads((PLUGIN / 'source-lock.json').read_bytes())
        bundle = helper.decode(helper.pack(lock, READONLY_SOURCES))
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)/'sources'
            helper.materialize(bundle, directory)
            for role in policy.SOURCES:
                original = (directory/role).read_bytes()
                prior = report.transform(role, original) if role in report.SOURCES else original
                if role in swap.SOURCES:
                    prior = swap.transform(role, prior)
                if role == 'NodeQuality.sh':
                    patched = helper.entrypoint(bundle)
                    self.assertEqual(patched.splitlines()[454], b'    exit 1')
                else:
                    url = next(url for url, value in helper.ALIASES.items() if value == role)
                    patched = helper.serve(directory, ['-Ls', url])
                    start, stop = policy.installer_span(role, prior)
                    self.assertGreater(stop-start, len(policy.checks(role)))
                patched = sources.fixture.undo_netflix(role, patched)
                patched = sources.fixture.undo_ip_scores(role, patched)
                patched = sources.fixture.undo_ranking(role, patched)
                patched = sources.fixture.undo_loader(role, patched)
                patched = sources.fixture.undo_data(role, patched, {path.name: path.read_bytes() for path in directory.iterdir()})
                self.assertEqual(hashlib.sha256(patched).hexdigest(), policy.SOURCES[role]['patched_sha256'])
                self.assertEqual(sources.fixture.undo_dependencies(role, patched, prior), prior)
                self.assertEqual((directory/role).read_bytes(), original, 'canonical licensed source remains intact')
                if role == 'hardware.sh':
                    start = original.index(b'adaptoslocale(){\n')
                    stop = original.index(b'check_connectivity(){\n')
                    self.assertIn(original[start:stop], patched)

    def test_patched_production_scripts_pass_supported_bash_syntax(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires the already verified readonly pinned source cache')
        major = subprocess.check_output(['/bin/bash', '-c', 'printf "%s" "${BASH_VERSINFO[0]}"'], text=True)
        if int(major) < 4:
            self.skipTest('upstream requires Bash >= 4; run this check on the Debian test node')
        lock = json.loads((PLUGIN / 'source-lock.json').read_bytes())
        bundle = helper.decode(helper.pack(lock, READONLY_SOURCES))
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name) / 'sources'
            helper.materialize(bundle, directory)
            for role in policy.SOURCES:
                if role == 'NodeQuality.sh':
                    patched = helper.entrypoint(bundle)
                else:
                    url = next(url for url, value in helper.ALIASES.items() if value == role)
                    patched = helper.serve(directory, ['-Ls', url])
                with self.subTest(role=role):
                    syntax = subprocess.run(['/bin/bash', '-n'], input=patched, capture_output=True, timeout=4)
                    self.assertEqual(syntax.returncode, 0, syntax.stderr)

    def test_identity_anchors_and_output_hash_fail_closed(self):
        role = 'NodeQuality.sh'
        original = sources.fixture.dependency_anchors(role)
        expected = policy.patch(role, original)
        spec = {'source_sha256':hashlib.sha256(original).hexdigest(),'patched_sha256':hashlib.sha256(expected).hexdigest()}
        with mock.patch.dict(policy.SOURCES, {role:spec}):
            self.assertEqual(policy.transform(role,original),expected)
            for invalid in (original+b'!', b'x'*(policy.MAX_SOURCE+1), 'text'):
                with self.assertRaises(ValueError):policy.transform(role,invalid)
            duplicate = original + policy.ENTRY_REPLACEMENTS[0][0]
            with mock.patch.dict(spec, source_sha256=hashlib.sha256(duplicate).hexdigest()):
                with self.assertRaisesRegex(ValueError,'exactly once'):policy.transform(role,duplicate)
            with mock.patch.dict(spec, patched_sha256='0'*64):
                with self.assertRaisesRegex(ValueError,'output SHA256'):policy.transform(role,original)
        with self.assertRaises(ValueError):policy.transform('unknown',b'')

    def test_helper_file_boundaries_and_served_output_are_verified(self):
        with tempfile.TemporaryDirectory() as name:
            root=Path(name);path=root/'dependency-policy.py';content=(PLUGIN/path.name).read_bytes()
            with mock.patch.object(helper,'__file__',str(root/'source-helper.py')):
                path.write_bytes(content);self.assertIn('transform',helper.dependency_policy())
                for data in (content+b'!',b'x'*65537):
                    path.write_bytes(data)
                    with self.assertRaises(ValueError):helper.dependency_policy()
                path.unlink()
                with self.assertRaises(OSError):helper.dependency_policy()
                path.symlink_to(PLUGIN/path.name)
                with self.assertRaises(OSError):helper.dependency_policy()
                path.unlink();os.mkfifo(path,0o600)
                with self.assertRaisesRegex(ValueError,'ordinary'):helper.dependency_policy()
        for invalid in (b'bad','text',b'x'*(helper.MAX_FILE+4097)):
            with mock.patch.object(helper,'dependency_policy',return_value={'transform':lambda *_:invalid,'SOURCES':policy.SOURCES}):
                with self.assertRaisesRegex(ValueError,'output SHA256 or byte limit'):helper.offline_dependencies('NodeQuality.sh',b'ignored')

    def test_builder_rejects_missing_or_modified_dependency_helper(self):
        for corrupt in (False,True):
            fixture=sources.SourceTests(methodName='runTest');fixture.setUp()
            try:
                tree,env=fixture.build_tree();path=tree/'plugins/nodequality/dependency-policy.py'
                if corrupt:path.write_bytes(path.read_bytes()+b'!')
                else:path.unlink()
                run=fixture.build(tree,env,'arm64');self.assertNotEqual(run.returncode,0)
                output=fixture.root/'artifacts/nodequality'/sources.VERSION
                self.assertFalse((output/'arm64').exists());self.assertFalse((output/'SHA256SUMS').exists())
            finally:fixture.tearDown()


if __name__ == '__main__':
    parser=argparse.ArgumentParser(add_help=False)
    parser.add_argument('--readonly-upstream-dir',type=Path)
    args,remaining=parser.parse_known_args()
    READONLY_SOURCES=args.readonly_upstream_dir
    sys.argv=[sys.argv[0]]+remaining
    unittest.main()
