#!/usr/bin/env python3
"""TEST_ONLY Debian carrier monitoring with real Agent/Panel/PG and loopback traffic."""
import argparse
from contextlib import closing
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import re
import runpy
import secrets
import select
import shutil
import socket
import sqlite3
import subprocess
import sys
import threading
import time
import types
import uuid

RUNTIME_SHA = 'fee83ca8457c94449dd04aa17a51830cbc9b449a4dda290e995d3366188e0302'
PG_PORT, PANEL_PORT, GATE_PORT = 55682, 55782, 55882
MAX_BODY = 1024 * 1024


def require(value, reason):
    if not value:
        raise RuntimeError(reason)


def sha(path):
    with Path(path).open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def private_helpers(built_root, output):
    manifest = json.loads((built_root / 'harness-identity.json').read_text())
    require(re.fullmatch(r'apsp-plugin-install-20\d{6}-r[1-9]\d{0,2}', built_root.name), 'reviewed build namespace required')
    modules = {}
    for name in ('common', 'finish'):
        data = (built_root / (name + '.py')).read_bytes()
        require(hashlib.sha256(data).hexdigest() == manifest['script_sha256'][name + '.py'], 'reviewed helper digest differs')
        # Only relocate the exact reviewed private namespace; no installer, build,
        # template, account or production path is executed by this controller.
        relocated = data.decode().replace(built_root.name, output.name)
        module = types.ModuleType(name)
        module.__file__ = str(built_root / (name + '.py'))
        sys.modules[name] = module
        if name == 'common':
            exec(compile(relocated, module.__file__, 'exec'), module.__dict__)
            module.ROOT = output
            module.PUBLIC_ROOT = Path('/var/lib') / (output.name + '-runtime')
            module.PG = Path('/var/lib/postgresql') / output.name
            module.PG_PORT, module.PANEL_PORT = PG_PORT, PANEL_PORT
            module.ORIGIN = f'http://127.0.0.1:{GATE_PORT}'
            module.DATABASE = output.name.replace('-', '_')
            module.PREFIX = output.name + '-'
            module.RUNTIME_UNIT = module.unit('runtime')
        else:
            exec(compile(relocated, module.__file__, 'exec'), module.__dict__)
        modules[name] = module
    return modules['common'], modules['finish']


class Gate(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = False

    def __init__(self):
        super().__init__(('127.0.0.1', GATE_PORT), GateHandler)
        self.upload = True
        self.available = True
        self.lock = threading.RLock()
        self.connections = set()
        self.attempts = []
        self.acked = set()
        self.authorization = ''
        self.errors = []
        threading.Thread(target=self.serve_forever, daemon=True).start()

    def disconnect(self):
        self.available = False
        with self.lock:
            for connection in list(self.connections):
                try:
                    connection.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass

    def handle_error(self, request, client_address):
        if not isinstance(sys.exception(), (BrokenPipeError, ConnectionResetError, TimeoutError)):
            self.errors.append(type(sys.exception()).__name__)


class GateHandler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *_):
        pass

    def respond(self, status, data, headers=()):
        self.send_response(status)
        for key, value in headers:
            if key.lower() not in ('connection', 'transfer-encoding', 'content-length'):
                self.send_header(key, value)
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def websocket(self):
        backend = socket.create_connection(('127.0.0.1', PANEL_PORT), timeout=3)
        with backend:
            headers = ''.join(f'{key}: {value}\r\n' for key, value in self.headers.items())
            backend.sendall(f'GET {self.path} HTTP/1.1\r\n{headers}\r\n'.encode())
            received = bytearray()
            while b'\r\n\r\n' not in received:
                chunk = backend.recv(4096)
                require(chunk and len(received) < 65536, 'bounded websocket handshake required')
                received.extend(chunk)
            self.connection.sendall(received)
            with self.server.lock:
                self.server.connections.add(self.connection)
            backend.settimeout(None)
            try:
                while self.server.available:
                    ready, _, _ = select.select([backend, self.connection], [], [], .25)
                    for source in ready:
                        data = source.recv(65536)
                        if not data:
                            return
                        (self.connection if source is backend else backend).sendall(data)
            finally:
                with self.server.lock:
                    self.server.connections.discard(self.connection)
                self.close_connection = True

    def exchange(self):
        if not self.server.available:
            self.close_connection = True
            return self.respond(503, b'{"error":"TEST_ONLY deliberate disconnect"}')
        if self.path == '/api/agent/v1/ws':
            return self.websocket()
        size = int(self.headers.get('Content-Length', '0'))
        require(0 <= size <= MAX_BODY, 'bounded loopback request required')
        body = self.rfile.read(size)
        if self.path.startswith('/api/agent/') and self.headers.get('Authorization'):
            self.server.authorization = self.headers['Authorization']
        probe_results = None
        if self.path == '/api/agent/v1/probe-results':
            probe_results = json.loads(body)['results']
            require(0 < len(probe_results) <= 64, 'bounded real probe batch required')
            with self.server.lock:
                self.server.attempts.extend({'at': time.time(), 'result': item, 'forwarded': self.server.upload} for item in probe_results)
            if not self.server.upload:
                return self.respond(503, b'{"error":"TEST_ONLY retain real probe outbox"}')
        connection = http.client.HTTPConnection('127.0.0.1', PANEL_PORT, timeout=3)
        try:
            headers = {key: value for key, value in self.headers.items() if key.lower() != 'connection'}
            connection.request(self.command, self.path, body, headers)
            response = connection.getresponse()
            data = response.read(MAX_BODY + 1)
            require(len(data) <= MAX_BODY, 'bounded panel response required')
            if probe_results is not None and response.status == 200:
                with self.server.lock:
                    self.server.acked.update(json.loads(data)['ids'])
            self.respond(response.status, data, response.getheaders())
        finally:
            connection.close()

    do_GET = exchange
    do_POST = exchange
    do_PATCH = exchange
    do_DELETE = exchange


