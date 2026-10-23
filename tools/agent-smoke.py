#!/usr/bin/env python3
"""Exercise a real native Agent against a loopback panel fixture without host services."""
import argparse
import base64
from contextlib import closing
import gzip
import hashlib
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import shutil
import runpy
import socket
import sqlite3
import struct
import subprocess
import sys
import tempfile
import threading
import time
import uuid

SESSION = 'smoke-session'
RELEASE = runpy.run_path(str(Path(__file__).with_name('ci-release-fixture.py')))


def wait_for(check, description, seconds=120 if os.name == 'nt' else 60):
    started = time.monotonic()
    deadline = started + seconds
    while time.monotonic() < deadline:
        result = check()
        if result:
            print(f'Passed: {description} ({time.monotonic() - started:.1f}s)', flush=True)
            return result
        time.sleep(0.5)
    raise AssertionError('Timed out waiting for ' + description)


class Panel(ThreadingHTTPServer):
    daemon_threads = True

    def handle_error(self, request, client_address):
        # Stopping an Agent deliberately closes its pooled HTTP connections.
        if not isinstance(sys.exception(), (ConnectionResetError, BrokenPipeError)):
            super().handle_error(request, client_address)

    def __init__(self, port=0):
        super().__init__(('127.0.0.1', port), Handler)
        self.acknowledge = True
        self.samples = {}
        self.seen = set()
        self.messages = []
        self.manifest = dict(rev=0, modules={})
        self.downloads = {}
        self.command_results = []
        self.probe_results = []
        self.requests = []
        self.command = dict(id=str(uuid.uuid4()), command='echo apsp-command-fixture',
                            timeout_secs=60 if os.name == 'nt' else 5, expires_at=int(time.time()) + 600)
        self.probe = dict(id=str(uuid.uuid4()), name='loopback fixture', kind='tcp',
                          target='127.0.0.1', port=self.server_port, interval_secs=10,
                          carrier='', enabled=True)
        self.icmp_probe = dict(self.probe, id=str(uuid.uuid4()), name='ICMP fixture', kind='icmp', port=None)
        for spec in (self.probe, self.icmp_probe):
            spec['monitor'] = dict(region='', address_family='any', authorization=dict(
                kind='owned', source='TEST_ONLY-this-process-loopback', scope='Four bounded probes per ten seconds',
                enabled=True, expires_at=int(time.time()) + 3600, identity=dict(
                    kind=spec['kind'], target=spec['target'], port=spec['port'], address_family='any')))
        threading.Thread(target=self.serve_forever, daemon=True).start()

    @property
    def origin(self):
        return f'http://127.0.0.1:{self.server_port}'


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *_):
        pass

    def reply(self, value, status=200):
        body = json.dumps(value).encode()
        self.send_response(status)
        self.send_header('Content-Type', 'application/json')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def authorized(self):
        if self.headers.get('Authorization') != 'Bearer ' + SESSION:
            self.reply({'error': 'unauthorized'}, 401)
            return False
        return True

    def do_GET(self):
        self.server.requests.append(self.path)
        if self.path == '/api/agent/v1/ws':
            return self.websocket()
        if not self.authorized():
            return
        if self.path in self.server.downloads:
            data = self.server.downloads[self.path]
            self.send_response(200)
            self.send_header('Content-Length', str(len(data)))
            self.end_headers()
            self.wfile.write(data)
            return
        suffix = self.path.partition('?')[0].removeprefix('/api/agent/v1/')
        values = {
            'settings': dict(sample_interval_secs=1, upload_interval_secs=1,
                             auto_update=False, discover_public_ips=False),
            'manifest': self.server.manifest, 'diagnostics': [],
            'commands': [self.server.command], 'probes': [self.server.probe, self.server.icmp_probe], 'update': None,
        }
        self.reply(values.get(suffix, []))

    def do_POST(self):
        self.server.requests.append(self.path)
        body = self.rfile.read(int(self.headers.get('Content-Length', 0)))
        if self.headers.get('Content-Encoding') == 'gzip':
            body = gzip.decompress(body)
        value = json.loads(body)
        if self.path == '/api/agent/v1/enroll':
            assert value['token'] == 'smoke-enrollment'
            return self.reply({'server_id': 1})
        if not self.authorized():
            return
        if self.path.endswith('/telemetry'):
            assert self.headers.get('Content-Encoding') == 'gzip'
            samples = value['samples']
            self.server.seen.update(v['id'] for v in samples)
            if not self.server.acknowledge:
                return self.reply({'error': 'fixture outage'}, 503)
            self.server.samples.update((v['id'], v) for v in samples)
            return self.reply({'ids': [v['id'] for v in samples]})
        if '/commands/' in self.path:
            self.server.command_results.append(value)
            return self.reply({'ids': [value['id']]})
        if self.path.endswith('/probe-results'):
            self.server.probe_results.extend(value['results'])
            return self.reply({'ids': [v['id'] for v in value['results']]})
        self.reply({}, 404)

    def frame(self, payload, opcode=1):
        encoded = json.dumps(payload).encode() if opcode == 1 else payload
        size = len(encoded)
        length = bytes([size]) if size < 126 else b'\x7e' + struct.pack('!H', size)
        self.wfile.write(bytes([0x80 | opcode]) + length + encoded)
        self.wfile.flush()

    def message(self, kind, payload):
        self.frame(dict(v=1, type=kind, id=str(uuid.uuid4()), ts=int(time.time()), payload=payload))

    def websocket(self):
        accept = base64.b64encode(hashlib.sha1((self.headers['Sec-WebSocket-Key'] +
                    '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').encode()).digest()).decode()
        self.send_response(101)
        self.send_header('Upgrade', 'websocket')
        self.send_header('Connection', 'Upgrade')
        self.send_header('Sec-WebSocket-Accept', accept)
        self.end_headers()
        self.message('auth.challenge', dict(nonce='smoke-challenge', server_time=int(time.time())))
        try:
            while True:
                header = self.rfile.read(2)
                if len(header) != 2:
                    return
                opcode, size = header[0] & 15, header[1] & 127
                if size == 126:
                    size = struct.unpack('!H', self.rfile.read(2))[0]
                elif size == 127:
                    size = struct.unpack('!Q', self.rfile.read(8))[0]
                assert size <= 1024 * 1024
                mask = self.rfile.read(4) if header[1] & 128 else None
                payload = self.rfile.read(size)
                if mask:
                    payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
                if opcode == 8:
                    return
                if opcode == 9:
                    self.frame(payload, 10)
                if opcode != 1:
                    continue
                message = json.loads(payload)
                self.server.messages.append(message)
                if message['type'] == 'auth.response':
                    self.message('hello.ack', dict(server_time=int(time.time()), session_token=SESSION,
                                                  session_expires_at=int(time.time()) + 3600))
                else:
                    self.frame(b'fixture', 9)
        except (OSError, ValueError):
            return


