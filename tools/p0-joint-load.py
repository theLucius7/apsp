#!/usr/bin/env python3
"""Bounded, TEST_ONLY joint load on the isolated Debian 12 acceptance guest."""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import pwd
import re
import runpy
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid

SOURCE = 'b5289a93a85445d0ca2fcb5d5e817f650abef9a3'
HEARTBEAT_LIMIT = 30
PAYLOAD_BYTES = 1024
PERIOD = 1
TOOLS = Path(__file__).resolve().parent
SMOKE = runpy.run_path(str(TOOLS / 'agent-smoke.py'))
NATIVE = runpy.run_path(str(TOOLS / 'native-service-smoke.py'))


def digest(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def command(args, timeout=10):
    return subprocess.run(args, check=True, capture_output=True, text=True,
                          timeout=timeout).stdout


def properties(unit):
    keys = 'LoadState,MainPID,ControlPID,ControlGroup,ActiveState,NRestarts,CPUWeight,OOMScoreAdjust,MemoryMax,MemorySwapMax,TasksMax'
    text = command(['systemctl', 'show', unit, '-p', keys])
    return dict(line.split('=', 1) for line in text.splitlines() if '=' in line)


def diagnostic_units():
    text = command(['systemctl', 'list-units', '--all', '--no-legend', '--plain',
                    '--no-pager', 'apsp-diagnostic-*.service'])
    pattern = r'apsp-diagnostic-[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\.service'
    return [line.split()[0] for line in text.splitlines()
            if line.split() and re.fullmatch(pattern, line.split()[0])]


def await_condition(check, seconds, description):
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        value = check()
        if value:
            return value
        time.sleep(0.5)
    raise AssertionError('Deadline exceeded: ' + description)


class Ledger:
    def __init__(self):
        self.lock = threading.RLock()
        self.phase = 'prepare'
        self.heartbeats, self.samples, self.attempts, self.traffic, self.events = [], [], [], [], []

    def record(self, rows, **value):
        with self.lock:
            rows.append(dict(phase=self.phase, monotonic=time.monotonic(),
                             received_at=time.time(), **value))

    def change(self, phase):
        with self.lock:
            self.phase = phase
            self.record(self.events, event='phase_start')
        print(json.dumps({'phase': phase}), flush=True)


class Messages(list):
    def __init__(self, panel):
        super().__init__()
        self.panel = panel

    def append(self, message):
        if message.get('type') == 'heartbeat':
            self.panel.ledger.record(self.panel.ledger.heartbeats,
                                    session=self.panel.sessions.get(threading.get_ident(), 0))
        super().append(message)


class Samples(dict):
    def __init__(self, ledger):
        super().__init__()
        self.ledger = ledger

    def update(self, values):
        for identifier, sample in values:
            self.ledger.record(self.ledger.samples, id=identifier,
                               sampled_at_ms=sample.get('sampled_at'))
            super().update({identifier: sample})


class Handler(SMOKE['Handler']):
    def do_GET(self):
        if not self.server.accepting:
            self.close_connection = True
            return self.reply({'error': 'bounded fixture outage'}, 503)
        if self.path in ('/api/agent/v1/commands', '/api/agent/v1/probes', '/api/agent/v1/probes?authorization=1'):
            if self.authorized():
                self.reply([])
            return
        super().do_GET()

    def do_POST(self):
        if not self.server.accepting:
            self.close_connection = True
            return self.reply({'error': 'bounded fixture outage'}, 503)
        if self.path == '/api/agent/v1/telemetry':
            if not self.authorized():
                return
            length = int(self.headers.get('Content-Length', 0))
            assert 0 < length <= 256 * 1024
            assert self.headers.get('Content-Encoding') == 'gzip'
            with gzip.GzipFile(fileobj=io.BytesIO(self.rfile.read(length))) as body:
                payload = body.read(1024 * 1024 + 1)
            assert len(payload) <= 1024 * 1024
            samples = json.loads(payload)['samples']
            acknowledge = self.server.acknowledge
            for sample in samples:
                self.server.ledger.record(self.server.ledger.attempts, id=sample['id'],
                                          sampled_at_ms=sample.get('sampled_at'),
                                          acknowledged=acknowledge)
            self.server.seen.update(sample['id'] for sample in samples)
            if not acknowledge:
                return self.reply({'error': 'fixture outage'}, 503)
            self.server.samples.update((sample['id'], sample) for sample in samples)
            return self.reply({'ids': [sample['id'] for sample in samples]})
        super().do_POST()

    def websocket(self):
        with self.server.connections_lock:
            self.server.session_count += 1
            self.server.sessions[threading.get_ident()] = self.server.session_count
            self.server.connections.add(self.connection)
            self.server.ledger.record(self.server.ledger.events, event='ws_open', session=self.server.session_count)
        try:
            super().websocket()
        finally:
            with self.server.connections_lock:
                self.server.connections.discard(self.connection)
                self.server.sessions.pop(threading.get_ident(), None)


class Panel(SMOKE['Panel']):
    def __init__(self, ledger):
        self.accepting = True
        self.ledger = ledger
        self.connections_lock = threading.Lock()
        self.connections, self.sessions, self.session_count = set(), {}, 0
        super().__init__()
        self.RequestHandlerClass = Handler
        self.messages, self.samples = Messages(self), Samples(ledger)

    def disconnect(self):
        self.accepting = False
        with self.connections_lock:
            for connection in list(self.connections):
                try:
                    connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass


def transfer_guard(port):
    # The only worker target is the private loopback runtime; no configurable host.
    started = time.monotonic()
    try:
        result = subprocess.run([sys.executable, str(Path(__file__).resolve()),
                                 '--echo-worker', str(port)], stdout=subprocess.DEVNULL,
                                stderr=subprocess.DEVNULL, timeout=3)
        return dict(ok=result.returncode == 0, exit_code=result.returncode,
                    elapsed=time.monotonic() - started, payload_bytes=PAYLOAD_BYTES)
    except subprocess.TimeoutExpired:
        # subprocess.run kills and reaps the single worker before returning.
        return dict(ok=False, timed_out=True, elapsed=time.monotonic() - started,
                    payload_bytes=PAYLOAD_BYTES)


def traffic_worker(ledger, stop, port):
    while not stop.is_set():
        try:
            measurement = transfer_guard(port)
        except Exception as error:
            # A failed process launch is a failed measurement, not a missing row.
            measurement = dict(ok=False, error_type=type(error).__name__,
                               payload_bytes=PAYLOAD_BYTES)
        ledger.record(ledger.traffic, **measurement)
        stop.wait(PERIOD)


def agent_status(binary, config):
    try:
        return json.loads(command([str(binary), '--config', str(config), 'status'], timeout=2))
    except (subprocess.SubprocessError, ValueError):
        return None


def evaluate(ledger):
    with ledger.lock:
        beats, samples, traffic = list(ledger.heartbeats), list(ledger.samples), list(ledger.traffic)
    baseline = [b for b in beats if b['phase'] == 'baseline']
    assert len(baseline) >= 3 and baseline[-1]['monotonic'] - baseline[0]['monotonic'] >= 39
    protected = [b for b in beats if b['phase'] in ['baseline', 'diagnostic_fixtures']]
    assert len({b['session'] for b in protected}) == 1, 'unexpected reconnect during joint load'
    assert not any(row.get('event') == 'ws_open' and row['phase'] in ['baseline', 'diagnostic_fixtures'] for row in ledger.events), 'unexpected websocket opened during protected phase'
    assert all(row['phase'] in ['prepare', 'agent_restart', 'panel_disconnect']
               for row in ledger.events if row.get('event') == 'ws_open'), 'unplanned reconnect'
    for phase in ['prepare', 'agent_restart', 'panel_disconnect']:
        assert sum(row.get('event') == 'ws_open' and row['phase'] == phase for row in ledger.events) == 1, 'unexpected session count'
    recovery = [b for b in beats if b['phase'] == 'recovery']
    assert recovery, 'missing recovery heartbeat'
    restored = [b for b in beats if b['session'] == recovery[0]['session']]
    assert len(restored) >= 2 and all(b['phase'] in ['panel_disconnect', 'recovery'] for b in restored), 'two actual heartbeats on the recovered session required'
    assert restored[-1]['monotonic'] - restored[0]['monotonic'] >= 19, 'recovered heartbeat period was not observed'
    assert len({b['session'] for b in recovery}) == 1, 'recovery session changed'
    sessions = {}
    for beat in beats:
        sessions.setdefault(beat['session'], []).append(beat['monotonic'])
    gaps = [b - a for times in sessions.values() for a, b in zip(times, times[1:])]
    assert gaps and max(gaps) <= HEARTBEAT_LIMIT, 'actual heartbeat interval exceeded 30s'
    assert len(samples) >= 3
    identities = {}
    latest = 0
    for sample in samples:
        stamp = sample['sampled_at_ms']
        assert isinstance(stamp, int) and stamp > 0, 'missing actual sampling timestamp'
        if sample['id'] in identities:
            assert identities[sample['id']] == stamp, 'replay changed the original timestamp'
        else:
            assert stamp >= latest, 'new sampling timestamp moved backwards'
            identities[sample['id']] = stamp
            latest = stamp
    assert traffic and all(row['ok'] for row in traffic), 'a bounded proxy echo failed'
    for phase in ['baseline', 'diagnostic_fixtures', 'agent_restart', 'panel_disconnect', 'recovery']:
        assert any(row['phase'] == phase for row in traffic), 'missing proxy traffic in ' + phase
    return dict(heartbeat_count=len(beats), baseline_heartbeat_count=len(baseline),
                baseline_span_seconds=baseline[-1]['monotonic'] - baseline[0]['monotonic'],
                max_connected_session_heartbeat_gap_seconds=max(gaps),
                global_heartbeat_gaps_seconds=[b['monotonic'] - a['monotonic'] for a, b in zip(beats, beats[1:])],
                sampling_receipts=len(samples), unique_sampling_ids=len(identities),
                proxy_echo_count=len(traffic), proxy_echo_failures=0,
                proxy_payload_bytes_per_direction=PAYLOAD_BYTES)


def verify_sampling_attempts(ledger):
    identities = {}
    for attempt in ledger.attempts:
        stamp = attempt['sampled_at_ms']
        assert isinstance(stamp, int) and stamp > 0, 'missing pre-ACK sampling timestamp'
        if attempt['id'] in identities:
            assert identities[attempt['id']] == stamp, 'pre-ACK timestamp changed during replay'
        identities[attempt['id']] = stamp
    assert identities, 'missing pre-ACK telemetry attempts'
    return identities


def classify_oom(kernel):
    lines = kernel.splitlines()
    hints = {i for i, line in enumerate(lines) if re.search('oom|out of memory|Killed process', line, re.I)}
    if not hints:
        return dict(status='none', allowed=True, lines=[])
    covered, start, killed = set(), None, set()
    for i, line in enumerate(lines):
        if 'invoked oom-killer:' in line:
            if start is not None:
                return dict(status='unknown_or_disallowed', allowed=False, lines=[lines[x] for x in sorted(hints)])
            start = i
        match = re.search(r'Memory cgroup out of memory: Killed process (\d+) ', line)
        if match and start is not None:
            events = [entry for entry in lines[start:i + 1] if 'oom-kill:constraint=' in entry]
            pattern = r'constraint=CONSTRAINT_MEMCG,.*oom_memcg=/system\.slice/apsp-diagnostic-[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}\.service,.*pid=' + match[1] + r',uid='
            if len(events) == 1 and re.search(pattern, events[0]):
                covered.update(range(start, i + 1)); killed.add(match[1])
            start = None
        reaper = re.search(r'oom_reaper: reaped process (\d+) ', line)
        if reaper and reaper[1] in killed:
            covered.add(i)
    allowed = start is None and bool(killed) and hints.issubset(covered)
    return dict(status='diagnostic_memcg' if allowed else 'unknown_or_disallowed',
                allowed=allowed, lines=[lines[x] for x in sorted(hints)])


def cleanup_unit(unit):
    record = dict(unit=unit, clean=False)
    try:
        status = subprocess.run(['systemctl', 'stop', '--', unit], capture_output=True, timeout=10)
        record['stop_exit_code'] = status.returncode
    except Exception as error:
        record['stop_error'] = type(error).__name__
    try:
        after = properties(unit)
        record['properties'] = after
        record['clean'] = after.get('MainPID') == '0' and after.get('ControlPID') == '0'
        if after.get('ControlGroup'):
            cg = Path('/sys/fs/cgroup') / after['ControlGroup'].lstrip('/')
            record['remaining_cgroup_pids'] = []
            for processes in cg.rglob('cgroup.procs') if cg.exists() else []:
                record['remaining_cgroup_pids'].extend(processes.read_text().split())
            record['clean'] &= not record['remaining_cgroup_pids']
    except Exception as error:
        record['clean'] = False
        record['confirm_error'] = type(error).__name__
    return record


def verify_isolation(receipt, actual_boot_hash):
    assert receipt['postcheck']['boot_id_sha256'] == actual_boot_hash
    assert receipt['management_loopback_listener_verified'] is True
    assert receipt['guest_probe_exited'] is True and receipt['ready_file_removed'] is True
    assert receipt['postcheck']['host_shared_fs_types'] == []
    assert receipt['postcheck']['guest_probe_listeners_left'] == 0
    assert receipt['observations']
    for row in receipt['observations']:
        assert len(row['checks']) == 2
        assert all(item['host_connect_errno'] == 61 and item['host_listener'] is False
                   for item in row['checks'])


def replay_only(args, inputs, boot_hash):
    args.output.mkdir(mode=0o700, exist_ok=False)
    os.umask(0o077)
    ledger, panel = Ledger(), None
    unit = 'apsp-p0-replay-agent-' + uuid.uuid4().hex[:12] + '.service'
    result = dict(source_commit=SOURCE, agent_sha256=inputs['agent_sha256'],
                  boot_id_sha256=boot_hash, started_at=time.time(), success=False,
                  scope='real monitor-only Agent restart and pre-ACK timestamp replay only')
    cleanup_errors = []
    try:
        panel = Panel(ledger)
        private = args.output / 'agent-state'; private.mkdir(mode=0o700)
        config = SMOKE['configure'](private, panel.origin)
        config.write_text(config.read_text().replace('allow_remote_commands = true', 'allow_remote_commands = false'))
        command([str(args.agent), '--config', str(config), 'enroll', '--panel', panel.origin, '--token', 'smoke-enrollment'])
        command(['systemd-run', '--unit=' + unit, '--no-block', '--property=Type=exec',
                 '--property=MemoryMax=256M', '--property=MemorySwapMax=0', '--property=TasksMax=128',
                 '--property=CPUWeight=1000', '--property=OOMScoreAdjust=-500', '--property=KillMode=control-group',
                 '--property=RuntimeMaxSec=90s', '--', str(args.agent), '--config', str(config), 'run', '--monitor-only'])
        await_condition(lambda: (value := agent_status(args.agent, config)) and value['connected'], 20, 'replay Agent connection')
        await_condition(lambda: len(panel.samples) >= 3, 10, 'initial real telemetry')
        result['before_restart'] = properties(unit)
        panel.acknowledge = False
        ledger.change('agent_restart')
        ids = await_condition(lambda: list(panel.seen - panel.samples.keys()) if len(panel.seen - panel.samples.keys()) >= 3 else None,
                              10, 'three actual 503 telemetry samples')
        original = {identifier: verify_sampling_attempts(ledger)[identifier] for identifier in ids}
        assert all(any(row['id'] == identifier and row['acknowledged'] is False for row in ledger.attempts) for identifier in ids)
        command(['systemctl', 'restart', unit])
        await_condition(lambda: properties(unit)['MainPID'] != result['before_restart']['MainPID']
                        and (value := agent_status(args.agent, config)) and value['connected'], 20, 'replay Agent restarted')
        panel.acknowledge = True
        await_condition(lambda: set(ids).issubset(panel.samples), 20, 'old IDs ACKed after restart')
        replayed = {identifier: panel.samples[identifier]['sampled_at'] for identifier in ids}
        assert original == replayed, 'real restart changed original sampling timestamps'
        verify_sampling_attempts(ledger)
        result['original_unacked_sampling_times_ms'] = original
        result['replayed_sampling_times_ms'] = replayed
        result['replayed_sampling_ids'] = len(ids)
        result['after_restart'] = properties(unit)
        result['success'] = True
    except Exception as error:
        result['failure_type'], result['failure_reason'] = type(error).__name__, str(error)[:1000]
        raise
    finally:
        original_error = sys.exc_info()[0] is not None
        result['cleanup'] = cleanup_unit(unit)
        if panel:
            for close in [panel.disconnect, panel.shutdown, panel.server_close]:
                try: close()
                except Exception as error: cleanup_errors.append(type(error).__name__)
        result['cleanup_errors'] = cleanup_errors
        result['success'] &= result['cleanup']['clean'] and not cleanup_errors
        result['finished_at'] = time.time()
        result['preack_attempts'], result['sampling_receipts_detail'], result['events'] = ledger.attempts, ledger.samples, ledger.events
        (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps({key: result.get(key) for key in ['success', 'replayed_sampling_ids', 'failure_reason']}), flush=True)
        if not original_error and not result['success']:
            raise AssertionError('Replay-only result or cleanup failed')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for flag in ['agent', 'core-tests', 'inputs', 'runtime', 'runtime-receipt', 'isolation-receipt', 'output']:
        parser.add_argument('--' + flag, type=Path, required=True)
    parser.add_argument('--replay-only', action='store_true', help='Only the real Agent restart/pre-ACK timestamp proof; no runtime or core fixture execution')
    args = parser.parse_args()
    if sys.flags.optimize:
        raise ValueError('assertion checks must remain enabled')
    assert os.geteuid() == 0 and sys.platform == 'linux'
    assert socket.gethostname() == 'lima-apsp-p0-debian12', 'dedicated guest required'
    assert os.environ.get('GITHUB_ACTIONS') != 'true', 'CI remains paused'
    assert Path('/sys/fs/cgroup/cgroup.controllers').is_file()
    assert os.statvfs('/').f_bavail * os.statvfs('/').f_frsize >= 3 * 1024**3
    inputs = json.loads(args.inputs.read_text())
    assert inputs['source_commit'] == SOURCE and inputs['source_files_unchanged'] is True
    assert digest(args.agent) == inputs['agent_sha256']
    assert digest(args.core_tests) == inputs['core_test_sha256']
    runtime = json.loads(args.runtime_receipt.read_text())
    assert all(runtime[key] is True for key in ['trusted_signature_verified', 'archive_identity_verified', 'binary_identity_verified'])
    assert runtime['elf_machine'] == 183 and digest(args.runtime) == runtime['binary_sha256']
    isolation = json.loads(args.isolation_receipt.read_text())
    boot_hash = hashlib.sha256(Path('/proc/sys/kernel/random/boot_id').read_bytes()).hexdigest()
    verify_isolation(isolation, boot_hash)
    assert not diagnostic_units(), 'another diagnostic owns the guest'
    if args.replay_only:
        return replay_only(args, inputs, boot_hash)
    listing = command([str(args.core_tests), 'real_systemd_diagnostic_', '--list'])
    assert len([line for line in listing.splitlines() if line.endswith(': test')]) == 6
    args.output.mkdir(mode=0o700, exist_ok=False)
    os.umask(0o077)
    ledger, panel = Ledger(), None
    units, observed, snapshots = [], set(), []
    stop = threading.Event()
    traffic_thread = observer = None
    run_id = uuid.uuid4().hex[:12]
    public = Path(tempfile.mkdtemp(prefix='apsp-p0-echo-', dir='/var/tmp'))
    os.chmod(public, 0o755)
    result = dict(source_commit=SOURCE, agent_sha256=inputs['agent_sha256'],
                  core_test_sha256=inputs['core_test_sha256'], runtime_receipt=runtime,
                  boot_id_sha256=boot_hash, vm_instance_config_sha256=isolation['actual_instance_config_sha256'],
                  scope='real monitor-only Agent, substitute panel, loopback VLESS, independent core systemd fixtures',
                  heartbeat_limit_seconds=HEARTBEAT_LIMIT, request_period_seconds=PERIOD,
                  started_at=time.time(), success=False)
    kernel_cursor = None
    try:
        kernel_records = [json.loads(line) for line in command(['journalctl', '-k', '-n', '1', '-o', 'json', '--no-pager']).splitlines() if line]
        kernel_cursor = kernel_records[-1]['__CURSOR'] if kernel_records else None
        panel = Panel(ledger)
        private = args.output / 'agent-state'
        private.mkdir(mode=0o700)
        config = SMOKE['configure'](private, panel.origin)
        config.write_text(config.read_text().replace('allow_remote_commands = true', 'allow_remote_commands = false'))
        command([str(args.agent), '--config', str(config), 'enroll', '--panel', panel.origin, '--token', 'smoke-enrollment'])
        runtime_binary = public / 'sing-box'
        shutil.copyfile(args.runtime, runtime_binary)
        runtime_binary.chmod(0o555)
        assert digest(runtime_binary) == runtime['binary_sha256']
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            port = listener.getsockname()[1]
        native = dict(log=dict(level='error'), inbounds=[dict(type='vless', tag='fixture',
                     listen='127.0.0.1', listen_port=port, users=[dict(uuid=str(uuid.UUID(int=1)))])],
                     outbounds=[dict(type='direct', tag='direct')],
                     route=dict(rules=[dict(ip_cidr=['127.0.0.0/8', '::1/128'], outbound='direct'), dict(action='reject')], final='direct'))
        runtime_config = public / 'config.json'
        runtime_config.write_text(json.dumps(native)); runtime_config.chmod(0o644)
        data = public / 'data'; data.mkdir(mode=0o700)
        account = pwd.getpwnam('nobody'); os.chown(data, account.pw_uid, account.pw_gid)
        runtime_unit, agent_unit = ('apsp-p0-joint-' + role + '-' + run_id + '.service' for role in ['runtime', 'agent'])
        for unit, program, memory, extra in [
            (runtime_unit, [str(runtime_binary), 'run', '-c', str(runtime_config), '-D', str(data)], '192M',
             ['--property=User=nobody', '--property=NoNewPrivileges=yes', '--property=CapabilityBoundingSet=']),
            (agent_unit, [str(args.agent), '--config', str(config), 'run', '--monitor-only'], '256M', [])]:
            units.append(unit)
            command(['systemd-run', '--unit=' + unit, '--no-block', '--property=Type=exec',
                     '--property=MemoryMax=' + memory, '--property=MemorySwapMax=0', '--property=TasksMax=128',
                     '--property=CPUWeight=1000', '--property=OOMScoreAdjust=-500', '--property=KillMode=control-group',
                     '--property=RuntimeMaxSec=240s', *extra, '--', *program])
        await_condition(lambda: (value := agent_status(args.agent, config)) and value['connected'], 25, 'Agent connection')
        await_condition(lambda: transfer_guard(port)['ok'], 10, 'loopback runtime')
        initial = {unit: properties(unit) for unit in units}
        result['initial_services'] = initial
        for unit in units:
            assert initial[unit]['MainPID'] != '0' and initial[unit]['NRestarts'] == '0'
            assert initial[unit]['CPUWeight'] == '1000' and initial[unit]['OOMScoreAdjust'] == '-500'
        def observe():
            while not stop.is_set():
                try:
                    for unit in diagnostic_units():
                        observed.add(unit)
                        if len(snapshots) < 256:
                            values = properties(unit)
                            if values.get('ControlGroup'):
                                cg = Path('/sys/fs/cgroup') / values['ControlGroup'].lstrip('/')
                                for name in ['cgroup.procs', 'memory.events', 'pids.events']:
                                    try: values[name] = (cg / name).read_text().strip()
                                    except FileNotFoundError: pass
                            snapshots.append(dict(monotonic=time.monotonic(), unit=unit, properties=values))
                except Exception as error:
                    ledger.record(ledger.events, observer_error=type(error).__name__)
                stop.wait(0.1)
        traffic_thread = threading.Thread(target=traffic_worker, args=(ledger, stop, port), daemon=True); traffic_thread.start()
        observer = threading.Thread(target=observe, daemon=True); observer.start()
        ledger.change('baseline')
        await_condition(lambda: len([b for b in ledger.heartbeats if b['phase'] == 'baseline']) >= 3, 70, 'three actual baseline heartbeats')
        last = ledger.heartbeats[-1]['monotonic']
        await_condition(lambda: time.monotonic() - last >= 18.5, 20, 'next heartbeat overlap window')
        ledger.change('diagnostic_fixtures')
        with (args.output / 'core-systemd.log').open('w') as log:
            completed = subprocess.run([str(args.core_tests), 'real_systemd_diagnostic_', '--ignored',
                                        '--test-threads=1', '--nocapture'], stdout=log, stderr=subprocess.STDOUT, timeout=120)
        assert completed.returncode == 0, 'real systemd fixture failure'
        assert '6 passed; 0 failed; 0 ignored' in (args.output / 'core-systemd.log').read_text()
        assert not diagnostic_units(), 'diagnostic unit survived fixture cleanup'
        result['after_fixtures_services'] = {unit: properties(unit) for unit in units}
        for unit in units:
            assert properties(unit)['MainPID'] == initial[unit]['MainPID']
            assert properties(unit)['NRestarts'] == '0'
        ledger.change('agent_restart')
        panel.acknowledge = False
        unacked = await_condition(lambda: list(panel.seen - panel.samples.keys()) if len(panel.seen - panel.samples.keys()) >= 3 else None, 10, 'telemetry backlog')
        command(['systemctl', 'restart', agent_unit])
        await_condition(lambda: properties(agent_unit)['MainPID'] != initial[agent_unit]['MainPID'] and (value := agent_status(args.agent, config)) and value['connected'], 25, 'restarted Agent')
        panel.acknowledge = True
        await_condition(lambda: set(unacked).issubset(panel.samples), 25, 'original outbox ids replayed')
        result['replayed_sampling_ids'] = len(unacked)
        result['after_restart_services'] = {unit: properties(unit) for unit in units}
        ledger.change('panel_disconnect')
        panel.disconnect()
        outage_start = time.monotonic()
        time.sleep(10)
        panel.accepting = True
        await_condition(lambda: any(b['monotonic'] >= outage_start + 10 for b in ledger.heartbeats), 25, 'actual heartbeat after fixture outage')
        ledger.change('recovery')
        recovered_count = len(ledger.heartbeats)
        await_condition(lambda: len(ledger.heartbeats) > recovered_count, 25, 'next recovered heartbeat')
        result['final_services'] = {unit: properties(unit) for unit in units}
        assert all(result['final_services'][unit]['ActiveState'] == 'active' and result['final_services'][unit]['MainPID'] != '0' for unit in units)
        assert properties(runtime_unit)['MainPID'] == initial[runtime_unit]['MainPID']
        assert properties(runtime_unit)['NRestarts'] == '0'
        result.update(evaluate(ledger))
        verify_sampling_attempts(ledger)
        result['heartbeat_receipts_during_fixtures'] = len([b for b in ledger.heartbeats if b['phase'] == 'diagnostic_fixtures'])
        assert result['heartbeat_receipts_during_fixtures'] >= 1
        result['success'] = True
    except Exception as error:
        result['failure_type'] = type(error).__name__
        result['failure_reason'] = str(error)[:1000]
        raise
    finally:
        original_error = sys.exc_info()[0] is not None
        result['worker_threads_live_before_stop'] = all(thread is not None and thread.is_alive()
                                                       for thread in [traffic_thread, observer])
        stop.set()
        cleanup_errors = []
        for thread in [traffic_thread, observer]:
            if thread:
                try: thread.join(timeout=5)
                except Exception as error: cleanup_errors.append(type(error).__name__)
        result['worker_threads_stopped'] = all(thread is None or not thread.is_alive() for thread in [traffic_thread, observer])
        if result['success'] and result['worker_threads_stopped']:
            try:
                # Include a transfer that completed while the workers were stopping.
                result.update(evaluate(ledger))
                verify_sampling_attempts(ledger)
            except Exception as error:
                result['success'] = False
                result['failure_type'] = type(error).__name__
                result['failure_reason'] = str(error)[:1000]
        cleanup = [cleanup_unit(unit) for unit in units + sorted(observed)]
        if panel:
            for close in [panel.disconnect, panel.shutdown, panel.server_close]:
                try: close()
                except Exception as error: cleanup_errors.append(type(error).__name__)
        if all(row['clean'] for row in cleanup if row['unit'] in units):
            try: shutil.rmtree(public)
            except Exception as error: cleanup_errors.append(type(error).__name__)
        else:
            cleanup_errors.append('ResidentCleanupUnconfirmed')
        result['cleanup'] = cleanup
        result['cleanup_errors'] = cleanup_errors
        result['runtime_fixture_directory_removed'] = not public.exists()
        try: result['remaining_diagnostic_units'] = diagnostic_units()
        except Exception as error:
            result['remaining_diagnostic_units'] = None; cleanup_errors.append(type(error).__name__)
        try:
            mounts = command(['findmnt', '--raw', '--noheadings', '--output', 'TARGET'])
            result['remaining_fixture_mounts'] = [x for x in mounts.splitlines() if 'apsp-cancel-test-' in x or 'apsp-p0-echo-' in x]
        except Exception as error:
            result['remaining_fixture_mounts'] = None; cleanup_errors.append(type(error).__name__)
        fixture_pids = set()
        for observation in snapshots:
            values = observation['properties']
            for field in ['MainPID', 'ControlPID', 'cgroup.procs']:
                fixture_pids.update(int(value) for value in values.get(field, '').split() if value.isdigit() and int(value) > 0)
        result['remaining_observed_fixture_pids'] = sorted(pid for pid in fixture_pids if Path('/proc/' + str(pid)).exists())
        result['observer_errors'] = [row['observer_error'] for row in ledger.events if 'observer_error' in row]
        kernel_command = ['journalctl', '-k', '-o', 'short-monotonic', '--no-pager']
        if kernel_cursor: kernel_command.append('--after-cursor=' + kernel_cursor)
        try: kernel = command(kernel_command)
        except Exception as error:
            kernel = ''; cleanup_errors.append(type(error).__name__)
        (args.output / 'kernel-difference.log').write_text(kernel)
        oom = classify_oom(kernel)
        result['kernel_oom_lines'], result['oom_classification'], result['only_diagnostic_memcg_oom'] = oom['lines'], oom['status'], oom['allowed']
        result['cleanup_passed'] = (not cleanup_errors and not result['observer_errors'] and result['worker_threads_live_before_stop'] and result['worker_threads_stopped'] and all(item['clean'] for item in cleanup)
                                    and result['remaining_diagnostic_units'] == [] and result['remaining_fixture_mounts'] == []
                                    and not result['remaining_observed_fixture_pids'] and result['runtime_fixture_directory_removed'])
        result['success'] = result['success'] and result['cleanup_passed'] and result['only_diagnostic_memcg_oom']
        result['finished_at'] = time.time()
        result['heartbeats'], result['sampling_receipts_detail'], result['traffic'], result['events'] = ledger.heartbeats, ledger.samples, ledger.traffic, ledger.events
        result['preack_attempts'] = ledger.attempts
        result['diagnostic_observations'] = snapshots
        (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps({k: result.get(k) for k in ['success', 'heartbeat_count', 'proxy_echo_count', 'failure_reason']}), flush=True)
        if not original_error and not result['success']:
            raise AssertionError('Joint result failed, including cleanup or kernel OOM verification')


if __name__ == '__main__':
    if sys.flags.optimize:
        raise SystemExit('assertion checks must remain enabled')
    if len(sys.argv) == 3 and sys.argv[1] == '--echo-worker':
        port = int(sys.argv[2]); assert 1024 < port < 65536
        NATIVE['transfer'](port)
    else:
        main()