class SlowTargets:
    def __init__(self):
        self.sockets = []
        self.ports = []
        for _ in range(4):
            listener = socket.socket()
            listener.bind(('127.0.0.1', 0))
            listener.listen(1)
            self.sockets.append(listener)
            self.ports.append(listener.getsockname()[1])
            for _ in range(16):
                connection = socket.socket()
                connection.settimeout(.15)
                try:
                    connection.connect(listener.getsockname())
                    self.sockets.append(connection)
                except TimeoutError:
                    connection.close()
                    break
            else:
                raise RuntimeError('owned slow listener backlog did not saturate')

    def close(self):
        for connection in self.sockets:
            connection.close()


def active_connects(pid, ports):
    try:
        inodes = set()
        for descriptor in Path(f'/proc/{pid}/fd').iterdir():
            try:
                match = re.fullmatch(r'socket:\[(\d+)\]', os.readlink(descriptor))
                if match:
                    inodes.add(match[1])
            except FileNotFoundError:
                pass
        counts = {port: 0 for port in ports}
        for line in Path(f'/proc/{pid}/net/tcp').read_text().splitlines()[1:]:
            fields = line.split()
            port = int(fields[2].split(':')[1], 16)
            if fields[3] == '02' and fields[9] in inodes and port in counts:
                require(fields[2].split(':')[0] == '0100007F', 'probe escaped its owned loopback targets')
                counts[port] += 1
        return counts
    except FileNotFoundError:
        return None


def state_rows(path, sql, args=()):
    with closing(sqlite3.connect(f'file:{path}?mode=ro', uri=True, timeout=2)) as connection:
        return connection.execute(sql, args).fetchall()


def cached(path):
    rows = state_rows(path, "SELECT value FROM kv WHERE key='probes:configuration'")
    return json.loads(rows[0][0]) if rows else None


def echo(port):
    native = runpy.run_path(str(Path(__file__).with_name('native-service-smoke.py')))
    native['transfer'](port)


def traffic_loop(rows, stop, port):
    while not stop.is_set():
        started = time.monotonic()
        try:
            completed = subprocess.run([sys.executable, str(Path(__file__).resolve()), '--echo-worker', str(port)],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=3)
            value = {'ok': completed.returncode == 0, 'exit_code': completed.returncode}
        except subprocess.TimeoutExpired:
            value = {'ok': False, 'deadline_exceeded': True}
        except Exception as error:
            value = {'ok': False, 'error_kind': type(error).__name__}
        rows.append({'at': time.time(), 'elapsed': time.monotonic() - started, **value})
        stop.wait(1)


