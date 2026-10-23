#!/usr/bin/env python3
"""Exercise source refusal before any consumer starts, using inert scripts."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
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


policy = module('loader_policy', PLUGIN / 'loader-policy.py')
helper = module('loader_source_helper', PLUGIN / 'source-helper.py')
source_tests = module('loader_source_tests', ROOT / 'tools/test-nodequality-sources.py')
PAYLOAD = b'printf executed > "$EXECUTED"\nprintf fixture-report\n\n\n'
PRELUDE = r'''
raw_file_prefix=https://raw.githubusercontent.com/LloydAsp/NodeQuality/refs/heads/main
opt_ipv=-6
opt_lang=-E
run_net_quality_test=L
params=' -V'
payload=fixture
hardware_quality_json_filename=hardware.json
ip_quality_json_filename=ip.json
net_quality_json_filename=net.json
backroute_trace_json_filename=route.json
curl(){ "$PYTHON" "$TOOL" --produce "$@"; }
chroot_run(){
[[ $1 != umount ]] || return 0
printf called >> "$CALLS"
if [[ $1 == bash && ${2:-} == /dev/fd/* ]];then
  "$PYTHON" "$TOOL" --consume "$@" < "$2"
else
  "$PYTHON" "$TOOL" --consume "$@"
fi
}
'''


def produce():
    mode = os.environ['MODE']
    Path(os.environ['PRODUCED']).write_text(json.dumps(sys.argv[2:]))
    Path(os.environ['READY']).touch()
    if mode == 'hold':
        time.sleep(30)
    if not mode.startswith('empty'):
        sys.stdout.buffer.write(PAYLOAD)
        sys.stdout.buffer.flush()
    time.sleep(0.04)
    Path(os.environ['DONE']).touch()
    raise SystemExit(70 if mode.endswith('failure') else 0)


def consume():
    early = not Path(os.environ['DONE']).exists()
    payload = sys.stdin.buffer.read()
    Path(os.environ['CONSUMED']).write_text(json.dumps({
        'argv': sys.argv[2:], 'started_before_producer_done': early,
        'sha256': hashlib.sha256(payload).hexdigest(), 'bytes': len(payload)}))
    raise SystemExit(subprocess.run(['/bin/bash', '-s'], input=payload, timeout=3).returncode)


class LoaderTests(unittest.TestCase):
    def environment(self, directory, mode):
        return dict(os.environ, MODE=mode, PYTHON=sys.executable, TOOL=str(Path(__file__).resolve()),
                    **{name: str(directory / name.lower()) for name in (
                        'PRODUCED', 'CONSUMED', 'EXECUTED', 'CALLS', 'READY', 'DONE')})

    def invoke(self, directory, index, mode, patched=True):
        path = directory / 'entry.sh'
        call = policy.REPLACEMENTS[index][int(patched)]
        path.write_bytes(PRELUDE.encode() + b'\ninvoke(){\n' + call + b'}\ninvoke\n')
        (directory / '.runner').mkdir()
        env = self.environment(directory, mode)
        env.update(BASH_ENV=str(PLUGIN / 'exit-observer.sh'), APSP_REPORT_UPSTREAM=str(path),
                   APSP_REPORT_WORKSPACE=str(directory))
        return subprocess.run(['/bin/bash', str(path)], env=env, capture_output=True, timeout=5)

    def test_empty_and_partial_failures_never_start_any_of_five_consumers(self):
        for index, (role, _) in enumerate(policy.REQUESTS):
            for mode in ('empty_failure', 'partial_failure', 'empty_success'):
                for patched in (False, True):
                    with self.subTest(role=role, mode=mode, patched=patched), tempfile.TemporaryDirectory() as name:
                        directory = Path(name)
                        run = self.invoke(directory, index, mode, patched)
                        self.assertEqual((directory / 'calls').exists(), not patched, run.stderr)
                        self.assertEqual((directory / 'executed').exists(), not patched and mode == 'partial_failure')
                        if patched:
                            self.assertEqual(run.returncode, 70, run.stderr)
                            self.assertEqual(run.stdout, b'')
                        self.assertTrue((directory / 'produced').exists())

    def test_success_waits_for_complete_source_and_preserves_all_bytes_and_arguments_once(self):
        for index, (role, request) in enumerate(policy.REQUESTS):
            with self.subTest(role=role), tempfile.TemporaryDirectory() as name:
                directory = Path(name)
                run = self.invoke(directory, index, 'success')
                self.assertEqual(run.returncode, 0, run.stderr)
                self.assertEqual(run.stdout, b'fixture-report')
                self.assertEqual((directory / 'calls').read_text(), 'called')
                row = json.loads((directory / 'consumed').read_text())
                self.assertFalse(row['started_before_producer_done'])
                self.assertEqual(row['sha256'], hashlib.sha256(PAYLOAD).hexdigest())
                self.assertEqual(row['bytes'], len(PAYLOAD))
                expected_url = request.strip('"').replace('$raw_file_prefix',
                    'https://raw.githubusercontent.com/LloydAsp/NodeQuality/refs/heads/main')
                self.assertEqual(json.loads((directory / 'produced').read_text()), ['-Ls', expected_url])
                if role == 'hardware':
                    self.assertEqual(row['argv'], ['env NQENV=fixture bash -s -- -E  -V -y -o /result/hardware.json'])
                else:
                    self.assertEqual(row['argv'][0], 'bash')
                    self.assertTrue(row['argv'][1].startswith('/dev/fd/'))
                    expected = {'header': [], 'ip': ['-6', '-E', '-y', '-o', '/result/ip.json'],
                                'net': ['-6', '-E', '-V', '-y', '-o', '/result/net.json'],
                                'route': ['-6', '-E', '-R', '-n', '-S', '123', '-o', '/result/route.json']}[role]
                    self.assertEqual(row['argv'][2:], expected)

    def test_cancellation_during_source_delivery_starts_no_consumer(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            script = PRELUDE.encode() + b'\ninvoke(){\n' + policy.REPLACEMENTS[2][1] + b'}\ninvoke\n'
            process = subprocess.Popen(['/bin/bash'], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                       stderr=subprocess.PIPE, env=self.environment(directory, 'hold'), start_new_session=True)
            try:
                process.stdin.write(script)
                process.stdin.close()
                process.stdin = None
                deadline = time.monotonic() + 4
                while not (directory / 'ready').exists() and process.poll() is None and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue((directory / 'ready').exists())
                self.assertEqual(os.getpgid(process.pid), process.pid)
                os.killpg(process.pid, signal.SIGTERM)
                process.communicate(timeout=4)
                self.assertNotEqual(process.returncode, 0)
                self.assertFalse((directory / 'calls').exists())
                self.assertFalse((directory / 'executed').exists())
            finally:
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.communicate(timeout=4)

    def production_entry(self):
        if READONLY_SOURCES is None:
            self.skipTest('requires the previously verified readonly 17-file source cache')
        lock = json.loads((PLUGIN / 'source-lock.json').read_bytes())
        return helper.entrypoint(helper.decode(helper.pack(lock, READONLY_SOURCES)))

    def production_functions(self, entry):
        text = entry.decode()
        for name in ('run_header', 'run_HardwareQuality', 'run_ip_quality', 'run_net_quality', 'run_net_trace'):
            start = text.index('function ' + name + '(){\n')
            stop = text.index('\n}\n', start) + 3
            yield name, text[start:stop]

    def test_production_entry_is_line_preserving_reversible_and_valid_shell(self):
        entry = self.production_entry()
        self.assertEqual(entry.splitlines()[454], b'    exit 1')
        self.assertEqual(hashlib.sha256(entry).hexdigest(), policy.SOURCES['NodeQuality.sh']['patched_sha256'])
        before = source_tests.fixture.undo_loader('NodeQuality.sh', entry)
        self.assertEqual(hashlib.sha256(before).hexdigest(), policy.SOURCES['NodeQuality.sh']['source_sha256'])
        self.assertEqual(len(before.splitlines()), len(entry.splitlines()))
        syntax = subprocess.run(['/bin/bash', '-n'], input=entry, capture_output=True, timeout=3)
        self.assertEqual(syntax.returncode, 0, syntax.stderr)

    def test_real_cleanup_observer_stops_after_refusal_and_keeps_earlier_report(self):
        entry = self.production_entry()
        original = (READONLY_SOURCES / 'NodeQuality.sh').read_bytes().splitlines(keepends=True)
        cleanup = b'\n' * 439 + b''.join(original[439:462])
        for name, function in self.production_functions(entry):
            main_call = next(line for line in entry.decode().splitlines()
                             if (name + ' > ' if name == 'run_header' else name + ' | tee ') in line)
            for mode in ('partial_failure', 'success'):
                with self.subTest(function=name, mode=mode), tempfile.TemporaryDirectory() as temp:
                    directory = Path(temp)
                    (directory / '.runner').mkdir()
                    earlier = directory / 'earlier-report.log'
                    earlier.write_text('previous completed chapter')
                    stubs = r'''
work_dir=$APSP_REPORT_WORKSPACE/nodequality-fixture
result_directory=$APSP_REPORT_WORKSPACE
header_info_filename=chapter.log
hardware_quality_filename=chapter.log
ip_quality_filename=chapter.log
net_quality_filename=chapter.log
backroute_trace_filename=chapter.log
clear_mount(){ :; }
post_check_mount(){ :; }
rm(){ :; }
_green_bold(){ :; }
L(){ printf cleanup; }
pre_fetch_info(){ osinfo=(fixture); meminfo=(fixture); diskinfo=(fixture); }
'''
                    script = cleanup + (PRELUDE + stubs + function + '\nmain(){\n'
                        + "trap 'sig_cleanup' INT TERM SIGHUP EXIT\n"
                        + main_call + '\n'
                        + 'printf next > "$APSP_REPORT_WORKSPACE/next"\npost_cleanup\n}\nmain\n').encode()
                    path = directory / 'entry.sh'
                    path.write_bytes(script)
                    env = self.environment(directory, mode)
                    env.update(BASH_ENV=str(PLUGIN / 'exit-observer.sh'), APSP_REPORT_UPSTREAM=str(path),
                               APSP_REPORT_WORKSPACE=str(directory))
                    run = subprocess.run(['/bin/bash', str(path)], env=env, capture_output=True, timeout=5)
                    self.assertEqual(run.returncode, 1, run.stderr)
                    expected = mode == 'success'
                    self.assertEqual((directory / 'next').exists(), expected)
                    self.assertEqual((directory / '.runner/upstream-completed').exists(), expected)
                    self.assertEqual((directory / 'executed').exists(), expected)
                    self.assertEqual(earlier.read_text(), 'previous completed chapter')

    def test_real_source_helper_refusal_reaches_all_five_original_functions(self):
        entry = self.production_entry()
        lock = json.loads((PLUGIN / 'source-lock.json').read_bytes())
        bundle = helper.decode(helper.pack(lock, READONLY_SOURCES))
        corruptions = ('header.sh', 'hardware.sh', 'ip-iso3166.json', 'net-iperf.json', 'net.sh')
        for (name, function), corrupt in zip(self.production_functions(entry), corruptions):
            with self.subTest(function=name, corrupt=corrupt), tempfile.TemporaryDirectory() as temp:
                directory = Path(temp)
                materialized = directory / 'sources'
                helper.materialize(bundle, materialized)
                path = materialized / corrupt
                path.write_bytes(path.read_bytes()[:-1] + b'!')
                # Never execute real upstream code, even if the refusal regresses.
                stubs = r'''
curl(){ "$PYTHON" "$SOURCE_HELPER" serve "$SOURCE_DIRECTORY" "$@"; }
chroot_run(){ printf called > "$CALLS"; return 99; }
pre_fetch_info(){ osinfo=(fixture); meminfo=(fixture); diskinfo=(fixture); }
'''
                env = self.environment(directory, 'unused')
                env.update(SOURCE_HELPER=str(PLUGIN / 'source-helper.py'), SOURCE_DIRECTORY=str(materialized))
                run = subprocess.run(['/bin/bash'], input=(PRELUDE + stubs + function + '\n' + name + '\n').encode(),
                                     env=env, capture_output=True, timeout=5)
                self.assertEqual(run.returncode, 1, run.stderr)
                self.assertIn(b'checksum or size mismatch: ' + corrupt.encode(), run.stderr)
                self.assertEqual(run.stdout, b'')
                self.assertFalse((directory / 'calls').exists())

    def test_identity_unique_anchors_and_output_hash_fail_closed(self):
        source = b''.join(before for before, _ in policy.REPLACEMENTS)
        role = 'NodeQuality.sh'
        spec = {'source_sha256': hashlib.sha256(source).hexdigest(),
                'patched_sha256': hashlib.sha256(policy.patch(source)).hexdigest()}
        with mock.patch.dict(policy.SOURCES, {role: spec}):
            self.assertEqual(policy.transform(role, source), policy.patch(source))
            for invalid in (source + b'!', b'x' * (policy.MAX_SOURCE + 1), 'text'):
                with self.assertRaises(ValueError):
                    policy.transform(role, invalid)
            with self.assertRaisesRegex(ValueError, 'unique'):
                policy.patch(source + source)
            with mock.patch.dict(spec, patched_sha256='0' * 64):
                with self.assertRaisesRegex(ValueError, 'output SHA256'):
                    policy.transform(role, source)
        with self.assertRaises(ValueError):
            policy.transform('unknown', b'')

    def test_helper_file_boundaries_are_rejected(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            path = root / 'loader-policy.py'
            content = (PLUGIN / path.name).read_bytes()
            with mock.patch.object(helper, '__file__', str(root / 'source-helper.py')):
                path.write_bytes(content)
                self.assertIn('transform', helper.loader_policy())
                for data in (content + b'!', b'x' * 65537):
                    path.write_bytes(data)
                    with self.assertRaises(ValueError):
                        helper.loader_policy()
                path.unlink()
                with self.assertRaises(OSError):
                    helper.loader_policy()
                path.symlink_to(PLUGIN / path.name)
                with self.assertRaises(OSError):
                    helper.loader_policy()
                path.unlink()
                os.mkfifo(path, 0o600)
                with self.assertRaisesRegex(ValueError, 'ordinary'):
                    helper.loader_policy()

    def test_entrypoint_rejects_invalid_loader_return(self):
        fixture = source_tests.SourceTests(methodName='runTest')
        fixture.setUp()
        try:
            private = module('private_loader_helper', fixture.fixture_plugin / 'source-helper.py')
            bundle = helper.decode(fixture.bundle_path.read_bytes())
            valid_policy = private.loader_policy()
            for invalid in (b'changed', 'text', b'x' * (helper.MAX_FILE + 4097)):
                with mock.patch.object(private, 'loader_policy', return_value={
                        **valid_policy, 'transform': lambda role, content: invalid}):
                    with self.assertRaisesRegex(ValueError, 'entrypoint loader output'):
                        private.entrypoint(bundle)
        finally:
            fixture.tearDown()

    def test_builder_rejects_missing_or_changed_loader_helper(self):
        for missing in (True, False):
            fixture = source_tests.SourceTests(methodName='runTest')
            fixture.setUp()
            try:
                tree, env = fixture.build_tree()
                path = tree / 'plugins/nodequality/loader-policy.py'
                if missing:
                    path.unlink()
                else:
                    path.write_bytes(path.read_bytes() + b'!')
                run = fixture.build(tree, env, 'arm64')
                self.assertNotEqual(run.returncode, 0)
                output = fixture.root / 'artifacts/nodequality' / source_tests.VERSION
                self.assertFalse((output / 'arm64').exists())
                self.assertFalse((output / 'SHA256SUMS').exists())
            finally:
                fixture.tearDown()


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == '--produce':
        produce()
    elif len(sys.argv) > 1 and sys.argv[1] == '--consume':
        consume()
    else:
        parser = argparse.ArgumentParser(add_help=False)
        parser.add_argument('--readonly-upstream-dir', type=Path)
        args, remaining = parser.parse_known_args()
        READONLY_SOURCES = args.readonly_upstream_dir
        sys.argv = [sys.argv[0]] + remaining
        unittest.main()
