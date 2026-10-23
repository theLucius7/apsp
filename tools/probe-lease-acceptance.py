#!/usr/bin/env python3
"""TEST_ONLY Debian 90-second probe leases with real Agent/Panel/PG and continuous loopback proxy traffic."""
import argparse
from contextlib import closing, contextmanager
import hashlib
import http.client
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import platform
import pwd
import re
import resource
import runpy
import secrets
import select
import shutil
import signal
import socket
import sqlite3
import stat
import subprocess
import sys
import threading
import time
import types
import uuid

RUNTIME_SHA = 'fee83ca8457c94449dd04aa17a51830cbc9b449a4dda290e995d3366188e0302'
PG_PORT, PANEL_PORT, GATE_PORT = 55683, 55783, 55883
MAX_BODY = 1024 * 1024
MAX_INPUT = 512 * 1024**2
CONTROLLER_SECONDS = 1800
CLEANUP_SECONDS = 120
MAX_GATE_REQUESTS = 16
MAX_GATE_EVENTS = 32768
MAX_CONTROLLER_FDS = 256
MAX_ACTOR_FDS = 256


class ControlledFailure(RuntimeError):
    pass


class ControllerDeadline(ControlledFailure):
    pass


@contextmanager
def finite_controller():
    """Signals enter normal owned cleanup; cleanup retains a finite timer."""
    require(signal.getitimer(signal.ITIMER_REAL) == (0.0, 0.0), 'controller cannot replace another active timer')
    previous = {number: signal.getsignal(number) for number in (signal.SIGALRM, signal.SIGTERM, signal.SIGHUP)}

    def expired(_number, _frame):
        # A cleanup operation may catch the first timeout. Keep interrupting
        # subsequent blocking cleanup operations until the controller exits.
        signal.setitimer(signal.ITIMER_REAL, 1)
        raise ControllerDeadline('finite private controller deadline exceeded')

    def cancelled(number, _frame):
        raise SystemExit(128 + number)

    try:
        signal.signal(signal.SIGALRM, expired)
        signal.signal(signal.SIGTERM, cancelled)
        signal.signal(signal.SIGHUP, cancelled)
        signal.setitimer(signal.ITIMER_REAL, CONTROLLER_SECONDS)
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        for number, handler in previous.items():
            signal.signal(number, handler)


def require(value, reason):
    if not value:
        raise ControlledFailure(reason)


@contextmanager
def deferred_controller_signals():
    previous = signal.pthread_sigmask(signal.SIG_BLOCK,
        {signal.SIGINT, signal.SIGTERM, signal.SIGHUP, signal.SIGALRM})
    try:
        yield
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous)


def sha(path):
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
    with os.fdopen(descriptor, 'rb') as stream:
        metadata = os.fstat(stream.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1
                and 0 < metadata.st_size <= MAX_INPUT, 'bounded ordinary frozen input required')
        value, length = hashlib.sha256(), 0
        while True:
            chunk = stream.read(min(65536, MAX_INPUT - length + 1))
            if not chunk:
                break
            length += len(chunk)
            require(length <= MAX_INPUT, 'frozen input exceeds its byte limit')
            value.update(chunk)
        require(length == metadata.st_size, 'frozen input changed while reading')
        return value.hexdigest()