def invoke(binary, config, *args, **kwargs):
    result = subprocess.run([str(binary), '--config', str(config), *args],
                            capture_output=True, text=True, encoding='utf-8',
                            timeout=180 if {'enroll', 'install-service'}.intersection(args) else 60, **kwargs)
    if result.returncode:
        raise subprocess.CalledProcessError(result.returncode, result.args, result.stdout, result.stderr)
    return result.stdout


def stop(process):
    if process.poll() is None:
        if os.name == 'nt':
            subprocess.run(['taskkill', '/PID', str(process.pid), '/T', '/F'], capture_output=True)
        else:
            process.terminate()
        try:
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=5)


def configure(root, origin):
    values = dict(panel_url=origin, identity_dir=str(root / 'identity'), state_db=str(root / 'state.db'),
                  runtime_root=str(root / 'runtime'), install_root=str(root / 'plugins'),
                  agent_root=str(root / 'core'), status_socket=str(root / 'status.sock'), operation_timeout_secs=5,
                  allow_remote_commands=True)
    config = root / 'agent.toml'
    config.write_text(''.join(f'{key} = {json.dumps(value)}\n' for key, value in values.items()) +
                      '[settings]\nsample_interval_secs=1\nupload_interval_secs=1\ndiscover_public_ips=false\n', encoding='utf-8')
    return config


