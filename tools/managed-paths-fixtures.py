#!/usr/bin/env python3
"""Bounded TEST_ONLY targets, clients and passive managed-path observations.

This helper never installs or manages an Agent or an A/M/B runtime. Product
configuration and subscription bytes belong to the panel publisher/compiler.
The controller supplies the isolated guests and the enclosing resource cgroup.
"""
import argparse
import contextlib
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import selectors
import signal
import socket
import socketserver
import ssl
import struct
import subprocess
import sys
import threading
import time
import uuid

from managed_paths_support import OwnedProcess

MAX_FILE = 2 * 1024 * 1024
MAX_OUTPUT = 256 * 1024
MAX_ROWS = 4096
MAX_OBSERVATIONS = 128
MAX_DURATION = 1800
DEADLINE = None
STDOUT_BYTES = 0
PORT_NAMES = {"x", "handshake", "tcp_echo", "udp_echo", "https"}
ADDRESS_NAMES = {"fixture", "A", "M", "B", "client"}
PRODUCT_ROLES = {"A", "M", "B"}


class Rejected(Exception):
    pass


def require(condition, code):
    if not condition:
        raise Rejected(code)


def remaining(maximum=3):
    value = min(maximum, DEADLINE - time.monotonic())
    require(value > 0, "deadline_exceeded")
    return value


def integer(value, lower, upper):
    return type(value) is int and lower <= value <= upper


def emit(value):
    global STDOUT_BYTES
    data = json.dumps(value, sort_keys=True, separators=(",", ":"))
    size = len(data.encode()) + 1
    limit = MAX_OUTPUT if value.get("kind") == "failure" else MAX_OUTPUT - 1024
    require(STDOUT_BYTES + size <= limit, "stdout_budget_exceeded")
    STDOUT_BYTES += size
    print(data, flush=True)


def absolute_path(value):
    require(isinstance(value, str), "absolute_path_required")
    path = Path(value)
    require(path.is_absolute() and ".." not in path.parts, "absolute_path_required")
    require(not any(item.is_symlink() for item in (path, *path.parents)), "symlink_path_rejected")
    return path


def regular_file(value, budget=MAX_FILE):
    path = absolute_path(str(value))
    require(path.is_file() and path.stat().st_size <= budget, "bounded_regular_file_required")
    return path


def read_json(value):
    with regular_file(value).open("rb") as source:
        return json.load(source)


def digest_file(value, budget=MAX_FILE):
    path = regular_file(value, budget)
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(65536), b""):
            remaining()
            digest.update(block)
    return digest.hexdigest()


def private_bytes(path, data):
    require(len(data) <= MAX_FILE, "file_budget_exceeded")
    path = absolute_path(str(path))
    with path.open("xb") as target:
        os.chmod(path, 0o600)
        target.write(data)


def private_json(path, value):
    private_bytes(path, (json.dumps(value, sort_keys=True, indent=2) + "\n").encode())


def private_address(value):
    require(isinstance(value, str), "literal_private_ipv4_required")
    address = ipaddress.ip_address(value)
    require(address.version == 4 and (address.is_private or address.is_loopback)
            and not address.is_unspecified and not address.is_multicast,
            "literal_private_ipv4_required")
    return str(address)


def validate_plan(value):
    require(isinstance(value, dict) and set(value) == {
        "schema", "test_only", "run_id", "native_binary", "native_sha256", "addresses", "ports", "managed_ports"
    }, "invalid_plan_schema")
    require(type(value["schema"]) is int and value["schema"] == 1 and value["test_only"] is True,
            "test_only_required")
    require(isinstance(value["run_id"], str) and str(uuid.UUID(value["run_id"])) == value["run_id"]
            and uuid.UUID(value["run_id"]).int != 0, "invalid_run_identity")
    require(isinstance(value["addresses"], dict) and set(value["addresses"]) == ADDRESS_NAMES,
            "role_addresses_required")
    for address in value["addresses"].values():
        private_address(address)
    require(len({value["addresses"][role] for role in PRODUCT_ROLES}) == 3,
            "independent_managed_addresses_required")
    require(value["addresses"]["fixture"] not in {value["addresses"][role] for role in PRODUCT_ROLES},
            "independent_fixture_address_required")
    require(isinstance(value["ports"], dict) and set(value["ports"]) == PORT_NAMES,
            "fixture_ports_required")
    require(all(integer(port, 1024, 65535) for port in value["ports"].values())
            and len(set(value["ports"].values())) == len(PORT_NAMES)
            and not {18085, 18086, 2080} & set(value["ports"].values()), "invalid_fixture_ports")
    require(isinstance(value["managed_ports"], dict) and set(value["managed_ports"]) == PRODUCT_ROLES,
            "managed_ports_required")
    for ports in value["managed_ports"].values():
        ports = [ports] if type(ports) is int else ports
        require(isinstance(ports, list) and 1 <= len(ports) <= 8
                and all(integer(port, 1024, 65535) for port in ports)
                and len(set(ports)) == len(ports) and not {18085, 18086} & set(ports),
                "managed_ports_required")
    require(isinstance(value["native_sha256"], str) and len(value["native_sha256"]) == 64
            and all(char in "0123456789abcdef" for char in value["native_sha256"]),
            "native_sha256_required")
    absolute_path(value["native_binary"])
    return value


