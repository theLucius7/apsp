#!/usr/bin/env python3
"""Verify swap refusal and failure propagation with bounded, unprivileged stubs."""
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


policy = module('swap_policy', PLUGIN / 'swap-policy.py')
helper = module('source_helper', PLUGIN / 'source-helper.py')
report = module('report_policy', PLUGIN / 'report-policy.py')
source_tests = module('source_tests', ROOT / 'tools/test-nodequality-sources.py')


class SwapTests(unittest.TestCase):
    def run_shell(self, text, directory, memory='447'):
        environment = {'PATH': os.environ['PATH'], 'LC_ALL': 'C',
                       'TEST_MEMORY': memory, 'TEST_DIRECTORY': str(directory)}
        return subprocess.run(['bash'], input=text, env=environment,
                              capture_output=True, timeout=4)

    def test_low_or_unknown_memory_stops_before_benchmark_and_never_touches_existing_file(self):
        prelude = b'''awk(){ printf '%s' "$TEST_MEMORY"; }
fallocate(){ printf forbidden >> "$TEST_DIRECTORY/trace"; }
dd(){ printf forbidden >> "$TEST_DIRECTORY/trace"; }
mkswap(){ printf forbidden >> "$TEST_DIRECTORY/trace"; }
swapon(){ printf forbidden >> "$TEST_DIRECTORY/trace"; }
swapoff(){ printf forbidden >> "$TEST_DIRECTORY/trace"; }
'''
        script = prelude + policy.MEMORY_GUARD + b'printf benchmark >> "$TEST_DIRECTORY/trace"\n}\ntest_cpu_gb5\n'
        for memory in ('', 'unavailable', '0', '447', '949', '950', '1200'):
            with self.subTest(memory=memory), tempfile.TemporaryDirectory() as name:
                root = Path(name)
                sentinel = root / '.gb5_tmp.swap'
                sentinel.write_bytes(b'owned sentinel: do not replace or unlink')
                run = self.run_shell(script, root, memory)
                allowed = memory in ('950', '1200')
                self.assertEqual(run.returncode, 0 if allowed else 70, run.stderr)
                trace = (root / 'trace').read_bytes() if (root / 'trace').exists() else b''
                self.assertEqual(trace, b'benchmark' if allowed else b'')
                self.assertEqual(sentinel.read_bytes(), b'owned sentinel: do not replace or unlink')
                if not allowed:
                    self.assertIn(b'insufficient available memory', run.stderr)

    def test_old_failed_swapon_leaves_small_file_but_new_refusal_does_not_allocate(self):
        prelude = b'''workdir=$TEST_DIRECTORY
osinfo=(kvm)
awk(){ case "$*" in *MemAvailable*) printf 447;; *SwapFree*) printf 0;; *) printf 10000;; esac; }
df(){ printf 'private disk fixture'; }
fallocate(){ printf allocated > "$3"; }
dd(){ printf forbidden >> "$TEST_DIRECTORY/trace"; return 1; }
chmod(){ :; }
mkswap(){ :; }
swapon(){ printf denied >> "$TEST_DIRECTORY/trace"; return 1; }
swapoff(){ printf forbidden >> "$TEST_DIRECTORY/trace"; return 1; }
'''
        for old in (True, False):
            with self.subTest(old=old), tempfile.TemporaryDirectory() as name:
                root = Path(name)
                prefix = policy.HARDWARE_PREFIX if old else policy.MEMORY_GUARD
                cleanup = policy.SWAP_CLEANUP if old else policy.NO_SWAP_CLEANUP
                script = prelude + prefix + b'cleanup_local(){\n' + cleanup + b':\n}\ncleanup_local\nreturn 0\n}\ntest_cpu_gb5\n'
                run = self.run_shell(script, root)
                self.assertEqual(run.returncode, 0 if old else 70, run.stderr)
                self.assertEqual((root / '.gb5_tmp.swap').exists(), old)
                if old:
                    self.assertEqual((root / '.gb5_tmp.swap').read_bytes(), b'allocated')
                    self.assertEqual((root / 'trace').read_bytes(), b'denied')
                else:
                    self.assertFalse((root / 'trace').exists())

    def test_entry_cleanup_keeps_unmounts_and_stops_calling_swapoff(self):
        for old in (True, False):
            with self.subTest(old=old), tempfile.TemporaryDirectory() as name:
                root = Path(name)
                swap_line = policy.ENTRY_REPLACEMENTS[0][0 if old else 1]
                script = b'''work_dir=$TEST_DIRECTORY
swapoff(){ printf swapoff\\n >> "$TEST_DIRECTORY/trace"; }
umount(){ printf unmount\\n >> "$TEST_DIRECTORY/trace"; }
clear_mount(){
''' + swap_line + b'''umount "$work_dir/BenchOs/proc/"
umount "$work_dir/BenchOs/sys/"
umount -R "$work_dir/BenchOs/dev/"
}
clear_mount
'''
                run = self.run_shell(script, root)
                self.assertEqual(run.returncode, 0, run.stderr)
                value = (root / 'trace').read_text()
                self.assertEqual(value.count('unmount'), 3)
                self.assertEqual('swapoff' in value, old)

    def test_memory_refusal_propagates_through_tee_and_runs_exit_cleanup(self):
        for old, status in ((True, 70), (False, 70), (False, 0)):
            with self.subTest(old=old, status=status), tempfile.TemporaryDirectory() as name:
                root = Path(name)
                script = b'''result_directory=$TEST_DIRECTORY
hardware_quality_filename=hardware.log
trap 'printf cleanup >> "$TEST_DIRECTORY/trace"' EXIT
run_HardwareQuality(){ printf 'hardware fixture\\n'; return ''' + str(status).encode() + b'''; }
main(){
''' + policy.ENTRY_REPLACEMENTS[2][0 if old else 1] + b'''printf next >> "$TEST_DIRECTORY/trace"
}
main
'''
                run = self.run_shell(script, root)
                denied = not old and status == 70
                self.assertEqual(run.returncode, 70 if denied else 0, run.stderr)
                self.assertEqual((root / 'trace').read_text(), 'cleanup' if denied else 'nextcleanup')
                self.assertEqual((root / 'hardware.log').read_bytes(), b'hardware fixture\n')

    def test_pinned_cleanup_with_real_observer_cannot_complete_after_hardware_refusal(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires previously verified, readonly pinned source cache')
        lock = json.loads((PLUGIN / 'source-lock.json').read_bytes())
        rows = helper.validate(lock)
        entry = helper.verified((READONLY_SOURCES / 'NodeQuality.sh').read_bytes(), rows['NodeQuality.sh'])
        lines = entry.splitlines(keepends=True)
        self.assertEqual(lines[439], b'function post_cleanup(){\n')
        self.assertEqual(lines[454], b'    exit 1\n')
        self.assertEqual(lines[462], b'\n')
        # Run only the exact cleanup functions, preserving their observer line
        # numbers. All mount, chroot and deletion operations are inert stubs.
        cleanup = b'\n' * 439 + b''.join(lines[439:462])
        prelude = b'''work_dir=$TEST_DIRECTORY/.nodequality-fixture
result_directory=$TEST_DIRECTORY
hardware_quality_filename=hardware.log
chroot_run(){ :; }
clear_mount(){ :; }
post_check_mount(){ :; }
rm(){ :; }
_green_bold(){ :; }
L(){ printf cleanup; }
run_HardwareQuality(){ printf 'hardware fixture\\n'; return "$TEST_STATUS"; }
main(){
trap 'sig_cleanup' INT TERM SIGHUP EXIT
'''
        tail = b'''printf next > "$TEST_DIRECTORY/trace"
post_cleanup
}
main
'''
        # The legacy status-70 guard behaves differently with Bash 3 and 5
        # DEBUG traps; status 7 is the stable failure-propagation control.
        cases = [('legacy-refusal', 0), ('legacy-refusal', 7),
                 ('fixed', 0), ('fixed', 70), ('fixed', 7)]
        for guard, status in cases:
            with self.subTest(guard=guard, status=status), tempfile.TemporaryDirectory() as name:
                root = Path(name)
                (root / '.runner').mkdir()
                path = root / 'cleanup-fixture.sh'
                if guard == 'legacy-refusal':
                    line = (policy.ENTRY_REPLACEMENTS[2][0].rstrip(b'\n')
                            + b'; [[ ${PIPESTATUS[0]} != 70 ]] || exit 70\n')
                else:
                    line = policy.ENTRY_REPLACEMENTS[2][1]
                path.write_bytes(cleanup + prelude + line + tail)
                environment = {'PATH': os.environ['PATH'], 'LC_ALL': 'C',
                               'TEST_DIRECTORY': name, 'TEST_STATUS': str(status),
                               'BASH_ENV': str(PLUGIN / 'exit-observer.sh'),
                               'APSP_REPORT_UPSTREAM': str(path),
                               'APSP_REPORT_WORKSPACE': name}
                run = subprocess.run(['bash', str(path)], env=environment,
                                     capture_output=True, timeout=4)
                # The pinned EXIT cleanup ends with exit 1 for both paths.
                # Only normal main cleanup is allowed to mark completion.
                self.assertEqual(run.returncode, 1, run.stderr)
                completed = guard == 'legacy-refusal' or status == 0
                self.assertEqual((root / 'trace').exists(), completed)
                self.assertEqual((root / '.runner/upstream-completed').exists(), completed)
                self.assertEqual((root / 'hardware.log').read_bytes(), b'hardware fixture\n')

    def test_hardware_source_failure_cannot_be_hidden_by_an_empty_child_script(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            script = b'''result_directory=$TEST_DIRECTORY
hardware_quality_filename=hardware.log
run_HardwareQuality(){ return_failure | accept_empty_input; }
return_failure(){ return 1; }
accept_empty_input(){ cat >/dev/null; }
main(){
''' + policy.ENTRY_REPLACEMENTS[2][1] + b'''printf next > "$TEST_DIRECTORY/trace"
}
main
'''
            environment = {'PATH': os.environ['PATH'], 'LC_ALL': 'C',
                           'TEST_DIRECTORY': name,
                           'BASH_ENV': str(PLUGIN / 'exit-observer.sh'),
                           'APSP_REPORT_UPSTREAM': 'inert-fixture',
                           'APSP_REPORT_WORKSPACE': name}
            run = subprocess.run(['bash'], input=script, env=environment,
                                 capture_output=True, timeout=4)
            self.assertEqual(run.returncode, 1, run.stderr)
            self.assertFalse((root / 'trace').exists())
            self.assertEqual((root / 'hardware.log').read_bytes(), b'')

    def test_transform_requires_exact_identity_anchors_and_final_hash(self):
        role = 'NodeQuality.sh'
        original = source_tests.fixture.swap_anchors(role)
        expected = policy.patch(role, original)
        spec = {'source_sha256': hashlib.sha256(original).hexdigest(),
                'patched_sha256': hashlib.sha256(expected).hexdigest()}
        with mock.patch.dict(policy.SOURCES, {role: spec}, clear=True):
            self.assertEqual(policy.transform(role, original), expected)
            for value in (original + b'!', 'text', b'x' * (policy.MAX_SOURCE + 1)):
                with self.assertRaises(ValueError):
                    policy.transform(role, value)
            with self.assertRaises(ValueError):
                policy.transform('unknown', original)
            duplicated = original + policy.ENTRY_REPLACEMENTS[0][0]
            with mock.patch.dict(spec, source_sha256=hashlib.sha256(duplicated).hexdigest()):
                with self.assertRaisesRegex(ValueError, 'exactly once'):
                    policy.transform(role, duplicated)
            with mock.patch.dict(spec, patched_sha256='0' * 64):
                with self.assertRaisesRegex(ValueError, 'output SHA256'):
                    policy.transform(role, original)

    def test_helper_file_and_return_value_boundaries(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            path = root / 'swap-policy.py'
            original = (PLUGIN / path.name).read_bytes()
            with mock.patch.object(helper, '__file__', str(root / 'source-helper.py')):
                path.write_bytes(original)
                self.assertIn('transform', helper.swap_policy())
                for data in (original + b'!', b'x' * 65537):
                    path.write_bytes(data)
                    with self.assertRaises(ValueError):
                        helper.swap_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.swap_policy()
                path.symlink_to(PLUGIN / path.name)
                with self.assertRaises(OSError):
                    helper.swap_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.swap_policy()
        for invalid in (b'bad', 'text', b'x' * (helper.MAX_FILE + 2049)):
            with mock.patch.object(helper, 'swap_policy', return_value={
                    'transform': lambda *_: invalid, 'SOURCES': policy.SOURCES}):
                with self.assertRaisesRegex(ValueError, 'output SHA256 or byte limit'):
                    helper.without_swap('NodeQuality.sh', b'ignored')

    def test_builder_rejects_missing_or_modified_swap_policy_before_archive(self):
        for corrupt in (False, True):
            fixture = source_tests.SourceTests(methodName='runTest')
            fixture.setUp()
            try:
                tree, env = fixture.build_tree()
                path = tree / 'plugins/nodequality/swap-policy.py'
                if corrupt:
                    path.write_bytes(path.read_bytes() + b'!')
                else:
                    path.unlink()
                result = fixture.build(tree, env, 'arm64')
                self.assertNotEqual(result.returncode, 0)
                output = fixture.root / 'artifacts/nodequality' / source_tests.VERSION
                self.assertFalse((output / 'arm64').exists())
                self.assertFalse((output / 'SHA256SUMS').exists())
            finally:
                fixture.tearDown()

    def test_fixed_production_sources_preserve_other_bytes_and_cleanup_line(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires previously verified, readonly pinned source cache')
        lock = json.loads((PLUGIN / 'source-lock.json').read_bytes())
        bundle = helper.decode(helper.pack(lock, READONLY_SOURCES))
        entry = (READONLY_SOURCES / 'NodeQuality.sh').read_bytes()
        patched_entry = helper.entrypoint(bundle)
        self.assertEqual(entry.splitlines()[454], b'    exit 1')
        self.assertEqual(patched_entry.splitlines()[454], b'    exit 1')
        restored = source_tests.fixture.undo_loader('NodeQuality.sh', patched_entry)
        restored = source_tests.fixture.undo_dependencies('NodeQuality.sh', restored, entry)
        for before, after in reversed(policy.ENTRY_REPLACEMENTS):
            restored = policy.replace_once(restored, after, before)
        self.assertEqual(restored, entry)
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name) / 'sources'
            helper.materialize(bundle, directory)
            served = helper.serve(directory, ['-Ls', 'https://Hardware.Check.Place'])
            prior = report.transform('hardware.sh', (directory / 'hardware.sh').read_bytes())
            restored = source_tests.fixture.undo_ranking('hardware.sh', served)
            restored = source_tests.fixture.undo_dependencies('hardware.sh', restored, prior)
            restored = policy.replace_once(restored, policy.NO_SWAP_CLEANUP, policy.SWAP_CLEANUP)
            restored = policy.replace_once(restored, policy.MEMORY_GUARD, policy.HARDWARE_PREFIX)
            self.assertEqual(restored, prior)
            for token in (b'swapon ', b'swapoff ', b'mkswap ', b'.gb5_tmp.swap'):
                self.assertNotIn(token, served)
            self.assertEqual((directory / 'NodeQuality.sh').read_bytes(), entry)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument('--readonly-upstream-dir', type=Path)
    args, remaining = parser.parse_known_args()
    READONLY_SOURCES = args.readonly_upstream_dir
    sys.argv = [sys.argv[0]] + remaining
    unittest.main()