def status(binary, config):
    try:
        return json.loads(invoke(binary, config, 'status'))
    except subprocess.CalledProcessError:
        return None


def reference(path, target):
    if os.name == 'nt':
        path.write_text(json.dumps(dict(apsp_directory_reference=True, target=str(target))), encoding='utf-8')
    else:
        path.symlink_to(target)


def write_json(path, value):
    temporary = path.with_name(path.name + '.tmp')
    temporary.write_text(json.dumps(value), encoding='utf-8')
    temporary.replace(path)


def read_shared_json(path):
    deadline = time.monotonic() + 2
    while True:
        try:
            return json.loads(path.read_text(encoding='utf-8'))
        except PermissionError:
            # Windows may briefly reject opening a file being atomically replaced.
            if sys.platform != 'win32' or time.monotonic() >= deadline:
                raise
            time.sleep(0.05)


def windows_permissions(binary, config, root, origin):
    if os.name != 'nt':
        return {}
    def quote(value):
        return "'" + str(value).replace("'", "''") + "'"
    timings = {}
    for name, probe in (
        ('PowerShell startup', '$null=1'),
        ('direct ACL read', f'$null=[IO.File]::GetAccessControl({quote(root / "identity/device.key")})'),
        ('cmdlet ACL read', f'$null=Get-Acl -LiteralPath {quote(root / "identity/device.key")}'),
    ):
        started = time.monotonic()
        subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', probe],
                       capture_output=True, check=True, timeout=60)
        timings[name] = round(time.monotonic() - started, 2)
    child = subprocess.Popen(['cmd.exe', '/c', 'exit', '0'], stdout=subprocess.DEVNULL)
    child.wait(timeout=10)
    started = time.monotonic()
    subprocess.run(['taskkill.exe', '/PID', str(child.pid), '/T', '/F'], capture_output=True, timeout=60)
    timings['completed process cleanup'] = round(time.monotonic() - started, 2)
    print('Windows command timings:', timings, flush=True)
    script = f"""$ErrorActionPreference='Stop'
$path={quote(root / 'identity/device.key')}
$acl=[IO.File]::GetAccessControl($path)
$saved=$acl.GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::All)
$users=[Security.Principal.SecurityIdentifier]'S-1-5-32-545'
$rule=[Security.AccessControl.FileSystemAccessRule]::new($users,[Security.AccessControl.FileSystemRights]::Read,[Security.AccessControl.AccessControlType]::Allow)
try {{
    $acl.AddAccessRule($rule)
    [IO.File]::SetAccessControl($path,$acl)
    $ErrorActionPreference='Continue'
    $result=& {quote(binary)} --config {quote(config)} enroll --panel {quote(origin)} --token smoke-enrollment 2>&1
    $code=$LASTEXITCODE
    $ErrorActionPreference='Stop'
    if($code -eq 0 -or ($result -join "`n") -notmatch 'not private') {{ throw "Insecure identity was not rejected: $result" }}
}} finally {{
    $restore=[Security.AccessControl.FileSecurity]::new()
    $restore.SetSecurityDescriptorSddlForm($saved)
    [IO.File]::SetAccessControl($path,$restore)
}}
"""
    result = subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', script],
                            capture_output=True, text=True, encoding='utf-8', errors='replace', timeout=120)
    assert result.returncode == 0, result.stdout + result.stderr
    print('Passed: Windows rejects an identity readable by ordinary Users', flush=True)
    return timings