def clean_environment(manifest=None):
    result = os.environ.copy()
    for name in list(result):
        if name.lower() in {"http_proxy", "https_proxy", "all_proxy", "no_proxy"}:
            del result[name]
    if manifest:
        result["SSL_CERT_FILE"] = manifest["tls"]["ca"]
        result["SSL_CERT_DIR"] = manifest["tls"]["empty_ca_directory"]
    return result


class Child:
    """Only a freshly created helper-owned process group may be terminated."""
    def __init__(self, command, env, log, pass_fds=()):
        self.log = absolute_path(str(log)).open("xb")
        os.chmod(log, 0o600)
        self.output, self.overflow = bytearray(), threading.Event()
        self.reader_error = False
        try:
            self.process = OwnedProcess(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                            stderr=subprocess.STDOUT, env=env, start_new_session=True,
                                            pass_fds=pass_fds)
        except BaseException:
            self.log.close()
            raise
        self.reader = threading.Thread(target=self._drain, daemon=True)
        self.reader.start()

    def _drain(self):
        try:
            for block in iter(lambda: self.process.stdout.read(4096), b""):
                allowed = max(0, MAX_OUTPUT - len(self.output))
                self.output.extend(block[:allowed])
                self.log.write(block[:allowed])
                if len(block) > allowed:
                    self.overflow.set()
                    break
        except Exception:
            self.reader_error = True
        finally:
            try:
                self.log.flush()
            except Exception:
                self.reader_error = True

    def healthy(self):
        require(not self.overflow.is_set(), "child_output_budget_exceeded")
        require(not self.reader_error, "child_output_reader_failed")
        require(self.process.poll() is None, "helper_child_exited")

    def stop(self):
        self.process.stop_group()
        self.reader.join(timeout=1)
        require(not self.reader.is_alive(), "child_reader_cleanup_timeout")
        self.process.stdout.close()
        self.log.close()
        require(not self.overflow.is_set(), "child_output_budget_exceeded")
        require(not self.reader_error, "child_output_reader_failed")


def capture(command, directory, name, timeout=20):
    child = Child(command, clean_environment(), directory / name)
    try:
        try:
            child.process.wait(timeout=remaining(timeout))
        except subprocess.TimeoutExpired:
            raise Rejected("controlled_command_timeout") from None
        child.reader.join(timeout=1)
        require(child.process.returncode == 0, "controlled_command_failed")
        return bytes(child.output)
    finally:
        child.stop()


def verify_native(manifest, directory):
    binary = regular_file(manifest["native_binary"], 512 * 1024 * 1024)
    require(digest_file(binary, 512 * 1024 * 1024) == manifest["native_sha256"], "native_binary_hash_mismatch")
    output = capture([str(binary), "version"], directory, "native-version.private.log").decode("utf-8")
    require("sing-box version 1.14.2" in [line.strip() for line in output.splitlines()],
            "exact_native_version_required")


def prepare(args):
    plan = validate_plan(read_json(args.plan))
    root = absolute_path(args.work_dir)
    require(not root.exists(), "new_private_directory_required")
    space = os.statvfs(root.parent)
    require(space.f_bavail * space.f_frsize >= 64 * 1024 * 1024 and space.f_favail >= 64,
            "insufficient_fixture_disk_or_inodes")
    root.mkdir(mode=0o700)
    verify_native(plan, root)
    tls = root / "tls"
    tls.mkdir(mode=0o700)
    (tls / "empty-ca-directory").mkdir(mode=0o700)
    private_bytes(tls / "extensions.cnf", (
        "subjectAltName=DNS:reality.test,DNS:target.test,IP:" + plan["addresses"]["fixture"]
        + "\nextendedKeyUsage=serverAuth\n").encode())
    capture(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-sha256", "-days", "1",
             "-subj", "/CN=Apsp TEST ONLY fixture CA", "-keyout", str(tls / "ca.key"),
             "-out", str(tls / "ca.crt")], root, "ca.private.log")
    capture(["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=target.test",
             "-keyout", str(tls / "server.key"), "-out", str(tls / "server.csr")], root, "csr.private.log")
    capture(["openssl", "x509", "-req", "-in", str(tls / "server.csr"), "-CA", str(tls / "ca.crt"),
             "-CAkey", str(tls / "ca.key"), "-CAcreateserial", "-out", str(tls / "server.crt"),
             "-days", "1", "-sha256", "-extfile", str(tls / "extensions.cnf")], root, "cert.private.log")
    accounts, sources = {}, {}
    for version in ("v1", "v2", "bad", "v3"):
        account = {"username": "TEST_ONLY_X_" + version,
                   "password": "TEST_ONLY_" + os.urandom(24).hex()}
        accounts[version] = account
        source = root / ("source-" + version + ".json")
        private_json(source, {"outbounds": [{"type": "http", "tag": "TEST_ONLY_X",
            "server": plan["addresses"]["fixture"], "server_port": plan["ports"]["x"], **account}]})
        sources[version] = {"path": str(source), "sha256": digest_file(source)}
    manifest = {**plan, "root": str(root), "accounts": accounts,
        "source_contents": sources, "tls": {"ca": str(tls / "ca.crt"), "cert": str(tls / "server.crt"),
            "key": str(tls / "server.key"), "empty_ca_directory": str(tls / "empty-ca-directory")}}
    private_json(root / "manifest.json", manifest)
    emit({"kind": "prepared", "event": "prepared", "test_only": True, "run_id": manifest["run_id"],
          "manifest": str(root / "manifest.json"), "manifest_sha256": digest_file(root / "manifest.json")})