def main(args):
    require(__debug__ and sys.platform == 'linux' and os.geteuid() == 0, 'unoptimized root Linux controller required')
    require(os.environ.get('APSP_REMAINING_TEST_SIGNAL') == '1', 'explicit final-test signal required')
    require(socket.gethostname() == 'lima-apsp-p0-debian12', 'dedicated Debian guest required')
    require(re.fullmatch(r'[0-9a-f]{40}', args.source_commit), 'explicit frozen source commit required')
    require(re.fullmatch(r'apsp-carrier-monitor-20\d{6}-r[1-9]\d{0,2}', args.output.name), 'fresh owned carrier namespace required')
    require(args.output.parent == Path('/home/l7.guest') and not args.output.exists(), 'fresh private guest output required')
    build = json.loads((args.built_root / 'build-v3-result.json').read_text())
    require(build['exit_code'] == 0 and build['reserve_stop'] is None and build['test_only'], 'successful current TEST_ONLY build required')
    require(build['source']['source_commit'] == args.source_commit, 'build does not match current source')
    require(sha(Path(__file__)) == build['source']['input_hashes']['tools/carrier-monitoring-acceptance.py'], 'controller differs from frozen source')
    for name in ('apsp-agent', 'apsp-panel'):
        require(sha(args.built_root / 'bin' / name) == build['binaries'][name]['sha256'], 'current frozen executable changed')
    runtime_receipt = json.loads(args.runtime_receipt.read_text())
    require(all(runtime_receipt[key] is True for key in ('trusted_signature_verified', 'archive_identity_verified', 'binary_identity_verified')), 'original runtime signature proofs required')
    require(runtime_receipt['binary_sha256'] == RUNTIME_SHA and sha(args.runtime) == RUNTIME_SHA, 'original runtime changed')
    os.umask(0o077)
    args.output.mkdir(mode=0o700)
    common, finish = private_helpers(args.built_root, args.output)
    common.reject_non_guest()
    require(not common.PG.exists() and not common.PUBLIC_ROOT.exists(), 'owned PG/runtime root already exists')
    for port in (PG_PORT, PANEL_PORT, GATE_PORT):
        with socket.socket() as check:
            check.bind(('127.0.0.1', port))
    common.write('SOURCE.json', build['source'])
    common.write('before-runtime.json', {**common.snapshot(), 'kernel_cursor': common.kernel_cursor()})
    common.ownership(units=[])
    result = {'ok': False, 'source_commit': args.source_commit, 'test_only': True, 'steps': {}, 'errors': []}
    phase, mounted, gate, slow, worker = 'setup', False, None, None, None
    storage_reader = None
    proxy_port = None
    stop = threading.Event()
    traffic, observations = [], []
    state_dir = args.output / 'private-state'
    filler = state_dir / 'TEST_ONLY_disk_fill'
    try:
        common.command(['install', '-d', '-o', 'postgres', '-g', 'postgres', '-m', '700', str(common.PG)])
        common.ownership(pg=True)
        with (args.output / 'initdb.log').open('x') as output:
            completed = subprocess.run(['runuser', '-u', 'postgres', '--', '/usr/lib/postgresql/15/bin/initdb', '-D', str(common.PG), '--auth-local=trust', '--auth-host=trust', '--encoding=UTF8', '--no-locale'], stdout=output, stderr=subprocess.STDOUT, timeout=60)
        require(completed.returncode == 0, 'fresh PG init failed')
        common.start('pg', ['/usr/lib/postgresql/15/bin/postgres', '-D', str(common.PG), '-p', str(PG_PORT), '-h', '127.0.0.1', '-k', str(common.PG), '-c', 'shared_buffers=32MB', '-c', 'max_connections=20', '-c', 'max_wal_size=128MB'], '192M', extra=['--property=User=postgres', '--property=Group=postgres'])
        common.wait_for(lambda: subprocess.run(['/usr/lib/postgresql/15/bin/pg_isready', '-h', '127.0.0.1', '-p', str(PG_PORT), '-U', 'postgres'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=2).returncode == 0, timeout=20)
        common.command(['runuser', '-u', 'postgres', '--', '/usr/lib/postgresql/15/bin/createdb', '-h', str(common.PG), '-p', str(PG_PORT), common.DATABASE])
        password = secrets.token_urlsafe(32)
        (args.output / 'admin-password').write_text(password)
        env = {'APSP_DATABASE_URL': f'postgres://postgres@127.0.0.1:{PG_PORT}/{common.DATABASE}', 'APSP_PUBLIC_URL': common.ORIGIN, 'APSP_LISTEN': f'127.0.0.1:{PANEL_PORT}', 'APSP_DATA_DIR': str(args.output / 'panel-data'), 'APSP_ADMIN_PASSWORD': password, 'RUST_LOG': 'info', 'PATH': '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'}
        (args.output / 'panel.env').write_text(''.join(key + '=' + value + '\n' for key, value in env.items()))
        common.start('panel', [str(args.built_root / 'bin/apsp-panel')], '256M', extra=['--property=EnvironmentFile=' + str(args.output / 'panel.env')])
        common.wait_for(lambda: common.port_open(PANEL_PORT), timeout=60)
        gate = Gate()
        api = common.API()
        api.login()
        server = api.expect('POST', '/api/servers', {'name': 'TEST_ONLY carrier loopback Debian', 'agent_settings': {'sample_interval_secs': 1, 'upload_interval_secs': 3, 'auto_update': False, 'discover_public_ips': False}}, status=201)
        sid = server['id']
        common.write('identity-meta.json', {'server_id': sid, 'source_commit': args.source_commit})
        slow = SlowTargets()
        specs = []
        for index, port in enumerate(slow.ports):
            spec = {'id': str(uuid.UUID(int=0)), 'name': 'TEST_ONLY slow loopback ' + str(index), 'kind': 'tcp', 'target': '127.0.0.1', 'port': port, 'interval_secs': 10, 'carrier': '', 'enabled': True, 'monitor': {'network': ['telecom', 'unicom', 'mobile', 'other'][index], 'region': 'TEST_ONLY owned loopback', 'address_family': 'ipv4', 'authorization': {'kind': 'owned', 'enabled': True, 'source': 'TEST_ONLY controller owns these listener sockets', 'scope': f'TEST_ONLY server {sid}, TCP to 127.0.0.1:{port}, four attempts every ten seconds', 'expires_at': None, 'identity': {'kind': 'tcp', 'target': '127.0.0.1', 'port': port, 'address_family': 'ipv4'}}}}
            specs.append(api.expect('POST', f'/api/servers/{sid}/probes', spec))
        state_dir.mkdir(mode=0o700)
        common.command(['mount', '-t', 'tmpfs', '-o', 'size=16m,mode=0700,nodev,nosuid,noexec', 'apsp-carrier-test-only', str(state_dir)])
        mounted = True
        db = state_dir / 'state.db'
        settings = {'sample_interval_secs': 1, 'upload_interval_secs': 3, 'auto_update': False, 'discover_public_ips': False}
        config = {'panel_url': common.ORIGIN, 'identity_dir': str(args.output / 'identity'), 'state_db': str(db), 'runtime_root': str(args.output / 'runtime'), 'install_root': str(args.output / 'install'), 'agent_root': str(args.output / 'core'), 'status_socket': str(args.output / 'status.sock'), 'operation_timeout_secs': 3, 'public_ips': [], 'allow_remote_commands': False}
        (args.output / 'agent.toml').write_text(''.join(key + '=' + json.dumps(value) + '\n' for key, value in config.items()) + '[settings]\n' + ''.join(key + '=' + json.dumps(value) + '\n' for key, value in settings.items()))
        enrollment = api.expect('POST', f'/api/servers/{sid}/enrollment', {})
        with (args.output / 'enroll.log').open('x') as output:
            completed = subprocess.run([str(args.built_root / 'bin/apsp-agent'), '--config', str(args.output / 'agent.toml'), 'enroll', '--panel', common.ORIGIN, '--token', enrollment['token']], stdout=output, stderr=subprocess.STDOUT, timeout=30)
        del enrollment
        require(completed.returncode == 0, 'real current Agent enrollment failed')
        common.PUBLIC_ROOT.mkdir(mode=0o755)
        common.PUBLIC_ROOT.chmod(0o755)
        common.ownership(public_root=True)
        (common.PUBLIC_ROOT / '.owned-by-apsp-plugin-install').write_bytes(str(args.output).encode() + b'\n')
        runtime = common.PUBLIC_ROOT / 'runtime'
        shutil.copyfile(args.runtime, runtime)
        runtime.chmod(0o555)
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            proxy_port = listener.getsockname()[1]
        runtime_config = common.PUBLIC_ROOT / 'config.json'
        runtime_config.write_text(json.dumps({'log': {'level': 'error'}, 'inbounds': [{'type': 'vless', 'tag': 'TEST_ONLY', 'listen': '127.0.0.1', 'listen_port': proxy_port, 'users': [{'uuid': str(uuid.UUID(int=1))}]}], 'outbounds': [{'type': 'direct', 'tag': 'direct'}], 'route': {'rules': [{'ip_cidr': ['127.0.0.0/8', '::1/128'], 'outbound': 'direct'}, {'action': 'reject'}], 'final': 'direct'}}))
        runtime_config.chmod(0o444)
        common.start('runtime', [str(runtime), 'run', '-c', str(runtime_config)], '96M', weight=200, oom=-500, extra=['--property=User=nobody'])
        common.start('agent', [str(args.built_root / 'bin/apsp-agent'), '--config', str(args.output / 'agent.toml'), 'run', '--monitor-only'], '128M', weight=200, oom=-500)
        common.wait_for(lambda: api.server().get('last_heartbeat_at'), timeout=60)
        original_runtime = common.fingerprint(common.unit('runtime'))
        agent_pid = common.fingerprint(common.unit('agent'))['pid']
        identity = {'key_sha256': sha(args.output / 'identity/device.key'), 'server_id': sid}
        worker = threading.Thread(target=traffic_loop, args=(traffic, stop, proxy_port), daemon=True)
        worker.start()
        common.wait_for(lambda: traffic, timeout=5)

        def guard(connected=True):
            common.reserves()
            require(common.fingerprint(common.unit('runtime')) == original_runtime, 'independent proxy runtime changed')
            for role, cap in (('agent', 128 * 1024**2), ('runtime', 96 * 1024**2)):
                properties = common.properties(common.unit(role))
                require(properties['ActiveState'] == 'active', 'resident workload stopped under monitoring load')
                require(int(properties['MemoryMax']) == cap and properties['MemorySwapMax'] == '0', 'workload memory limit changed')
                events = dict(line.split() for line in properties['cgroup']['memory.events'].splitlines())
                require(events['oom'] == '0' and events['oom_kill'] == '0', 'workload cgroup reported an OOM event')
            require(worker.is_alive() and traffic and time.time() - traffic[-1]['at'] <= 5,
                    'continuous loopback traffic stopped')
            require(all(row['ok'] for row in traffic), 'an actual bounded loopback transfer failed')
            connects = active_connects(agent_pid, slow.ports)
            require(connects is not None and sum(connects.values()) <= 4, 'Agent probe concurrency exceeded four')
            value = {'at': time.time(), 'phase': phase, 'connects': connects}
            if connected:
                server = api.server()
                beat = server.get('last_heartbeat_at')
                require(type(beat) is int and time.time() - beat <= 30, 'actual heartbeat stale beyond 30 seconds')
                value.update(heartbeat=beat, metrics_sampled_at=server.get('metrics_sampled_at'))
            observations.append(value)
            return value

        def window(seconds, connected=True):
            first = len(observations)
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                guard(connected)
                time.sleep(.5)
            if connected and seconds >= 42:
                beats = sorted(set(row['heartbeat'] for row in observations[first:]))
                require(len(beats) >= 2 and beats[-1] - beats[0] >= 19,
                        'real heartbeat did not advance over the observation window')
                require(all(0 < after - before <= 30 for before, after in zip(beats, beats[1:])),
                        'real heartbeat interval exceeded thirty seconds')

        def phase_start(name):
            nonlocal phase
            phase = name
            print(json.dumps({'phase': phase}), flush=True)

        phase_start('four_slow_targets_low_memory')
        window(45)
        require(max(sum(row['connects'].values()) for row in observations) == 4, 'four actual simultaneous slow TCP connects were not observed')
        require(gate.attempts and {row['result']['probe_id'] for row in gate.attempts} == {spec['id'] for spec in specs}, 'actual slow-target results are missing')
        require(all(row['result']['attempts'] == 4 and row['result']['loss_percent'] == 100 and row['result']['latency_ms'] is None and 'timed out' in row['result']['error'] for row in gate.attempts), 'real slow connections did not retain truthful failed attempts')
        result['steps'][phase] = {'agent': common.properties(common.unit('agent')), 'runtime': common.properties(common.unit('runtime')), 'max_active_probes': 4}
        phase_start('restart_unacked_outbox')
        gate.upload = False
        common.wait_for(lambda: len(state_rows(db, 'SELECT id FROM probe_outbox')) >= 4, timeout=25)
        pending = {row[0]: json.loads(row[1]) for row in state_rows(db, 'SELECT id,result FROM probe_outbox')}
        before_pid = agent_pid
        restarted_at = time.time()
        common.command(['systemctl', 'restart', common.unit('agent')], timeout=30)
        agent_pid = common.wait_for(lambda: (value if (value := common.fingerprint(common.unit('agent'))['pid']) != before_pid else None), timeout=20)
        common.wait_for(lambda: all(any(item['result'] == value for item in gate.attempts if item['at'] > restarted_at) for value in pending.values()), timeout=20)
        gate.upload = True
        common.wait_for(lambda: set(pending).issubset(gate.acked), timeout=20)
        require(sha(args.output / 'identity/device.key') == identity['key_sha256'] and api.server()['id'] == sid, 'restart changed enrolled identity')
        ids = ','.join("'" + identifier + "'" for identifier in pending)
        require(int(common.sql(f'SELECT count(*) FROM probe_results WHERE id IN ({ids})')) == len(pending), 'replayed results were lost or duplicated')
        # Repeat an actually accepted authenticated batch, never a fabricated sample.
        connection = http.client.HTTPConnection('127.0.0.1', PANEL_PORT, timeout=3)
        try:
            connection.request('POST', '/api/agent/v1/probe-results', json.dumps({'results': list(pending.values())}), {'Content-Type': 'application/json', 'Authorization': gate.authorization})
            response = connection.getresponse(); response.read(MAX_BODY)
            require(response.status == 200, 'real duplicate replay was rejected')
        finally:
            connection.close()
        require(int(common.sql(f'SELECT count(*) FROM probe_results WHERE id IN ({ids})')) == len(pending), 'duplicate replay added history rows')
        window(42)
        result['steps'][phase] = {'pending_ids': list(pending), 'replayed_same_samples': True, 'unique_history_rows': len(pending), 'agent_pid_changed': before_pid != agent_pid, 'identity_unchanged': True}
        phase_start('disconnect_lease_expiry')
        last_fetch = cached(db)[0]
        gate.disconnect()
        disconnected = time.time()
        window(123, connected=False)
        require(time.time() - last_fetch >= 120 and sum(active_connects(agent_pid, slow.ports).values()) == 0, 'expired configuration continued active probes')
        count = len(state_rows(db, 'SELECT id FROM probe_outbox'))
        window(12, connected=False)
        require(len(state_rows(db, 'SELECT id FROM probe_outbox')) == count, 'expired configuration kept sampling offline')
        gate.available = True
        disconnected_seconds = time.time() - disconnected
        common.wait_for(lambda: api.server()['last_heartbeat_at'] > disconnected, timeout=75)
        common.wait_for(lambda: cached(db)[0] > last_fetch, timeout=40)
        window(42)
        result['steps'][phase] = {'observed_disconnect_seconds': disconnected_seconds, 'last_config_fetched_at': last_fetch, 'active_after_expiry': 0, 'outbox_stable_after_expiry': True}
        phase_start('private_filesystem_enospc')
        # Fill only a dedicated 16 MiB tmpfs. Never allocate guest root disk space.
        # Pin a short owned WAL snapshot so real writes cannot recycle previously
        # allocated WAL capacity while the dedicated filesystem is actually full.
        storage_reader = sqlite3.connect(db, timeout=2)
        checkpoint = storage_reader.execute('PRAGMA wal_checkpoint(TRUNCATE)').fetchone()
        require(checkpoint[0] == 0, 'private WAL checkpoint remained busy')
        storage_reader.execute("INSERT OR REPLACE INTO kv(key,value) VALUES(?,?)", ('TEST_ONLY_carrier_full_snapshot', json.dumps(time.time())))
        storage_reader.commit()
        storage_reader.execute('BEGIN')
        require(storage_reader.execute("SELECT value FROM kv WHERE key=?", ('TEST_ONLY_carrier_full_snapshot',)).fetchone(), 'private WAL snapshot is missing')
        require(Path(str(db) + '-wal').stat().st_size <= 65536, 'bounded private WAL setup was raced by unrelated writes')
        before_disk = os.statvfs(state_dir)
        with filler.open('xb') as output:
            try:
                os.posix_fallocate(output.fileno(), 0, before_disk.f_bavail * before_disk.f_frsize)
            except OSError as error:
                require(error.errno == 28, 'private filler failed for a reason other than ENOSPC')
            output.flush()
        # Consume any race-created remainder with a bounded block write.
        with filler.open('ab', buffering=0) as output:
            for _ in range(4096):
                try:
                    output.write(b'x' * 4096); output.flush()
                except OSError as error:
                    require(error.errno == 28, 'private filesystem failure was not ENOSPC')
                    break
            else:
                raise RuntimeError('private tmpfs did not become full within its 16 MiB bound')
        require(os.statvfs(state_dir).f_bavail == 0, 'private state filesystem is not actually full')
        window(45)
        text = (args.output / 'agent.log').read_text()
        require('state storage temporarily unavailable' in text and ('database or disk is full' in text or 'disk I/O error' in text), 'actual Agent storage failure was not observed')
        storage_reader.close()
        storage_reader = None
        filler.unlink()
        count = int(common.sql(f'SELECT count(*) FROM probe_results WHERE server_id={sid}'))
        common.wait_for(lambda: int(common.sql(f'SELECT count(*) FROM probe_results WHERE server_id={sid}')) > count, timeout=25)
        window(42)
        require('state storage recovered' in (args.output / 'agent.log').read_text(), 'storage did not recover through its normal retry')
        require(common.fingerprint(common.unit('agent'))['pid'] == agent_pid, 'Agent restarted after its private filesystem filled')
        result['steps'][phase] = {'filesystem': 'private 16MiB tmpfs', 'actual_enospc': True, 'agent_pid_unchanged': common.fingerprint(common.unit('agent'))['pid'] == agent_pid, 'recovered': True}
        phase_start('revoke_pause_delete_late_results')
        gate.upload = False
        common.wait_for(lambda: any(json.loads(row[0])['probe_id'] == specs[0]['id'] for row in state_rows(db, 'SELECT result FROM probe_outbox')), timeout=25)
        late = {row[0]: json.loads(row[1]) for row in state_rows(db, 'SELECT id,result FROM probe_outbox') if json.loads(row[1])['probe_id'] == specs[0]['id']}
        require(late, 'real removed-target late outbox is missing')
        api.expect('DELETE', f'/api/servers/{sid}/probes/{specs[0]["id"]}')
        specs[1]['enabled'] = False
        specs[1]['monitor']['authorization']['enabled'] = False
        api.expect('PATCH', f'/api/servers/{sid}/probes/{specs[1]["id"]}', specs[1])
        for spec in specs[2:]:
            spec['enabled'] = False
            api.expect('PATCH', f'/api/servers/{sid}/probes/{spec["id"]}', spec)
        common.wait_for(lambda: (value := cached(db)) and all(item['id'] != specs[0]['id'] and (not item['enabled'] or not item['monitor']['authorization']['enabled']) for item in value[1]), timeout=40)
        common.wait_for(lambda: sum(active_connects(agent_pid, slow.ports).values()) == 0, timeout=4)
        gate.upload = True
        common.wait_for(lambda: set(late).issubset(gate.acked), timeout=20)
        late_ids = ','.join("'" + identifier + "'" for identifier in late)
        require(int(common.sql(f'SELECT count(*) FROM probe_results WHERE id IN ({late_ids})')) == 0, 'deleted target late results were reassigned or inserted')
        preserved = int(common.sql(f'SELECT count(*) FROM probe_results WHERE probe_id=\'{specs[0]["id"]}\''))
        require(preserved > 0, 'deleting target erased its existing history')
        window(42)
        require(all(sum(row['connects'].values()) == 0 for row in observations if row['phase'] == phase and row['at'] > time.time() - 40), 'paused or revoked target resumed requests')
        result['steps'][phase] = {'late_real_samples': len(late), 'late_ack_discard': True, 'old_history_rows_preserved': preserved, 'all_active_targets_stopped': True}
        result['ok'] = True
    except BaseException as error:
        result['failed_phase'] = phase
        result['error_kind'] = type(error).__name__
        if type(error) is RuntimeError:
            result['controlled_reason'] = str(error)
    finally:
        stop.set()
        if worker:
            worker.join(timeout=6)
            if worker.is_alive():
                result['errors'].append('traffic_worker_did_not_stop')
        result['traffic'] = traffic
        result['observations'] = observations
        result['ok'] = result['ok'] and bool(traffic) and all(row['ok'] for row in traffic)
        for name, action in [('release_private_wal_reader', lambda: storage_reader.close() if storage_reader else None),
                             ('release_private_filler', lambda: filler.unlink(missing_ok=True)),
                             ('stop_agent', lambda: common.stop(common.unit('agent'))),
                             ('stop_runtime', lambda: common.stop(common.unit('runtime')))]:
            try:
                action()
            except BaseException as error:
                result['errors'].append({'step': name, 'error_kind': type(error).__name__})
        if mounted:
            try:
                with closing(sqlite3.connect(state_dir / 'state.db')) as source, closing(sqlite3.connect(args.output / 'retained-private-state.db')) as destination:
                    source.backup(destination)
            except BaseException as error:
                result['errors'].append({'step': 'retain_private_state', 'error_kind': type(error).__name__})
            try:
                common.command(['umount', str(state_dir)])
                mounted = False
            except BaseException as error:
                result['errors'].append({'step': 'unmount_private_state', 'error_kind': type(error).__name__})
        if gate:
            try:
                gate.disconnect(); gate.shutdown(); gate.server_close()
                result['errors'].extend(gate.errors)
            except BaseException as error:
                result['errors'].append({'step': 'close_owned_gate', 'error_kind': type(error).__name__})
        if slow:
            slow.close()
        try:
            require(all(not common.port_open(port) for port in ([GATE_PORT] + (slow.ports if slow else []) + ([proxy_port] if proxy_port else []))), 'owned carrier listener remains')
        except BaseException as error:
            result['errors'].append({'step': 'owned_loopback_listeners_closed', 'error_kind': type(error).__name__})
        try:
            result['cleanup'] = finish.cleanup()
            result['ok'] = result['ok'] and result['cleanup']['ok']
        except BaseException as error:
            result['errors'].append({'step': 'continue_owned_cleanup', 'error_kind': type(error).__name__})
        result['ok'] = result['ok'] and not result['errors'] and not mounted
        result['finished_at'] = time.time()
        common.write('carrier-result.json', result)
        print(json.dumps({'ok': result['ok'], 'failed_phase': result.get('failed_phase'), 'errors': result['errors'], 'proxy_echoes': len(traffic), 'proxy_failures': sum(not row['ok'] for row in traffic)}), flush=True)
    require(result['ok'], 'carrier monitoring or safe cleanup failed; preserve the first receipt')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--echo-worker', type=int)
    parser.add_argument('--built-root', type=Path)
    parser.add_argument('--source-commit')
    parser.add_argument('--runtime', type=Path)
    parser.add_argument('--runtime-receipt', type=Path)
    parser.add_argument('--output', type=Path)
    arguments = parser.parse_args()
    if arguments.echo_worker is not None:
        require(__debug__ and 0 < arguments.echo_worker <= 65535, 'bounded loopback echo worker required')
        echo(arguments.echo_worker)
    else:
        require(all(getattr(arguments, name) is not None for name in ('built_root', 'source_commit', 'runtime', 'runtime_receipt', 'output')), 'all frozen inputs must be explicit')
        main(arguments)