def read_private(path, limit=MAX_BODY):
    descriptor = os.open(path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
    with os.fdopen(descriptor, 'rb') as stream:
        metadata = os.fstat(stream.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1 and metadata.st_size <= limit,
                'bounded ordinary private input required')
        data = stream.read(limit + 1)
        require(len(data) == metadata.st_size and len(data) <= limit, 'private input changed or exceeded limit')
        return data


def native_debian():
    require(platform.machine() in ('x86_64', 'aarch64'), 'native amd64/arm64 controller required')
    release = Path('/etc/os-release')
    if release.is_symlink():
        require(release.resolve(strict=True) == Path('/usr/lib/os-release'), 'unapproved release link')
        release = Path('/usr/lib/os-release')
    fields = dict(line.split('=', 1) for line in read_private(release, 65536).decode().splitlines() if '=' in line)
    require(fields.get('ID', '').strip('\"\'') == 'debian'
            and fields.get('VERSION_ID', '').strip('\"\'') == '12', 'native Debian 12 controller required')


def bounded_backup(source_path, destination_path):
    deadline = time.monotonic() + 10

    def progress(_status, _remaining, _total):
        require(time.monotonic() < deadline, 'private state backup deadline exceeded')

    with closing(sqlite3.connect(source_path, timeout=.5)) as source, \
            closing(sqlite3.connect(destination_path, timeout=.5)) as destination:
        source.backup(destination, pages=128, progress=progress, sleep=.05)


def private_helpers(built_root, output, manifest_sha256):
    manifest_bytes = read_private(built_root / 'harness-identity.json')
    require(re.fullmatch(r'[0-9a-f]{64}', manifest_sha256 or '')
            and hashlib.sha256(manifest_bytes).hexdigest() == manifest_sha256,
            'explicit trusted helper manifest digest differs')
    manifest = json.loads(manifest_bytes)
    require(re.fullmatch(r'apsp-plugin-install-20\d{6}-r[1-9]\d{0,2}', built_root.name), 'reviewed build namespace required')
    scripts = {}
    for name in ('common', 'finish'):
        data = read_private(built_root / (name + '.py'))
        require(hashlib.sha256(data).hexdigest() == manifest['script_sha256'][name + '.py'], 'reviewed helper digest differs')
        scripts[name] = data
    modules = {}
    # Validate the complete pair before executing either reviewed helper.
    for name, data in scripts.items():
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
    request_queue_size = MAX_GATE_REQUESTS

    def __init__(self):
        self.request_slots = threading.BoundedSemaphore(MAX_GATE_REQUESTS)
        super().__init__(('127.0.0.1', GATE_PORT), GateHandler)
        self.upload = True
        self.available = True
        self.lock = threading.RLock()
        self.connections = set()
        self.attempts = []
        self.acked = set()
        self.authorization = ''
        self.errors = []
        self.lease_mode = 'deny'
        self.leases = []
        self.lease_faults = []
        self.probe_pid = None
        self.probe_ports = []
        threading.Thread(target=self.serve_forever, daemon=True).start()

    def process_request(self, request, client_address):
        if not self.request_slots.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, client_address)
        except BaseException:
            self.request_slots.release()
            raise

    def process_request_thread(self, request, client_address):
        try:
            super().process_request_thread(request, client_address)
        finally:
            self.request_slots.release()

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

    def setup(self):
        super().setup()
        self.connection.settimeout(3)

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
            # select bounds waiting; both forwarding writes retain timeouts.
            backend.settimeout(3)
            deadline = time.monotonic() + CONTROLLER_SECONDS
            try:
                while self.server.available and time.monotonic() < deadline:
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
        if self.path == '/api/agent/v1/probe-lease' and self.server.lease_mode != 'allow':
            mode = self.server.lease_mode
            deadline = time.monotonic() + 1.5
            active = 0
            while self.server.probe_pid and time.monotonic() < deadline:
                active = sum((active_connects(self.server.probe_pid, self.server.probe_ports) or {}).values())
                if active:
                    break
                time.sleep(.02)
            with self.server.lock:
                require(len(self.server.lease_faults) < MAX_GATE_EVENTS, 'bounded lease fault evidence required')
                self.server.lease_faults.append({'at': time.time(), 'mode': mode, 'active_connects': active})
            if mode == 'hang':
                time.sleep(7)
                return self.respond(503, b'{"error":"TEST_ONLY hung lease request"}')
            return self.respond(int(mode) if mode in ('401', '403') else 503,
                                b'{"error":"TEST_ONLY lease permission withheld"}')
        size = int(self.headers.get('Content-Length', '0'))
        require(0 <= size <= MAX_BODY, 'bounded loopback request required')
        body = self.rfile.read(size)
        require(len(body) == size, 'complete bounded loopback request required')
        if self.path.startswith('/api/agent/') and self.headers.get('Authorization'):
            self.server.authorization = self.headers['Authorization']
        probe_results = None
        if self.path == '/api/agent/v1/probe-results':
            probe_results = json.loads(body)['results']
            require(0 < len(probe_results) <= 64, 'bounded real probe batch required')
            with self.server.lock:
                require(len(self.server.attempts) + len(probe_results) <= MAX_GATE_EVENTS,
                        'bounded probe attempt evidence required')
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
            if self.path == '/api/agent/v1/probe-lease' and response.status == 200:
                snapshot = json.loads(data)
                require(type(snapshot['server_id']) is int and type(snapshot['revision']) is int,
                        'real lease device and revision required')
                require(0 < snapshot['expires_at'] - snapshot['issued_at'] <= 90,
                        'real issuance exceeded ninety seconds')
                # Store only the issued permission's safe identity and monotonic observation.
                # Session bearer credentials are never copied into receipts.
                with self.server.lock:
                    require(len(self.server.leases) < MAX_GATE_EVENTS, 'bounded lease issuance evidence required')
                    self.server.leases.append({**{key: snapshot[key] for key in
                        ('id', 'server_id', 'revision', 'issued_at', 'expires_at')},
                        'at': time.time(), 'received_monotonic': time.monotonic(),
                        'probe_ids': [item['spec']['id'] for item in snapshot['probes']]})
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
        try:
            for _ in range(4):
                listener = socket.socket()
                self.sockets.append(listener)
                listener.bind(('127.0.0.1', 0))
                listener.listen(1)
                self.ports.append(listener.getsockname()[1])
                for _ in range(16):
                    connection = socket.socket()
                    self.sockets.append(connection)
                    connection.settimeout(.15)
                    try:
                        connection.connect(listener.getsockname())
                    except TimeoutError:
                        connection.close()
                        break
                else:
                    raise ControlledFailure('owned slow listener backlog did not saturate')
        except BaseException:
            self.close()
            raise

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