def load_manifest(value):
    manifest = read_json(value)
    require(isinstance(manifest, dict) and set(manifest) == {
        "schema", "test_only", "native_binary", "native_sha256", "addresses", "ports", "managed_ports",
        "run_id", "root", "accounts", "source_contents", "tls"}, "invalid_manifest_schema")
    validate_plan({key: manifest[key] for key in (
        "schema", "test_only", "run_id", "native_binary", "native_sha256", "addresses", "ports", "managed_ports")})
    root = absolute_path(manifest["root"])
    require(root.is_dir() and root.stat().st_mode & 0o077 == 0, "private_fixture_directory_required")
    require(set(manifest["accounts"]) == {"v1", "v2", "bad", "v3"}
            and set(manifest["source_contents"]) == set(manifest["accounts"]), "source_versions_required")
    for version, account in manifest["accounts"].items():
        require(isinstance(account, dict) and set(account) == {"username", "password"}
                and all(isinstance(item, str) and item.startswith("TEST_ONLY_") and len(item) <= 128
                        for item in account.values()), "test_only_accounts_required")
        record = manifest["source_contents"][version]
        require(set(record) == {"path", "sha256"} and absolute_path(record["path"]).parent == root
                and digest_file(record["path"]) == record["sha256"], "prepared_source_changed")
    require(set(manifest["tls"]) == {"ca", "cert", "key", "empty_ca_directory"}, "prepared_tls_required")
    for name, path in manifest["tls"].items():
        require(absolute_path(path).parent == root / "tls", "tls_path_escape")
        if name != "empty_ca_directory":
            regular_file(path)
    return manifest


def process_identity(pid):
    require(integer(pid, 1, 2**31 - 1), "invalid_process_id")
    text = (Path("/proc") / str(pid) / "stat").read_text()
    fields = text[text.rfind(")") + 2:].split()
    require(len(fields) >= 20, "invalid_process_stat")
    return {"pid": pid, "starttime": int(fields[19]),
            "netns_inode": os.stat(Path("/proc") / str(pid) / "ns/net").st_ino}


def validate_identity(identity):
    require(isinstance(identity, dict) and set(identity) == {"pid", "starttime", "netns_inode"}
            and all(integer(value, 1, 2**63 - 1) for value in identity.values()), "invalid_process_identity")
    try:
        actual = process_identity(identity["pid"])
    except (OSError, ValueError):
        raise Rejected("process_identity_disappeared") from None
    require(actual == identity, "process_identity_changed")


def identities_file(value, manifest):
    identities = read_json(value)
    require(isinstance(identities, dict) and set(identities) == {"schema", "run_id", "roles"}
            and identities["schema"] == 1 and identities["run_id"] == manifest["run_id"],
            "identity_run_binding_mismatch")
    require(isinstance(identities["roles"], dict) and bool(identities["roles"])
            and set(identities["roles"]) <= PRODUCT_ROLES | {"X", "client"}, "invalid_identity_roles")
    for identity in identities["roles"].values():
        validate_identity(identity)
    require(len({item["pid"] for item in identities["roles"].values()}) == len(identities["roles"]),
            "duplicate_role_process")
    managed = [identity for role, identity in identities["roles"].items() if role in PRODUCT_ROLES]
    require(len({item["netns_inode"] for item in managed}) == len(managed),
            "independent_managed_namespaces_required")
    return identities


def reserve(address, port):
    handles = []
    try:
        for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
            item = socket.socket(socket.AF_INET, kind)
            handles.append(item)
            if kind == socket.SOCK_STREAM:
                item.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            item.bind((address, port))
        return handles
    except OSError:
        for item in handles:
            item.close()
        raise Rejected("port_conflict_no_process_stopped") from None


def wait_port(address, port, child):
    end = time.monotonic() + remaining(5)
    while time.monotonic() < end:
        child.healthy()
        try:
            with socket.create_connection((address, port), timeout=0.1):
                return
        except OSError:
            time.sleep(0.03)
    raise Rejected("helper_child_readiness_timeout")