def supervise_smoke(binary, config, root, log):
    core = root / 'core'
    core.mkdir(exist_ok=True)
    version = subprocess.check_output([str(binary), '--version'], text=True).strip().split()[-1]
    # Reuse this build as a previous-install fixture; verify real process replacement and readiness.
    previous = '0.0.1'
    for name in (previous, version):
        (core / name).mkdir()
        shutil.copy2(binary, core / name / binary.name)
        RELEASE['install'](core / name, RELEASE['proof']('agent', name, binary.name, binary.read_bytes()))
    reference(core / 'current', core / previous)
    process = subprocess.Popen([str(binary), '--config', str(config), 'supervise', '--monitor-only'], stdout=log, stderr=log)
    try:
        first = wait_for(lambda: status(binary, config), 'supervised Agent')
        identity = (root / 'identity/device.key').read_bytes()
        pending = dict(version=version, sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                       proof=RELEASE['proof']('agent', version, binary.name, binary.read_bytes()))
        write_json(core / 'pending-update.json', pending)
        def state():
            return read_shared_json(core / 'update-state.json')
        def consumed():
            return read_shared_json(core / 'pending-update.json') is None
        wait_for(lambda: state()['current'] == version and state()['trial'] is None and consumed(), 'successful Agent activation', 180 if os.name == 'nt' else 90)
        upgraded = wait_for(lambda: status(binary, config), 'upgraded status')
        assert upgraded['pid'] != first['pid']
        # A corrupt candidate must be rejected before stopping the running Agent.
        (core / '99.0.1').mkdir()
        shutil.copy2(binary, core / '99.0.1' / binary.name)
        write_json(core / 'pending-update.json', dict(version='99.0.1', sha256='0' * 64,
             proof=RELEASE['proof']('agent', '99.0.1', binary.name, binary.read_bytes())))
        wait_for(lambda: '99.0.1' in state()['failed_versions'] and consumed(), 'corrupt candidate rejection')
        assert status(binary, config)['pid'] == upgraded['pid']
        def failed_candidate(candidate_version, cache_exit):
            failed = core / candidate_version
            failed.mkdir()
            fixture = failed / binary.name
            if os.name == 'nt':
                source = failed / 'fixture.rs'
                source.write_text('#![forbid(unsafe_code)]\nfn main(){let args:Vec<_>=std::env::args().skip(1).collect();'
                    f'if args==["--version"]{{println!("apsp-agent {candidate_version}")}}'
                    f'else if args.len()==3 && args[0]=="--config" && args[2]=="verify-cache"{{std::process::exit({cache_exit})}}'
                    'else{std::process::exit(1)}}')
                subprocess.run(['rustc', '--edition=2024', str(source), '-o', str(fixture)], check=True)
            else:
                fixture.write_text('#!/bin/sh\n'
                    f'if [ "$1" = --version ]; then echo "apsp-agent {candidate_version}"; '
                    f'elif [ "$#" = 3 ] && [ "$1" = --config ] && [ "$3" = verify-cache ]; then exit {cache_exit}; '
                    'else exit 1; fi\n')
                fixture.chmod(0o755)
            proof = RELEASE['proof']('agent', candidate_version, binary.name, fixture.read_bytes())
            RELEASE['install'](failed, proof)
            return dict(version=candidate_version, sha256=hashlib.sha256(fixture.read_bytes()).hexdigest(), proof=proof)

        # The candidate's roots may reject the current cache after a key rotation.
        # Reject that update before touching the old process, identity or usage ledger.
        with closing(sqlite3.connect(root / 'state.db')) as db:
            db.execute('INSERT INTO usage_baselines VALUES(?,?,?,?,?,?)',
                       ('preflight-fixture', 'u1_n1', str(uuid.uuid4()), '123', '456', int(time.time())))
            db.commit()
        def usage_snapshot():
            with closing(sqlite3.connect(root / 'state.db')) as db:
                return (db.execute('SELECT * FROM usage_baselines ORDER BY module,stat_name').fetchall(),
                        db.execute('SELECT * FROM usage_outbox ORDER BY epoch,seq').fetchall())
        before_usage = usage_snapshot()
        before_config = config.read_bytes()
        before_reference = (core / 'current').read_bytes() if os.name == 'nt' else os.readlink(core / 'current')
        write_json(core / 'pending-update.json', failed_candidate('99.0.2', 1))
        wait_for(lambda: '99.0.2' in state()['failed_versions'] and consumed(), 'candidate cache preflight rejection')
        assert state()['trial'] is None and state()['current'] == version
        assert 'cache signature preflight' in state()['last_error']
        assert status(binary, config)['pid'] == upgraded['pid'], 'cache rejection stopped the healthy Agent'
        assert config.read_bytes() == before_config and (root / 'identity/device.key').read_bytes() == identity
        assert usage_snapshot() == before_usage, 'cache rejection changed the usage ledger'
        assert ((core / 'current').read_bytes() if os.name == 'nt' else os.readlink(core / 'current')) == before_reference
        # Remove only this synthetic ledger row before the later monitor-only
        # restart scenarios, which intentionally reject managed usage history.
        with closing(sqlite3.connect(root / 'state.db')) as db:
            db.execute("DELETE FROM usage_baselines WHERE module='preflight-fixture' AND stat_name='u1_n1'")
            db.commit()

        pending = failed_candidate('99.0.0', 0)
        write_json(core / 'pending-update.json', pending)
        wait_for(lambda: '99.0.0' in state()['failed_versions'] and state()['trial'] is None and consumed(), 'failed-start rollback', 90)
        restored = wait_for(lambda: status(binary, config), 'restored Agent')
        assert restored['agent_version'] == version and restored['pid'] != upgraded['pid']
        assert (root / 'identity/device.key').read_bytes() == identity
        assert state()['current'] == version and state()['last_error']
        write_json(core / 'pending-update.json', pending)
        wait_for(consumed, 'failed version suppression')
        assert status(binary, config)['pid'] == restored['pid']
    finally:
        stop(process)
    wait_for(lambda: status(binary, config) is None, 'supervisor child cleanup')
    # Recreate an interrupted, unconfirmed update and ensure startup restores the previous release.
    state_file = core / 'update-state.json'
    interrupted = json.loads(state_file.read_text())
    interrupted.update(current='99.0.0', previous=version, trial=pending)
    write_json(state_file, interrupted)
    (core / 'current').unlink()
    reference(core / 'current', core / '99.0.0')
    process = subprocess.Popen([str(binary), '--config', str(config), 'supervise', '--monitor-only'], stdout=log, stderr=log)
    try:
        wait_for(lambda: status(binary, config), 'interrupted update recovery')
        restored = json.loads(state_file.read_text())
        assert restored['current'] == version and restored['trial'] is None
        assert '99.0.0' in restored['failed_versions']
    finally:
        stop(process)
    wait_for(lambda: status(binary, config) is None, 'recovered supervisor shutdown')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    os.environ['NO_PROXY'] = os.environ['no_proxy'] = '127.0.0.1,localhost'
    panel = Panel()
    # Darwin's default temporary path leaves too little room for Unix sockets.
    directory = tempfile.mkdtemp(prefix='sn-', dir=None if os.name == 'nt' else '/tmp')
    root = Path(directory).resolve()
    try:
        config = configure(root, panel.origin)
        print(invoke(binary, config, 'enroll', '--panel', panel.origin, '--token', 'smoke-enrollment'))
        windows_timings = windows_permissions(binary, config, root, panel.origin)
        with (root / 'agent.log').open('w', encoding='utf-8') as log:
            def start():
                return subprocess.Popen([str(binary), '--config', str(config), 'run', '--monitor-only'], stdout=log, stderr=log)
            def connected():
                assert process.poll() is None, f'Agent exited unexpectedly: {process.returncode}'
                info = status(binary, config)
                return info and info['pid'] == process.pid and info['connected']
            process = start()
            try:
                wait_for(connected, 'native Agent connection')
                wait_for(lambda: len(panel.samples) >= 3, 'compressed telemetry with ACK')
                wait_for(lambda: panel.command_results, 'remote command completion', 90)
                assert panel.command_results[0]['status'] == 'succeeded', panel.command_results[0]
                assert 'apsp-command-fixture' in panel.command_results[0]['stdout']
                for probe in (panel.probe, panel.icmp_probe):
                    result = wait_for(lambda: next((r for r in panel.probe_results if r['probe_id'] == probe['id']), None),
                                      'continuous ' + probe['kind'] + ' probe')
                    assert result['loss_percent'] == 0 and result['latency_ms'] is not None, result
                panel.acknowledge = False
                wait_for(lambda: len(panel.seen - panel.samples.keys()) >= 3, 'offline telemetry spool')
                unacked = panel.seen - panel.samples.keys()
                stop(process)
                process = start()
                wait_for(connected, 'restarted Agent connection', 120)
                panel.acknowledge = True
                wait_for(lambda: unacked.issubset(panel.samples), 'persisted telemetry replay after restart', 120)
                assert len(panel.command_results) == 1, 'command executed twice after restart'
                sample = list(panel.samples.values())[-1]['metrics']
                assert sample['processes'] > 0 and sample['memory_used'] > 0
                static = next(m['payload'] for m in panel.messages if m['type'] == 'telemetry.static')
                if expected_libc := os.environ.get('APSP_EXPECT_RUNTIME_LIBC'):
                    assert static.get('runtime_libc') == expected_libc, static.get('runtime_libc')
                if sys.platform.startswith('freebsd'):
                    # Repeated startup overlaps static inventories with one-second sampling.
                    for _ in range(5):
                        stop(process)
                        received = len(panel.samples)
                        process = start()
                        wait_for(connected, 'FreeBSD repeated startup')
                        wait_for(lambda: len(panel.samples) >= received + 2, 'FreeBSD concurrent inventories')
            finally:
                stop(process)
            supervise_smoke(binary, config, root, log)
        with closing(sqlite3.connect(root / 'state.db')) as db:
            assert db.execute('pragma user_version').fetchone()[0] == 1
        print('Native Agent: enrollment, telemetry/replay, command deduplication, TCP/ICMP probes, activation, rollback and shutdown passed')
    except BaseException as error:
        if isinstance(error, subprocess.CalledProcessError):
            print(error.stdout, error.stderr)
        if (root / 'agent.log').exists():
            print((root / 'agent.log').read_text(encoding='utf-8', errors='replace')[-24000:])
        if 'process' in locals():
            print('Agent exit code:', process.poll())
        if 'windows_timings' in locals() and windows_timings:
            print('Windows command timings:', windows_timings)
        core_directory = os.environ.get('APSP_SMOKE_CORE_DIR')
        if core_directory and shutil.which('lldb'):
            for core in Path(core_directory).glob('*.core'):
                try:
                    trace = subprocess.run(['lldb', '-b', '-c', str(core), str(binary), '-o', 'thread backtrace all'],
                                           capture_output=True, text=True, errors='replace', timeout=30)
                    print('Native crash backtrace:', trace.stdout[:16000], trace.stderr[:2000])
                except subprocess.TimeoutExpired:
                    print('Native crash backtrace timed out')
        print('Recent panel messages:', [m['type'] for m in panel.messages[-12:]])
        print('Recent panel requests:', panel.requests[-20:])
        print('Unacknowledged samples:', len(panel.seen - panel.samples.keys()))
        if (root / 'state.db').exists():
            try:
                with closing(sqlite3.connect(root / 'state.db')) as db:
                    print('Pending local telemetry:', db.execute('SELECT count(*) FROM telemetry_outbox').fetchone()[0])
            except sqlite3.Error as database_error:
                print('Cannot inspect telemetry state:', database_error)
        for name in ('update-state.json', 'pending-update.json'):
            state_file = root / 'core' / name
            if state_file.exists():
                print(name, state_file.read_text(encoding='utf-8', errors='replace'))
        raise
    finally:
        panel.shutdown()
        panel.server_close()
        shutil.rmtree(root)


if __name__ == '__main__':
    main()