def latest_lease(gate):
    with gate.lock:
        return dict(gate.leases[-1]) if gate.leases else None


def old_cache_is_inert(path):
    rows = state_rows(path, "SELECT value FROM kv WHERE key='probes:configuration'")
    return not rows


def echo(port_number):
    # Plain loopback VLESS is only a transport fixture; production uses Reality.
    with socket.socket() as echo:
        echo.settimeout(5)
        echo.bind(('127.0.0.1', 0))
        echo.listen()
        with socket.create_connection(('127.0.0.1', port_number), timeout=5) as proxy:
            body = bytes([42]) * 1024
            proxy.sendall(b'\0' + uuid.UUID(int=1).bytes + b'\0\1' +
                          echo.getsockname()[1].to_bytes(2, 'big') + b'\1\x7f\0\0\1' + body)
            stream, _ = echo.accept()
            with stream:
                received = bytearray()
                while len(received) < len(body):
                    received.extend(stream.recv(len(body) - len(received)))
                assert bytes(received) == body
                stream.sendall(body)
            received = bytearray()
            while len(received) < 1026:
                chunk = proxy.recv(1026 - len(received))
                assert chunk
                received.extend(chunk)
            assert received == b'\0\0' + body

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
    native_debian()
    require(re.fullmatch(r'[0-9a-f]{40}', args.source_commit), 'explicit frozen source commit required')
    require(re.fullmatch(r'apsp-probe-lease-20\d{6}-r[1-9]\d{0,2}', args.output.name), 'fresh owned probe lease namespace required')
    require(args.output.parent == Path('/home/l7.guest') and not args.output.exists(), 'fresh private guest output required')
    require(args.built_root.parent == Path('/home/l7.guest') and not args.built_root.is_symlink(),
            'private reviewed guest build namespace required')
    build_directory = args.built_root.lstat()
    require(stat.S_ISDIR(build_directory.st_mode) and not build_directory.st_mode & 0o022
            and build_directory.st_uid in (0, pwd.getpwnam('l7.guest').pw_uid),
            'ordinary owned private build directory required')
    build = json.loads(read_private(args.built_root / 'build-v3-result.json', 8 * MAX_BODY))
    require(build['exit_code'] == 0 and build['reserve_stop'] is None and build['test_only'], 'successful current TEST_ONLY build required')
    require(build['source']['source_commit'] == args.source_commit, 'build does not match current source')
    controller_sha = sha(Path(__file__))
    frozen_controller = build['source']['input_hashes'].get('tools/probe-lease-acceptance.py')
    if frozen_controller == controller_sha:
        controller_source = args.source_commit
    else:
        require(args.controller_sha256 == controller_sha
                and args.controller_source_commit is not None
                and re.fullmatch(r'[0-9a-f]{40}', args.controller_source_commit),
                'separately frozen controller source and explicit trusted digest required')
        controller_source = args.controller_source_commit
    controller_identity = {'source_commit': controller_source, 'sha256': controller_sha,
                           'binary_source_commit': args.source_commit,
                           'separately_frozen': frozen_controller != controller_sha}

    for name in ('apsp-agent', 'apsp-panel'):
        require(sha(args.built_root / 'bin' / name) == build['binaries'][name]['sha256'], 'current frozen executable changed')
    runtime_receipt = json.loads(read_private(args.runtime_receipt))
    require(all(runtime_receipt[key] is True for key in ('trusted_signature_verified', 'archive_identity_verified', 'binary_identity_verified')), 'original runtime signature proofs required')
    require(runtime_receipt['binary_sha256'] == RUNTIME_SHA and sha(args.runtime) == RUNTIME_SHA, 'original runtime changed')
    os.umask(0o077)
    fd_soft, fd_hard = resource.getrlimit(resource.RLIMIT_NOFILE)
    require(fd_hard == resource.RLIM_INFINITY or fd_hard >= MAX_CONTROLLER_FDS, 'controller FD hard limit is insufficient')
    controller_fd_limit = MAX_CONTROLLER_FDS if fd_soft == resource.RLIM_INFINITY else min(fd_soft, MAX_CONTROLLER_FDS)
    resource.setrlimit(resource.RLIMIT_NOFILE, (controller_fd_limit, fd_hard))
    args.output.mkdir(mode=0o700)
    common, finish = private_helpers(args.built_root, args.output, args.helper_manifest_sha256)
    common.reject_non_guest()
    require(not common.PG.exists() and not common.PUBLIC_ROOT.exists(), 'owned PG/runtime root already exists')
    for port in (PG_PORT, PANEL_PORT, GATE_PORT):
        with socket.socket() as check:
            check.bind(('127.0.0.1', port))
    common.write('SOURCE.json', build['source'])
    common.write('before-runtime.json', {**common.snapshot(), 'kernel_cursor': common.kernel_cursor()})
    common.ownership(units=[])
    result = {'ok': False, 'source_commit': args.source_commit, 'test_only': True, 'steps': {}, 'errors': [], 'controller_identity': controller_identity}
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
        common.start('pg', ['/usr/lib/postgresql/15/bin/postgres', '-D', str(common.PG), '-p', str(PG_PORT), '-h', '127.0.0.1', '-k', str(common.PG), '-c', 'shared_buffers=32MB', '-c', 'max_connections=20', '-c', 'max_wal_size=128MB'], '192M', extra=['--property=User=postgres', '--property=Group=postgres', '--property=LimitNOFILE=256'])
        common.wait_for(lambda: subprocess.run(['/usr/lib/postgresql/15/bin/pg_isready', '-h', '127.0.0.1', '-p', str(PG_PORT), '-U', 'postgres'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=2).returncode == 0, timeout=20)
        common.command(['runuser', '-u', 'postgres', '--', '/usr/lib/postgresql/15/bin/createdb', '-h', str(common.PG), '-p', str(PG_PORT), common.DATABASE])
        password = secrets.token_urlsafe(32)
        (args.output / 'admin-password').write_text(password)
        env = {'APSP_DATABASE_URL': f'postgres://postgres@127.0.0.1:{PG_PORT}/{common.DATABASE}', 'APSP_PUBLIC_URL': common.ORIGIN, 'APSP_LISTEN': f'127.0.0.1:{PANEL_PORT}', 'APSP_DATA_DIR': str(args.output / 'panel-data'), 'APSP_ADMIN_PASSWORD': password, 'RUST_LOG': 'info', 'PATH': '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin'}
        (args.output / 'panel.env').write_text(''.join(key + '=' + value + '\n' for key, value in env.items()))
        common.start('panel', [str(args.built_root / 'bin/apsp-panel')], '256M', extra=['--property=EnvironmentFile=' + str(args.output / 'panel.env'), '--property=LimitNOFILE=256'])
        common.wait_for(lambda: common.port_open(PANEL_PORT), timeout=60)
        with deferred_controller_signals():
            gate = Gate()
        api = common.API()
        api.login()
        server = api.expect('POST', '/api/servers', {'name': 'TEST_ONLY probe lease loopback Debian', 'agent_settings': {'sample_interval_secs': 1, 'upload_interval_secs': 3, 'auto_update': False, 'discover_public_ips': False}}, status=201)
        sid = server['id']
        common.write('identity-meta.json', {'server_id': sid, 'source_commit': args.source_commit})
        slow = SlowTargets()
        specs = []
        for index, port in enumerate(slow.ports):
            spec = {'id': str(uuid.UUID(int=0)), 'name': 'TEST_ONLY slow loopback ' + str(index), 'kind': 'tcp', 'target': '127.0.0.1', 'port': port, 'interval_secs': 10, 'carrier': '', 'enabled': True, 'monitor': {'network': ['telecom', 'unicom', 'mobile', 'other'][index], 'region': 'TEST_ONLY owned loopback', 'address_family': 'ipv4', 'authorization': {'kind': 'owned', 'enabled': True, 'source': 'TEST_ONLY controller owns these listener sockets', 'scope': f'TEST_ONLY server {sid}, TCP to 127.0.0.1:{port}, four attempts every ten seconds', 'expires_at': None, 'identity': {'kind': 'tcp', 'target': '127.0.0.1', 'port': port, 'address_family': 'ipv4'}}}}
            specs.append(api.expect('POST', f'/api/servers/{sid}/probes', spec))
        state_dir.mkdir(mode=0o700)
        with deferred_controller_signals():
            common.command(['mount', '-t', 'tmpfs', '-o', 'size=16m,mode=0700,nodev,nosuid,noexec', 'apsp-probe-lease-test-only', str(state_dir)])
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
        common.start('runtime', [str(runtime), 'run', '-c', str(runtime_config)], '96M', weight=200, oom=-500, extra=['--property=User=nobody', '--property=LimitNOFILE=256'])
        common.start('agent', [str(args.built_root / 'bin/apsp-agent'), '--config', str(args.output / 'agent.toml'), 'run', '--monitor-only'], '128M', weight=200, oom=-500, extra=['--property=LimitNOFILE=256'])
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
                pid = common.fingerprint(common.unit(role))['pid']
                uid = 0 if role == 'agent' else pwd.getpwnam('nobody').pw_uid
                status = Path(f'/proc/{pid}/status').read_text()
                actual_uids = re.search(r'^Uid:\s+(\d+)\s+(\d+)\s+(\d+)\s+(\d+)\s*$', status, re.MULTILINE)
                require(actual_uids and all(int(value) == uid for value in actual_uids.groups()),
                        'private actor UID changed')
                require(len(list(Path(f'/proc/{pid}/fd').iterdir())) <= MAX_ACTOR_FDS,
                        'private actor FD budget exceeded')
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

        gate.probe_pid, gate.probe_ports = agent_pid, slow.ports
        phase_start('cold_start_without_permission')
        window(7)
        require(gate.lease_faults and not gate.leases, 'cold-start lease GET was not actually denied')
        require(not gate.attempts and not state_rows(db, 'SELECT id FROM probe_outbox')
                and all(sum(row['connects'].values()) == 0 for row in observations),
                'cold-start Agent measured without a fresh execution permission')
        gate.lease_mode = 'allow'
        common.wait_for(lambda: latest_lease(gate), timeout=40)
        first_lease = latest_lease(gate)
        require(first_lease['server_id'] == sid and old_cache_is_inert(db),
                'fresh permission did not bind device or old cache remained executable')
        result['steps'][phase] = {'get_denied': True, 'measurements_without_permission': 0,
                                 'actual_first_lease': first_lease}
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
        previous_lease = latest_lease(gate)
        gate.lease_mode = 'deny'
        with closing(sqlite3.connect(db, timeout=2)) as connection:
            legacy_specs = [{key: item[key] for key in ('id','name','kind','target','port','interval_secs','carrier','enabled','monitor')} for item in specs]
            connection.execute("INSERT INTO kv(key,value) VALUES('probes:configuration',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                               (json.dumps([int(time.time()) + 86400, legacy_specs]),))
            connection.commit()
        restarted_at = time.time()
        common.command(['systemctl', 'restart', common.unit('agent')], timeout=30)
        agent_pid = common.wait_for(lambda: (value if (value := common.fingerprint(common.unit('agent'))['pid']) != before_pid else None), timeout=20)
        gate.probe_pid = agent_pid
        window(7)
        require(old_cache_is_inert(db), 'restarted Agent did not retire its legacy permission cache')
        require(set(row[0] for row in state_rows(db, 'SELECT id FROM probe_outbox')) == set(pending)
                and sum(active_connects(agent_pid, slow.ports).values()) == 0,
                'restarted Agent executed while a fresh GET was refused')
        gate.lease_mode = 'allow'
        common.wait_for(lambda: (value := latest_lease(gate)) and value['at'] > restarted_at
                        and value['id'] != previous_lease['id'], timeout=40)
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
        previous_lease = latest_lease(gate)
        require(previous_lease and previous_lease['expires_at'] - previous_lease['issued_at'] <= 90,
                'actual short issuance is missing')
        common.wait_for(lambda: sum(active_connects(agent_pid, slow.ports).values()) > 0, timeout=15)
        active_before = sum(active_connects(agent_pid, slow.ports).values())
        gate.disconnect()
        disconnected = time.time()
        common.wait_for(lambda: sum(active_connects(agent_pid, slow.ports).values()) == 0, timeout=2)
        window(3, connected=False)
        count = len(state_rows(db, 'SELECT id FROM probe_outbox'))
        window(93, connected=False)
        require(sum(active_connects(agent_pid, slow.ports).values()) == 0
                and len(state_rows(db, 'SELECT id FROM probe_outbox')) == count,
                'disconnected execution or result creation continued')
        gate.available = True
        disconnected_seconds = time.time() - disconnected
        common.wait_for(lambda: api.server()['last_heartbeat_at'] > disconnected, timeout=75)
        common.wait_for(lambda: (value := latest_lease(gate)) and value['at'] > disconnected
                        and value['id'] != previous_lease['id'], timeout=40)
        window(42)
        result['steps'][phase] = {'observed_disconnect_seconds': disconnected_seconds,
                                 'actual_previous_lease': previous_lease,
                                 'active_at_disconnect': active_before, 'cancelled_within_seconds': 2,
                                 'active_after_expiry': 0, 'outbox_stable_after_disconnect': True,
                                 'new_session_requires_new_lease': True}
        phase_start('lease_get_failure_revokes_permission')
        faults = []
        for mode in ('401', '403', 'hang'):
            gate.lease_mode = mode
            started = time.time()
            common.wait_for(lambda: any(item['at'] >= started and item['mode'] == mode for item in gate.lease_faults), timeout=40)
            fault = next(item for item in gate.lease_faults if item['at'] >= started and item['mode'] == mode)
            # The real Agent GET has a five-second budget; wait beyond it while
            # retaining actual heartbeat and independent proxy traffic observations.
            window(8)
            require(sum(active_connects(agent_pid, slow.ports).values()) == 0,
                    'failed or hung permission GET retained active measurement')
            pending_ids = set(row[0] for row in state_rows(db, 'SELECT id FROM probe_outbox'))
            attempted = {row['result']['id'] for row in gate.attempts}
            window(12)
            require({row['result']['id'] for row in gate.attempts}.issubset(attempted)
                    and set(row[0] for row in state_rows(db, 'SELECT id FROM probe_outbox')).issubset(pending_ids),
                    'permission GET failure kept creating samples')
            faults.append({'mode': mode, 'real_get_fault': fault, 'active_after_get_budget': 0})
            gate.lease_mode = 'allow'
            recovered_at = time.time()
            common.wait_for(lambda: (value := latest_lease(gate)) and value['at'] > recovered_at, timeout=40)
            window(12)
        result['steps'][phase] = {'faults': faults, 'heartbeat_and_proxy_kept_running': True}
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
        phase_start('native_icmp_ipv4_ipv6')
        icmp_specs = []
        for target, family in (('127.0.0.1', 'ipv4'), ('::1', 'ipv6')):
            item = {'id': str(uuid.UUID(int=0)), 'name': 'TEST_ONLY native ' + family,
                    'kind':'icmp', 'target':target, 'port':None, 'interval_secs':10,
                    'carrier':'', 'enabled':True, 'monitor':{'network':'other',
                    'region':'TEST_ONLY owned loopback', 'address_family':family,
                    'authorization':{'kind':'owned', 'enabled':True,
                    'source':'TEST_ONLY dedicated Guest owner', 'scope':'ICMP loopback only, four echoes per ten seconds',
                    'expires_at':None, 'identity':{'kind':'icmp','target':target,'port':None,'address_family':family}}}}
            icmp_specs.append(api.expect('POST', f'/api/servers/{sid}/probes', item))
        def actual_icmp():
            values = {}
            for item in gate.attempts:
                sample = item['result']
                if sample['probe_id'] in {probe['id'] for probe in icmp_specs} and sample.get('error') is None and sample.get('attempts') == 4 and sample.get('loss_percent') == 0 and sample.get('execution'):
                    values[sample['probe_id']] = sample
            return values if len(values) == 2 else None
        native_samples = common.wait_for(actual_icmp, timeout=45)
        require({item['address_family'] for item in native_samples.values()} == {'ipv4','ipv6'},
                'actual ICMP family identity changed')
        result['steps'][phase] = {'actual_current_agent_samples': list(native_samples.values()),
                                 'standard_agent_icmp_ipv4_ipv6': True}
        for item in icmp_specs:
            api.expect('DELETE', f'/api/servers/{sid}/probes/{item["id"]}', {'revision':item['revision']})
        phase_start('revoke_pause_delete_late_results')
        gate.upload = False
        common.wait_for(lambda: any(json.loads(row[0])['probe_id'] == specs[0]['id'] for row in state_rows(db, 'SELECT result FROM probe_outbox')), timeout=45)
        late = {row[0]: json.loads(row[1]) for row in state_rows(db, 'SELECT id,result FROM probe_outbox') if json.loads(row[1])['probe_id'] == specs[0]['id']}
        require(late, 'real removed-target late outbox is missing')
        api.expect('DELETE', f'/api/servers/{sid}/probes/{specs[0]["id"]}', {'revision':specs[0]['revision']})
        specs[1]['enabled'] = False
        specs[1]['monitor']['authorization']['enabled'] = False
        api.expect('PATCH', f'/api/servers/{sid}/probes/{specs[1]["id"]}', specs[1])
        for spec in specs[2:]:
            spec['enabled'] = False
            api.expect('PATCH', f'/api/servers/{sid}/probes/{spec["id"]}', spec)
        revoked_at = time.time()
        common.wait_for(lambda: (value := latest_lease(gate)) and value['at'] > revoked_at and not value['probe_ids'], timeout=40)
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
        if isinstance(error, ControlledFailure):
            result['controlled_reason'] = str(error)
    finally:
        signal.setitimer(signal.ITIMER_REAL, CLEANUP_SECONDS)
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
                bounded_backup(state_dir / 'state.db', args.output / 'retained-private-state.db')
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
        result['actual_lease_issuances'] = gate.leases if gate else []
        result['actual_lease_get_faults'] = gate.lease_faults if gate else []
        result['controller_derivation'] = {'base':'tools/carrier-monitoring-acceptance.py', 'base_sha256':'6454437f24ad034281b813f5c67683f91b7d8fd6f4fd55abeae6426d61efb1d4', 'scope':'fresh frozen ninety-second permission with bounded private controller IO; original helper bytes remain digest-checked', 'echo_helper_sha256':'f580cb3b1524485aab4f134e2cf7df6f4ca64f52d7dced8840bd1780c668ab56'}
        result['controller_budgets'] = {'overall_seconds': CONTROLLER_SECONDS, 'cleanup_seconds': CLEANUP_SECONDS,
                                        'controller_fds': MAX_CONTROLLER_FDS, 'actor_fds': MAX_ACTOR_FDS,
                                        'concurrent_gate_requests': MAX_GATE_REQUESTS, 'gate_events': MAX_GATE_EVENTS}
        common.write('probe-lease-result.json', result)
        print(json.dumps({'ok': result['ok'], 'failed_phase': result.get('failed_phase'), 'errors': result['errors'], 'proxy_echoes': len(traffic), 'proxy_failures': sum(not row['ok'] for row in traffic)}), flush=True)
    require(result['ok'], 'probe lease acceptance or safe cleanup failed; preserve the first receipt')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--echo-worker', type=int)
    parser.add_argument('--built-root', type=Path)
    parser.add_argument('--helper-manifest-sha256', help='Explicit trusted SHA256 of the separately reviewed private helper manifest')
    parser.add_argument('--source-commit')
    parser.add_argument('--controller-source-commit', help='Exact commit containing a separately frozen controller; never relabels binary source')
    parser.add_argument('--controller-sha256', help='Explicit trusted SHA256 of that independently frozen controller')
    parser.add_argument('--runtime', type=Path)
    parser.add_argument('--runtime-receipt', type=Path)
    parser.add_argument('--output', type=Path)
    arguments = parser.parse_args()
    if arguments.echo_worker is not None:
        require(__debug__ and 0 < arguments.echo_worker <= 65535, 'bounded loopback echo worker required')
        echo(arguments.echo_worker)
    else:
        require(all(getattr(arguments, name) is not None for name in ('built_root', 'helper_manifest_sha256', 'source_commit', 'runtime', 'runtime_receipt', 'output')), 'all frozen inputs must be explicit')
        with finite_controller():
            main(arguments)