class BoundedTCP(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True
    block_on_close = False
    gate = threading.BoundedSemaphore(32)

    def __init__(self, *args, **kwargs):
        self.active, self.lock = set(), threading.Condition()
        super().__init__(*args, **kwargs)

    def get_request(self):
        request, address = super().get_request()
        request.settimeout(3)
        return request, address

    def process_request(self, request, address):
        if not self.gate.acquire(blocking=False):
            request.close()
            return
        with self.lock:
            self.active.add(request)
        try:
            super().process_request(request, address)
        except BaseException:
            with self.lock:
                self.active.discard(request)
                self.lock.notify_all()
            self.gate.release()
            request.close()
            raise

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            with self.lock:
                self.active.discard(request)
                self.lock.notify_all()
            self.gate.release()

    def server_close(self):
        with self.lock:
            for request in self.active:
                with contextlib.suppress(OSError):
                    request.shutdown(socket.SHUT_RDWR)
                request.close()
        super().server_close()
        end = time.monotonic() + 4
        with self.lock:
            while self.active and time.monotonic() < end:
                self.lock.wait(timeout=max(0, end - time.monotonic()))
            require(not self.active, "target_handler_cleanup_timeout")

    def handle_error(self, request, address):
        self.fixture.errors.set()


class QuietUDP(socketserver.UDPServer):
    max_packet_size = 513

    def handle_error(self, request, address):
        self.fixture.errors.set()


def exact(connection, length):
    require(integer(length, 0, 65536), "socket_read_budget_exceeded")
    data = bytearray()
    while len(data) < length:
        connection.settimeout(remaining())
        part = connection.recv(length - len(data))
        require(bool(part), "unexpected_socket_eof")
        data.extend(part)
    return bytes(data)


class Journal:
    def __init__(self, path):
        self.path, self.lock, self.sequence, self.size = path, threading.Lock(), 0, 0
        self.file = path.open("xb")
        os.chmod(path, 0o600)
        self.started = time.monotonic()

    def append(self, value):
        with self.lock:
            value = {"seq": self.sequence + 1, "elapsed_ms": int((time.monotonic() - self.started) * 1000), **value}
            data = (json.dumps(value, sort_keys=True) + "\n").encode()
            require(self.size + len(data) <= MAX_FILE, "target_journal_budget_exceeded")
            self.file.write(data)
            self.file.flush()
            self.sequence += 1
            self.size += len(data)

    def snapshot(self, after):
        require(integer(after, 0, 2**63 - 1), "invalid_snapshot_cursor")
        with self.lock:
            require(after <= self.sequence, "snapshot_cursor_ahead")
            events = []
            with self.path.open("rb") as source:
                for line in source:
                    value = json.loads(line)
                    if value["seq"] > after:
                        events.append(value)
                        if len(events) == MAX_OBSERVATIONS:
                            break
            next_cursor = events[-1]["seq"] if events else after
            return {"events": events, "next": next_cursor, "has_more": next_cursor < self.sequence,
                    "total": self.sequence}

    def close(self):
        self.file.close()


class Targets:
    def __init__(self, manifest, directory):
        self.manifest, self.directory = manifest, directory
        self.errors, self.servers, self.threads = threading.Event(), [], []
        self.journal, self.x, self.x_generation = Journal(directory / "targets.private.jsonl"), None, 0

    def record(self, kind, peer, size=0):
        roles = [role for role in PRODUCT_ROLES if self.manifest["addresses"][role] == peer[0]]
        try:
            self.journal.append({"kind": kind, "peer": {"address": peer[0], "port": peer[1]},
                                 "peer_role": roles[0] if len(roles) == 1 else None, "bytes": size})
        except BaseException:
            self.errors.set()
            raise

    def tls(self, handshake=False):
        result = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        result.load_cert_chain(self.manifest["tls"]["cert"], self.manifest["tls"]["key"])
        result.minimum_version = result.maximum_version = ssl.TLSVersion.TLSv1_3
        result.set_alpn_protocols(["h2", "http/1.1"] if handshake else ["http/1.1"])
        return result

    def start(self):
        fixture = self
        class Handshake(socketserver.BaseRequestHandler):
            def handle(self):
                with contextlib.suppress(OSError, ssl.SSLError):
                    with fixture.tls(True).wrap_socket(self.request, server_side=True) as secure:
                        secure.settimeout(2)
                        secure.recv(4096)
        class Echo(socketserver.BaseRequestHandler):
            def handle(self):
                with contextlib.suppress(OSError, Rejected):
                    payload = exact(self.request, 4096)
                    fixture.record("tcp", self.client_address, len(payload))
                    self.request.sendall(payload)
                    time.sleep(0.1)
        class HTTPS(socketserver.BaseRequestHandler):
            def handle(self):
                with contextlib.suppress(OSError, ssl.SSLError):
                    with fixture.tls().wrap_socket(self.request, server_side=True) as secure:
                        secure.settimeout(3)
                        data = bytearray()
                        while b"\r\n\r\n" not in data:
                            chunk = secure.recv(1024)
                            require(bool(chunk), "unexpected_socket_eof")
                            data.extend(chunk)
                            require(len(data) <= 8192, "https_header_budget_exceeded")
                        require(bytes(data).split(b"\r\n", 1)[0] == b"HEAD /health HTTP/1.1", "unexpected_fixture_https_path")
                        fixture.record("https", self.client_address)
                        secure.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        class UDP(socketserver.BaseRequestHandler):
            def handle(self):
                payload, sender = self.request
                if len(payload) <= 512:
                    fixture.record("udp", self.client_address, len(payload))
                    sender.sendto(payload, self.client_address)
        for name, handler, udp in (("handshake", Handshake, False), ("tcp_echo", Echo, False),
                                   ("https", HTTPS, False), ("udp_echo", UDP, True)):
            cls = QuietUDP if udp else BoundedTCP
            server = cls((self.manifest["addresses"]["fixture"], self.manifest["ports"][name]), handler)
            server.fixture = self
            self.servers.append(server)
            thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
            self.threads.append(thread)
            thread.start()
        self.start_x()

    def start_x(self):
        require(self.x is None, "x_already_running")
        require(self.x_generation < 64, "x_restart_budget_exceeded")
        config = self.directory / ("X-" + str(self.x_generation) + ".json")
        private_json(config, {"log": {"level": "warn"}, "inbounds": [{"type": "http", "tag": "TEST_ONLY_X",
            "listen": self.manifest["addresses"]["fixture"], "listen_port": self.manifest["ports"]["x"],
            "users": [self.manifest["accounts"][version] for version in ("v1", "v2", "v3")]}],
            "outbounds": [{"type": "direct", "tag": "direct"}], "route": {"final": "direct"}})
        self.x = Child([self.manifest["native_binary"], "run", "-c", str(config)], clean_environment(self.manifest),
                       self.directory / ("X-" + str(self.x_generation) + ".private.log"))
        self.x_generation += 1
        wait_port(self.manifest["addresses"]["fixture"], self.manifest["ports"]["x"], self.x)

    def stop_x(self):
        require(self.x is not None, "x_not_running")
        self.x.stop()
        self.x = None

    def command(self, value):
        require(isinstance(value, dict) and "op" in value, "invalid_serve_command")
        op = value["op"]
        require((op == "snapshot" and set(value) <= {"op", "after"})
                or (op in {"stop_x", "start_x", "stop"} and set(value) == {"op"}), "invalid_serve_command")
        if op == "snapshot":
            return {"kind": "snapshot", "event": "snapshot", "x_running": self.x is not None,
                    "x_identity": process_identity(self.x.process.pid) if self.x else None,
                    **self.journal.snapshot(value.get("after", 0))}
        if op == "stop_x":
            self.stop_x()
        elif op == "start_x":
            self.start_x()
        return {"kind": {"stop_x": "x_stopped", "start_x": "x_started", "stop": "stopped"}[op],
                "event": op, "x_running": self.x is not None,
                "x_identity": process_identity(self.x.process.pid) if self.x else None}

    def close(self):
        errors = []
        if self.x:
            try:
                self.stop_x()
            except BaseException:
                errors.append("x")
        for server in self.servers:
            try:
                server.shutdown()
                server.server_close()
            except BaseException:
                errors.append("target")
        for thread in self.threads:
            thread.join(timeout=1)
            if thread.is_alive():
                errors.append("target_thread")
        self.journal.close()
        for port in self.manifest["ports"].values():
            try:
                for handle in reserve(self.manifest["addresses"]["fixture"], port):
                    handle.close()
            except BaseException:
                errors.append("listener")
        require(not errors, "owned_fixture_cleanup_not_confirmed")


def serve(args, manifest):
    directory = absolute_path(manifest["root"]) / ("serve-" + uuid.uuid4().hex)
    directory.mkdir(mode=0o700)
    verify_native(manifest, directory)
    fixture = Targets(manifest, directory)
    selector, data, reason = selectors.DefaultSelector(), bytearray(), "stdin_closed"
    try:
        fixture.start()
        emit({"kind": "ready", "event": "ready", "run_id": manifest["run_id"], "fixture_pid": os.getpid(),
              "x_identity": process_identity(fixture.x.process.pid), "journal": str(fixture.journal.path)})
        selector.register(sys.stdin.fileno(), selectors.EVENT_READ)
        done, commands = False, 0
        while not done and time.monotonic() < DEADLINE:
            require(not fixture.errors.is_set(), "target_handler_failed")
            if fixture.x:
                fixture.x.healthy()
            for key, _ in selector.select(timeout=min(0.1, remaining())):
                chunk = os.read(key.fd, 4096)
                if not chunk:
                    require(not data, "truncated_serve_command")
                    done = True
                    break
                data.extend(chunk)
                require(len(data) <= 32768, "serve_command_budget_exceeded")
                while b"\n" in data:
                    line, _, rest = data.partition(b"\n")
                    data = bytearray(rest)
                    value = json.loads(line)
                    commands += 1
                    require(commands <= MAX_ROWS, "serve_command_count_exceeded")
                    reply = fixture.command(value)
                    if value["op"] == "stop":
                        done, reason = True, "requested_stop"
                        break
                    emit(reply)
        if not done:
            reason = "deadline_expired"
    finally:
        selector.close()
        fixture.close()
    require(not fixture.errors.is_set(), "target_handler_failed")
    emit({"kind": "stopped", "event": "stopped", "reason": reason,
          "cleanup_confirmed": True, "run_id": manifest["run_id"]})


def socks(destination, port, command=1):
    connection = socket.create_connection(("127.0.0.1", 2080), timeout=remaining())
    try:
        connection.settimeout(remaining())
        connection.sendall(b"\x05\x01\x00")
        require(exact(connection, 2) == b"\x05\x00", "socks_authentication_rejected")
        connection.sendall(bytes([5, command, 0, 1]) + socket.inet_aton(destination) + struct.pack("!H", port))
        require(exact(connection, 3) == b"\x05\x00\x00", "socks_route_rejected")
        kind = exact(connection, 1)[0]
        require(kind in (1, 3, 4), "invalid_socks_address")
        if kind == 1:
            host = socket.inet_ntoa(exact(connection, 4))
        elif kind == 4:
            host = socket.inet_ntop(socket.AF_INET6, exact(connection, 16))
        else:
            host = exact(connection, exact(connection, 1)[0]).decode("ascii")
        return connection, (host, struct.unpack("!H", exact(connection, 2))[0])
    except BaseException:
        connection.close()
        raise


def traffic(manifest, sequence):
    host, ports = manifest["addresses"]["fixture"], manifest["ports"]
    result = {}
    def attempt(name, action):
        try:
            result[name] = {"ok": True, "code": "verified", **action()}
        except (OSError, ssl.SSLError, Rejected) as error:
            code = error.args[0] if isinstance(error, Rejected) else "network_failure"
            result[name] = {"ok": False, "code": code}
    def tcp():
        payload = ("TEST_ONLY_TCP_" + str(sequence)).encode().ljust(4096, b"t")
        connection, _ = socks(host, ports["tcp_echo"])
        with connection:
            connection.sendall(payload)
            require(exact(connection, len(payload)) == payload, "tcp_payload_differs")
        return {"bytes": len(payload)}
    def udp():
        control, relay = socks(host, 0, 3)
        try:
            relay_host = "127.0.0.1" if relay[0] in {"0.0.0.0", "::"} else relay[0]
            require(relay_host == "127.0.0.1" and relay[1] > 0, "nonlocal_socks_udp_relay")
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as connection:
                connection.settimeout(remaining())
                header = b"\x00\x00\x00\x01" + socket.inet_aton(host) + struct.pack("!H", ports["udp_echo"])
                for index in range(5):
                    payload = ("TEST_ONLY_UDP_" + str(sequence) + "_" + str(index)).encode().ljust(192, b"u")
                    connection.sendto(header + payload, (relay_host, relay[1]))
                    packet, peer = connection.recvfrom(1024)
                    require(peer == (relay_host, relay[1]) and packet == header + payload, "udp_payload_differs")
            return {"packets": 5, "bytes": 960}
        finally:
            control.close()
    def https():
        connection, _ = socks(host, ports["https"])
        context = ssl.create_default_context(cafile=manifest["tls"]["ca"])
        try:
            with context.wrap_socket(connection, server_hostname="target.test") as secure:
                secure.settimeout(remaining())
                secure.sendall(b"HEAD /health HTTP/1.1\r\nHost: target.test\r\nConnection: close\r\n\r\n")
                data = bytearray()
                while b"\r\n\r\n" not in data:
                    part = secure.recv(1024)
                    require(bool(part), "unexpected_socket_eof")
                    data.extend(part)
                    require(len(data) <= 8192, "https_response_budget_exceeded")
                require(bytes(data).split(b"\r\n", 1)[0] == b"HTTP/1.1 200 OK", "https_status_not_success")
            return {}
        finally:
            connection.close()
    for name, action in (("tcp", tcp), ("udp", udp), ("https", https)):
        attempt(name, action)
    return result


def subscription_file(value, expected):
    path = regular_file(value)
    require(digest_file(path) == expected, "subscription_bytes_hash_mismatch")
    config = read_json(path)
    require(isinstance(config, dict) and isinstance(config.get("inbounds"), list)
            and len(config["inbounds"]) == 1 and config["inbounds"][0].get("type") == "mixed"
            and config["inbounds"][0].get("listen") == "127.0.0.1"
            and config["inbounds"][0].get("listen_port") == 2080, "unchanged_product_client_listener_required")
    return path


def client(args, manifest):
    global DEADLINE
    config = subscription_file(args.config, args.config_sha256)
    require(integer(args.iterations, 1, 64) and integer(args.interval_ms, 0, 5000)
            and (args.duration_secs is None or integer(args.duration_secs, 1, 60)), "invalid_client_budget")
    require(isinstance(args.phase, str) and 1 <= len(args.phase) <= 64
            and all(char.isalnum() or char in "_-" for char in args.phase), "invalid_phase_label")
    directory = absolute_path(manifest["root"]) / ("client-" + uuid.uuid4().hex)
    directory.mkdir(mode=0o700)
    verify_native(manifest, directory)
    for handle in reserve("127.0.0.1", 2080):
        handle.close()
    child = Child([manifest["native_binary"], "run", "-c", str(config)], clean_environment(manifest),
                  directory / "client.private.log")
    events, started = [], time.monotonic()
    outer_deadline = DEADLINE
    try:
        wait_port("127.0.0.1", 2080, child)
        emit({"kind": "ready", "event": "client_ready", "run_id": manifest["run_id"], "phase": args.phase,
              "identity": process_identity(child.process.pid), "subscription_sha256": args.config_sha256})
        phase_end = min(DEADLINE, time.monotonic() + args.duration_secs) if args.duration_secs is not None else DEADLINE
        interval = args.interval_ms / 1000
        if args.duration_secs is not None:
            # Keep a sixty-second fault window alive without exceeding sixty
            # four rounds. Finish an in-flight bounded round before stopping.
            interval = max(interval, args.duration_secs / 64)
        rounds = 64 if args.duration_secs is not None else args.iterations
        for sequence in range(rounds):
            if time.monotonic() >= min(DEADLINE, phase_end):
                break
            child.healthy()
            event = {"round": sequence + 1, "elapsed_ms": int((time.monotonic() - started) * 1000),
                     **traffic(manifest, sequence)}
            events.append(event)
            emit({"kind": "traffic", "event": "client_round", "phase": args.phase, **event})
            if interval and time.monotonic() < min(DEADLINE, phase_end):
                time.sleep(min(interval, remaining(5), max(0, phase_end - time.monotonic())))
        DEADLINE = outer_deadline
        require(digest_file(config) == args.config_sha256, "subscription_changed_during_client_run")
    finally:
        DEADLINE = outer_deadline
        child.stop()
        for handle in reserve("127.0.0.1", 2080):
            handle.close()
    result = {"kind": "result", "event": "client_result", "run_id": manifest["run_id"], "phase": args.phase,
              "events": events, "all_passed": bool(events) and all(
                  all(event[protocol]["ok"] for protocol in ("tcp", "udp", "https")) for event in events),
              "cleanup_confirmed": True, "subscription_sha256": args.config_sha256}
    private_json(directory / "result.json", result)
    emit(result)


def net_rows(pid, protocol):
    rows = []
    for name in (protocol, protocol + "6"):
        path = Path("/proc") / str(pid) / "net" / name
        if not path.is_file():
            continue
        with path.open(encoding="ascii") as source:
            next(source)
            for line in source:
                require(len(rows) < MAX_ROWS, "socket_table_budget_exceeded")
                fields = line.split()
                def address(text):
                    host, port = text.split(":")
                    raw = bytes.fromhex(host)
                    if len(raw) == 4:
                        host = socket.inet_ntoa(raw[::-1])
                    else:
                        raw = b"".join(raw[index:index + 4][::-1] for index in range(0, 16, 4))
                        value = ipaddress.IPv6Address(raw)
                        host = str(value.ipv4_mapped or value)
                    return host, int(port, 16)
                rows.append((address(fields[1]), address(fields[2]), fields[9]))
    return rows


def process_sockets(identity):
    validate_identity(identity)
    files = list((Path("/proc") / str(identity["pid"]) / "fd").iterdir())
    require(len(files) <= 256, "process_descriptor_budget_exceeded")
    result = set()
    for file in files:
        try:
            target = os.readlink(file)
        except FileNotFoundError:
            continue
        if target.startswith("socket:["):
            result.add(target[8:-1])
    validate_identity(identity)
    return result


def endpoint_roles(manifest):
    result = {}
    for role in PRODUCT_ROLES:
        ports = manifest["managed_ports"][role]
        ports = [ports] if type(ports) is int else ports
        for port in ports:
            endpoint = (manifest["addresses"][role], port)
            require(endpoint not in result, "ambiguous_managed_endpoint")
            result[endpoint] = role
    external = (manifest["addresses"]["fixture"], manifest["ports"]["x"])
    require(external not in result, "ambiguous_external_endpoint")
    result[external] = "X"
    for name in ("tcp_echo", "udp_echo", "https"):
        endpoint = (manifest["addresses"]["fixture"], manifest["ports"][name])
        require(endpoint not in result, "ambiguous_target_endpoint")
        result[endpoint] = "target"
    return result


def observe_once(manifest, identities):
    destinations, edges = endpoint_roles(manifest), set()
    for role, identity in identities["roles"].items():
        owned = process_sockets(identity)
        for protocol in ("tcp", "udp"):
            for local, remote, inode in net_rows(identity["pid"], protocol):
                if inode in owned and remote in destinations:
                    edges.add((role, destinations[remote], protocol, local[0], local[1], remote[0], remote[1]))
        validate_identity(identity)
    return sorted(edges)


def observe(args, manifest):
    require(integer(args.seconds, 1, 60), "invalid_observation_budget")
    identities = identities_file(args.identities, manifest)
    end, edges = time.monotonic() + remaining(args.seconds), set()
    while time.monotonic() < end:
        edges.update(observe_once(manifest, identities))
        require(len(edges) <= MAX_ROWS, "observed_edge_budget_exceeded")
        time.sleep(min(0.02, max(0, end - time.monotonic())))
    emit({"kind": "result", "event": "observation", "run_id": manifest["run_id"],
          "identities": identities["roles"], "edges": sorted({edge[:2] for edge in edges}),
          "socket_edges": [{"from": edge[0], "to": edge[1], "protocol": edge[2],
                     "netns_inode": identities["roles"][edge[0]]["netns_inode"],
                     "local": {"address": edge[3], "port": edge[4]},
                     "remote": {"address": edge[5], "port": edge[6]}} for edge in sorted(edges)]})


def grpc_stats(port=18085):
    # Empty QueryStatsRequest means reset=false. Never issue GetStats/reset.
    def frame(kind, flags, stream, data=b""):
        return len(data).to_bytes(3, "big") + bytes([kind, flags]) + struct.pack("!I", stream) + data
    def string(value):
        data = value.encode("ascii")
        require(len(data) < 127, "hpack_string_budget_exceeded")
        return bytes([len(data)]) + data
    headers = b"\x83\x86\x01" + string("127.0.0.1:" + str(port)) + b"\x04" + string("/v2ray.core.app.stats.command.StatsService/QueryStats")
    headers += b"\x0f\x10" + string("application/grpc") + b"\x00" + string("te") + string("trailers")
    with socket.create_connection(("127.0.0.1", port), timeout=remaining()) as connection:
        connection.settimeout(remaining())
        connection.sendall(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" + frame(4, 0, 0) + frame(1, 4, 1, headers) + frame(0, 1, 1, b"\x00\x00\x00\x00\x00"))
        data = bytearray()
        for _ in range(128):
            header = exact(connection, 9)
            length, kind, flags = int.from_bytes(header[:3], "big"), header[3], header[4]
            stream = int.from_bytes(header[5:], "big") & 0x7fffffff
            require(length <= 65536, "grpc_frame_budget_exceeded")
            body = exact(connection, length)
            if kind == 4 and not flags & 1:
                connection.sendall(frame(4, 1, 0))
            elif kind == 6 and not flags & 1:
                connection.sendall(frame(6, 1, 0, body))
            elif kind == 0 and stream == 1:
                if flags & 8:
                    require(body and body[0] < len(body), "grpc_padding_invalid")
                    body = body[1:len(body) - body[0]]
                data.extend(body)
                require(len(data) <= 65536, "grpc_response_budget_exceeded")
            elif kind in (3, 7):
                raise Rejected("grpc_stream_rejected")
            if stream == 1 and kind in (0, 1) and flags & 1:
                break
        else:
            raise Rejected("grpc_frame_count_exceeded")
    require(len(data) >= 5 and data[0] == 0 and int.from_bytes(data[1:5], "big") == len(data) - 5,
            "grpc_unary_response_missing")
    def fields(raw):
        position, output = 0, []
        def varint():
            nonlocal position
            value = 0
            for shift in range(0, 70, 7):
                require(position < len(raw), "protobuf_truncated")
                byte = raw[position]
                position += 1
                value |= (byte & 127) << shift
                if not byte & 128:
                    return value
            raise Rejected("protobuf_varint_budget_exceeded")
        while position < len(raw):
            key = varint()
            if key & 7 == 0:
                value = varint()
            elif key & 7 == 2:
                size = varint()
                require(size <= len(raw) - position, "protobuf_length_invalid")
                value = raw[position:position + size]
                position += size
            else:
                raise Rejected("protobuf_wire_type_unsupported")
            output.append((key >> 3, key & 7, value))
            require(len(output) <= 64, "protobuf_field_budget_exceeded")
        return output
    result = {}
    for field, wire, message in fields(bytes(data[5:])):
        require(field == 1 and wire == 2, "unexpected_stats_response_field")
        values = {field: value for field, _, value in fields(message)}
        name, value = values.get(1, b"").decode("ascii"), values.get(2, 0)
        require(name.startswith("user>>>") and len(name) <= 512 and name not in result
                and type(value) is int and 0 <= value < 2**63, "invalid_user_counter")
        result[name] = value
    return [{"name": name, "value": str(value)} for name, value in sorted(result.items())]


def stats(args, manifest):
    identities = identities_file(args.identities, manifest)
    require(args.role in PRODUCT_ROLES and args.role in identities["roles"], "managed_stats_identity_required")
    identity = identities["roles"][args.role]
    if args.inside_namespace:
        require(os.stat("/proc/self/ns/net").st_ino == identity["netns_inode"], "stats_namespace_mismatch")
        counters = grpc_stats()
        validate_identity(identity)
        emit({"kind": "result", "event": "stats", "run_id": manifest["run_id"], "role": args.role,
              "identity": identity, "counters": counters})
        return
    descriptor = os.open("/proc/" + str(identity["pid"]) + "/ns/net", os.O_RDONLY | os.O_CLOEXEC)
    directory = absolute_path(manifest["root"]) / ("stats-" + uuid.uuid4().hex)
    directory.mkdir(mode=0o700)
    child = None
    try:
        require(os.fstat(descriptor).st_ino == identity["netns_inode"], "stats_namespace_mismatch")
        validate_identity(identity)
        # nsenter receives an already pinned namespace descriptor, not a PID
        # that could be reused between validation and namespace entry.
        command = ["nsenter", "--net=/proc/self/fd/" + str(descriptor), "--", sys.executable,
                   str(Path(__file__).resolve()), "stats", "--manifest", str(absolute_path(args.manifest)),
                   "--identities", str(absolute_path(args.identities)), "--role", args.role, "--inside-namespace"]
        child = Child(command, clean_environment(), directory / "stats.private.log", pass_fds=(descriptor,))
        try:
            child.process.wait(timeout=remaining(5))
            child.reader.join(timeout=1)
            require(not child.reader.is_alive() and child.process.returncode == 0,
                    "stats_namespace_worker_failed")
            output = bytes(child.output)
        except subprocess.TimeoutExpired:
            raise Rejected("stats_namespace_worker_timeout") from None
        private_bytes(directory / "stats.json", output)
        value = json.loads(output)
        require(value.get("event") == "stats" and value.get("identity") == identity
                and value.get("run_id") == manifest["run_id"] and value.get("role") == args.role,
                "stats_worker_binding_mismatch")
        validate_identity(identity)
        emit(value)
    finally:
        os.close(descriptor)
        if child:
            child.stop()


def parser():
    result = argparse.ArgumentParser(description=__doc__)
    commands = result.add_subparsers(dest="mode", required=True)
    initial = commands.add_parser("prepare")
    initial.add_argument("--plan", required=True)
    initial.add_argument("--work-dir", required=True)
    initial.add_argument("--dedicated-test-node", action="store_true")
    for mode in ("serve", "client", "observe", "stats"):
        command = commands.add_parser(mode)
        command.add_argument("--manifest", required=True)
        if mode == "client":
            command.add_argument("--config", required=True)
            command.add_argument("--config-sha256", required=True)
            command.add_argument("--phase", default="baseline")
            command.add_argument("--iterations", type=int, default=1)
            command.add_argument("--duration-secs", type=int)
            command.add_argument("--interval-ms", type=int, default=1000)
        elif mode in {"observe", "stats"}:
            command.add_argument("--identities", required=True)
            if mode == "observe":
                command.add_argument("--seconds", type=int, default=1)
            else:
                command.add_argument("--role", choices=sorted(PRODUCT_ROLES), required=True)
                command.add_argument("--inside-namespace", action="store_true", help=argparse.SUPPRESS)
    return result


def interrupted(signum, frame):
    raise KeyboardInterrupt


def main():
    global DEADLINE, STDOUT_BYTES
    os.umask(0o077)
    DEADLINE = time.monotonic() + MAX_DURATION
    STDOUT_BYTES = 0
    args = parser().parse_args()
    try:
        require(sys.platform.startswith("linux"), "dedicated_linux_required")
        signal.signal(signal.SIGTERM, interrupted)
        if args.mode == "prepare":
            require(args.dedicated_test_node, "dedicated_test_node_acknowledgement_required")
            prepare(args)
        else:
            manifest = load_manifest(args.manifest)
            {"serve": serve, "client": client, "observe": observe, "stats": stats}[args.mode](args, manifest)
    except KeyboardInterrupt:
        emit({"kind": "failure", "event": "failure", "code": "interrupted_not_a_pass"})
        return 130
    except Exception as error:
        emit({"kind": "failure", "event": "failure",
              "code": error.args[0] if isinstance(error, Rejected) else "unexpected_fixture_failure"})
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
