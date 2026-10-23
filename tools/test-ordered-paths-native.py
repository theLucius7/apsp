#!/usr/bin/env python3
"""Two-stage, TEST_ONLY Linux loopback acceptance of product-compiled paths.

prepare generates compiler-input.json on the dedicated Linux guest. Build the
Rust ordered_native_fixture example from the frozen checkout on the build host,
then invoke it with that absolute input path and a new absolute output directory.
Transfer its directory back unchanged. run requires independently recorded
manifest/generator-source hashes. Only statistics and mixed-listener ports are
relocated; original product files are also checked with the pinned native binary.
No successful result claims a production bundle, Agent deployment, or a ledger
transaction. Live cumulative counters prove single entry traffic measurement.
"""
import argparse
import base64
import contextlib
import datetime
import hashlib
import http.server
import ipaddress
import json
import os
import pathlib
import secrets
import signal
import socket
import socketserver
import ssl
import struct
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

from managed_paths_support import OwnedProcess

MAX_FILE = 2 * 1024 * 1024
MAX_OUTPUT = 256 * 1024
MAX_ROWS = 4096
REQUIRED_TAGS = {"with_clash_api", "with_v2ray_api", "with_utls", "with_quic"}
COMPILER_COMMON_SOURCES = frozenset({
    "lib.rs", "relays.rs", "external.rs", "protocols.rs", "certificates.rs",
    "paths/mod.rs", "paths/model.rs", "paths/network.rs", "paths/context.rs",
    "paths/validate.rs", "paths/render.rs", "paths/transport_validation.rs",
    "paths/outbound_validation.rs",
})
COMPILER_LEGACY_SOURCES = COMPILER_COMMON_SOURCES | {"settings.rs"}
COMPILER_SPLIT_SOURCES = COMPILER_COMMON_SOURCES | {
    "settings/mod.rs", "settings/apply.rs", "settings/transport.rs", "settings/validate.rs",
}
COMPILER_CLIENT_SOURCES = COMPILER_SPLIT_SOURCES | {"client.rs"}
PORT_NAMES = {"a", "m", "b", "x", "client", "stats_a", "stats_m", "stats_b", "https", "tcp_echo", "udp_echo", "handshake", "controller"}
DEADLINE = None


class Rejected(Exception):
    pass


def require(condition, code):
    if not condition:
        raise Rejected(code)


def remaining(maximum=10):
    value = min(maximum, DEADLINE - time.monotonic())
    require(value > 0, "total_deadline_exceeded")
    return value


def sha_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(65536), b""):
            remaining()
            digest.update(block)
    return digest.hexdigest()


def path_absolute(value):
    path = pathlib.Path(value)
    require(path.is_absolute(), "absolute_path_required")
    return path


def private_json(path, value):
    data = json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True).encode()
    require(len(data) <= MAX_FILE, "evidence_budget_exceeded")
    with open(path, "xb") as target:
        os.chmod(path, 0o600)
        target.write(data)


def load_json(path):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= MAX_FILE, "bounded_regular_json_required")
    with open(path, "rb") as source:
        return json.load(source)


def capture(command, timeout=10, private_log=None):
    process = OwnedProcess(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    buffers, overflow, total, lock = [bytearray(), bytearray()], threading.Event(), [0], threading.Lock()
    def drain(stream, index):
        for block in iter(lambda: stream.read(4096), b""):
            with lock:
                allowed = max(0, MAX_OUTPUT - total[0])
                buffers[index].extend(block[:allowed])
                total[0] += len(block)
                if total[0] > MAX_OUTPUT:
                    overflow.set()
                    break
    readers = [threading.Thread(target=drain, args=(stream, index), daemon=True) for index, stream in enumerate((process.stdout, process.stderr))]
    for reader in readers:
        reader.start()
    try:
        try:
            process.wait(timeout=remaining(timeout))
        except subprocess.TimeoutExpired:
            raise Rejected("controlled_command_timeout") from None
        for reader in readers:
            reader.join(timeout=1)
        require(not overflow.is_set() and all(not reader.is_alive() for reader in readers), "command_output_budget_exceeded")
        require(process.returncode == 0, "controlled_command_failed")
        return bytes(buffers[0])
    finally:
        process.stop_group()
        for reader in readers:
            reader.join(timeout=1)
        for stream in (process.stdout, process.stderr):
            stream.close()
        if private_log is not None:
            with open(private_log, "xb") as target:
                target.write(bytes(buffers[1]))
        require(all(not reader.is_alive() for reader in readers), "controlled_command_reader_cleanup_timeout")


def binary_identity(binary, expected):
    require(binary.is_absolute() and binary.is_file() and not binary.is_symlink(), "regular_native_binary_required")
    require(binary.stat().st_size <= 512 * 1024 * 1024, "native_binary_file_budget_exceeded")
    require(len(expected) == 64 and sha_file(binary) == expected.lower(), "native_binary_hash_mismatch")
    version = capture([str(binary), "version"]).decode("utf-8")
    require(any(line.strip() == "sing-box version 1.14.2" for line in version.splitlines()), "exact_runtime_version_required")
    tags = next((line.split(":", 1)[1].strip() for line in version.splitlines() if line.startswith("Tags:")), None)
    actual = set(tags.split(",")) if tags is not None else set()
    actual = {tag.strip() for tag in actual}
    require(REQUIRED_TAGS <= actual, "required_runtime_tags_missing")
    return {"version": "1.14.2", "sha256": expected.lower(), "observed_build_tags": sorted(actual)}


def linux_dedicated(args):
    require(sys.platform.startswith("linux"), "dedicated_linux_required_not_a_pass")
    require(args.dedicated_test_node, "dedicated_test_node_acknowledgement_required")
    require(pathlib.Path("/proc/net/tcp").is_file(), "linux_process_socket_observation_required")
    memory = next((line.split()[1] for line in pathlib.Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemAvailable:")), "0")
    require(int(memory) >= 256 * 1024, "insufficient_memory_for_bounded_multi_process_fixture")


def disk_budget(path):
    while not path.exists():
        path = path.parent
    budget = os.statvfs(path)
    require(budget.f_bavail * budget.f_frsize >= 64 * 1024 * 1024 and budget.f_favail >= 64, "insufficient_fixture_disk_or_inodes")


def reserve(port):
    handles = []
    try:
        for kind in (socket.SOCK_STREAM, socket.SOCK_DGRAM):
            item = socket.socket(socket.AF_INET, kind)
            handles.append(item)
            if kind == socket.SOCK_STREAM:
                item.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            item.bind(("127.0.0.1", port))
            if kind == socket.SOCK_STREAM:
                item.listen(1)
        return handles
    except OSError:
        for item in handles:
            item.close()
        raise Rejected("reserved_port_conflict_no_process_was_stopped") from None


def allocate_ports():
    result = {"controller": 18086}
    reservations = reserve(18086)
    try:
        for name in sorted(PORT_NAMES - {"controller"}):
            for _ in range(128):
                with socket.socket() as candidate:
                    candidate.bind(("127.0.0.1", 0))
                    port = candidate.getsockname()[1]
                if port in result.values() or port == 18085:
                    continue
                try:
                    handles = reserve(port)
                except Rejected:
                    continue
                reservations.extend(handles)
                result[name] = port
                break
            require(name in result, "unable_to_allocate_bounded_fixture_ports")
        return result
    finally:
        for handle in reservations:
            handle.close()


def prepare(args):
    linux_dedicated(args)
    binary = path_absolute(args.native_binary)
    identity = binary_identity(binary, args.native_sha256)
    directory = path_absolute(args.evidence_dir)
    disk_budget(directory.parent)
    directory.mkdir(mode=0o700)
    tls = directory / "tls"
    tls.mkdir(mode=0o700)
    (tls / "empty-ca-directory").mkdir(mode=0o700)
    credentials = {"public_uuid": "00000000-0000-0000-0000-000000000007", "x_username": "TEST_ONLY_ACCOUNT",
                   "x_password": "TEST_ONLY_" + secrets.token_hex(24), "controller_secret": secrets.token_hex(32)}
    keys = {}
    for role in ("A", "M", "B"):
        output = capture([str(binary), "generate", "reality-keypair"]).decode("ascii")
        pair = {}
        for line in output.splitlines():
            if line.startswith("PrivateKey:"):
                pair["private_key"] = line.split(":", 1)[1].strip()
            elif line.startswith("PublicKey:"):
                pair["public_key"] = line.split(":", 1)[1].strip()
        require(set(pair) == {"private_key", "public_key"}, "native_test_keypair_output_invalid")
        for value in pair.values():
            require(len(base64.urlsafe_b64decode(value + "=" * (-len(value) % 4))) == 32, "native_test_keypair_invalid")
        keys[role] = pair
    extensions = tls / "extensions.cnf"
    extensions.write_text("subjectAltName=DNS:reality.test,DNS:target.test,IP:127.0.0.1\nextendedKeyUsage=serverAuth\n")
    capture(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-sha256", "-days", "1", "-subj", "/CN=Apsp TEST ONLY CA",
             "-keyout", str(tls / "ca.key"), "-out", str(tls / "ca.crt")], 20)
    capture(["openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=reality.test", "-keyout", str(tls / "server.key"), "-out", str(tls / "server.csr")], 20)
    capture(["openssl", "x509", "-req", "-in", str(tls / "server.csr"), "-CA", str(tls / "ca.crt"), "-CAkey", str(tls / "ca.key"), "-CAcreateserial",
             "-out", str(tls / "server.crt"), "-days", "1", "-sha256", "-extfile", str(extensions)], 20)
    value = {"schema": 1, "test_only": True, "runtime_version": "1.14.2", "native_binary_sha256": identity["sha256"],
             "ports": allocate_ports(), "node_keys": keys, "credentials": credentials}
    private_json(directory / "compiler-input.json", value)
    private_json(directory / "prepare-receipt.json", {"schema": 1, "test_only": True, "native": identity,
        "compiler_input_sha256": sha_file(directory / "compiler-input.json"), "tls_files": {path.name: sha_file(path) for path in tls.iterdir() if path.is_file()},
        "scope": "test-only keys and CA; no host trust or network configuration changed", "traffic_verified": False})
    print(json.dumps({"status": "prepared_not_tested", "compiler_input": str(directory / "compiler-input.json")}), flush=True)


def checked_relative(parent, name):
    require(isinstance(name, str) and len(name) < 256, "invalid_artifact_name")
    path = parent / name
    require(not pathlib.PurePath(name).is_absolute() and ".." not in pathlib.PurePath(name).parts and path.is_file() and not path.is_symlink(), "artifact_path_escape")
    require(path.stat().st_size <= MAX_FILE and path.resolve().is_relative_to(parent.resolve()), "bounded_artifact_required")
    return path


def pointer_assign(value, pointer, replacement):
    parts = pointer.lstrip("/").split("/")
    item = value
    for part in parts[:-1]:
        item = item[int(part)] if isinstance(item, list) else item[part]
    part = int(parts[-1]) if isinstance(item, list) else parts[-1]
    item[part] = replacement


def compiler_source_identity(value):
    require(isinstance(value, dict), "compiler_source_identity_missing")
    require(frozenset(value) in (COMPILER_LEGACY_SOURCES, COMPILER_SPLIT_SOURCES, COMPILER_CLIENT_SOURCES), "compiler_source_identity_missing")
    require(all(isinstance(digest, str) and len(digest) == 64
                and all(character in "0123456789abcdef" for character in digest)
                for digest in value.values()), "compiler_source_identity_missing")


def compilation(args, directory, inputs):
    parent = path_absolute(args.compiled_dir)
    manifest_file = checked_relative(parent, "compilation-manifest.json")
    require(sha_file(manifest_file) == args.compilation_manifest_sha256.lower(), "compilation_receipt_hash_mismatch")
    manifest = load_json(manifest_file)
    require(manifest.get("schema") == 1 and manifest.get("test_only") is True and manifest.get("runtime_version") == "1.14.2", "invalid_compilation_contract")
    require(manifest.get("native_binary_sha256") == inputs["native_binary_sha256"] and manifest.get("compiler_input_sha256") == sha_file(directory / "compiler-input.json"), "compiled_input_binding_mismatch")
    require(manifest.get("generator_source_sha256") == args.generator_source_sha256.lower(), "generator_source_binding_mismatch")
    compiler_source_identity(manifest.get("compiler_sources"))
    require([case.get("name") for case in manifest["cases"]] == ["three", "four"], "complete_path_cases_required")
    for case in manifest["cases"]:
        require(set(case["files"]) == {"A", "M", "B", "client"}, "product_role_missing")
        for role, record in case["files"].items():
            product = checked_relative(parent, record["product_file"])
            fixture = checked_relative(parent, record["fixture_file"])
            require(sha_file(product) == record["product_sha256"] and sha_file(fixture) == record["fixture_sha256"], "compiled_file_hash_mismatch")
            source, controlled = load_json(product), load_json(fixture)
            expected = "/inbounds/0/listen_port" if role == "client" else "/experimental/v2ray_api/listen"
            to = inputs["ports"]["client"] if role == "client" else "127.0.0.1:" + str(inputs["ports"]["stats_" + role.lower()])
            require(record["transforms"] == [{"pointer": expected, "from": 2080 if role == "client" else "127.0.0.1:18085", "to": to}], "unapproved_fixture_transform")
            pointer_assign(source, expected, to)
            require(source == controlled, "fixture_changes_product_routing_or_authentication")
    return parent, manifest


class NativeProcess:
    def __init__(self, binary, config, env, log_path):
        self.overflow = False
        self.log = open(log_path, "xb")
        try:
            self.process = OwnedProcess([str(binary), "run", "-c", str(config)], env=env, stdin=subprocess.DEVNULL,
                                            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, start_new_session=True)
        except Exception:
            self.log.close()
            raise
        self.reader = threading.Thread(target=self._drain, daemon=True)
        self.reader.start()

    def _drain(self):
        count = 0
        for block in iter(lambda: self.process.stderr.read(4096), b""):
            allowed = max(0, MAX_OUTPUT - count)
            self.log.write(block[:allowed])
            count += len(block)
            if count > MAX_OUTPUT:
                self.overflow = True
        self.log.flush()

    def stop(self):
        self.process.stop_group()
        self.reader.join(timeout=1)
        require(not self.reader.is_alive(), "native_log_reader_cleanup_timeout")
        self.process.stderr.close()
        if not self.log.closed:
            self.log.close()


def wait_port(port, process):
    end = time.monotonic() + remaining(5)
    while time.monotonic() < end:
        require(process.process.poll() is None, "native_service_exited_before_ready")
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                return
        except OSError:
            time.sleep(0.03)
    raise Rejected("native_service_readiness_timeout")


def net_rows(protocol):
    result = []
    for name in (protocol, protocol + "6"):
        path = pathlib.Path("/proc/net") / name
        if not path.is_file():
            continue
        with open(path, encoding="ascii") as source:
            next(source)
            for line in source:
                require(len(result) < MAX_ROWS, "socket_table_budget_exceeded")
                fields = line.split()
                def address(value):
                    host, port = value.split(":")
                    raw = bytes.fromhex(host)
                    if len(raw) == 4:
                        host = socket.inet_ntoa(raw[::-1])
                    else:
                        raw = b"".join(raw[index:index + 4][::-1] for index in range(0, 16, 4))
                        parsed = ipaddress.IPv6Address(raw)
                        host = str(parsed.ipv4_mapped or parsed)
                    return host, int(port, 16)
                result.append((address(fields[1]), address(fields[2]), fields[9]))
    return result


class Observer:
    def __init__(self, processes, ports):
        self.processes, self.ports = processes, ports
        self.edges, self.lock, self.closed, self.error = set(), threading.Lock(), threading.Event(), None
        self.thread = threading.Thread(target=self._observe, daemon=True)
        self.thread.start()

    def owners(self):
        result = {}
        for role, item in list(self.processes.items()):
            if item.process.poll() is not None:
                continue
            directory = pathlib.Path("/proc") / str(item.process.pid) / "fd"
            try:
                files = list(directory.iterdir())
            except FileNotFoundError:
                require(item.process.poll() is not None, "owned_process_socket_table_disappeared")
                continue
            require(len(files) <= 256, "process_descriptor_budget_exceeded")
            for file in files:
                try:
                    target = os.readlink(file)
                except (FileNotFoundError, PermissionError):
                    continue
                if target.startswith("socket:["):
                    result[target[8:-1]] = role
        return result

    def final_owner(self, protocol, peer, target_port):
        owners = self.owners()
        for local, remote, inode in net_rows(protocol):
            same_local = local == peer or (protocol == "udp" and local[1] == peer[1] and local[0] in {"0.0.0.0", "::"})
            if same_local and (protocol == "udp" or remote == ("127.0.0.1", target_port)):
                return owners.get(inode)
        return None

    def _observe(self):
        try:
            destinations = {self.ports[key]: value for key, value in {"a": "A", "m": "M", "b": "B", "x": "X", "https": "target", "tcp_echo": "target"}.items()}
            while not self.closed.is_set():
                owners = self.owners()
                with self.lock:
                    for local, remote, inode in net_rows("tcp"):
                        if inode in owners and remote[0] == "127.0.0.1" and remote[1] in destinations:
                            self.edges.add((owners[inode], destinations[remote[1]]))
                self.closed.wait(0.02)
        except Exception:
            self.error = "process_socket_observation_failed"

    def reset(self):
        with self.lock:
            self.edges.clear()

    def snapshot(self):
        require(self.error is None, "process_socket_observation_failed")
        with self.lock:
            return sorted(self.edges)

    def stop(self):
        self.closed.set()
        self.thread.join(timeout=2)
        require(not self.thread.is_alive(), "observer_cleanup_timeout")


class BoundedTCP(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True
    block_on_close = False
    gate = threading.BoundedSemaphore(32)

    def __init__(self, *args, **kwargs):
        self.active, self.active_lock, self.tls = set(), threading.Lock(), None
        super().__init__(*args, **kwargs)

    def get_request(self):
        connection, address = super().get_request()
        connection.settimeout(3)
        if self.tls is not None:
            try:
                connection = self.tls.wrap_socket(connection, server_side=True)
            except Exception:
                connection.close()
                raise
        return connection, address

    def process_request(self, request, address):
        if not self.gate.acquire(blocking=False):
            request.close()
            return
        with self.active_lock:
            self.active.add(request)
        super().process_request(request, address)

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            with self.active_lock:
                self.active.discard(request)
            self.gate.release()

    def server_close(self):
        with self.active_lock:
            for request in self.active:
                with contextlib.suppress(OSError):
                    request.shutdown(socket.SHUT_RDWR)
                request.close()
        super().server_close()

    def handle_error(self, request, address):
        pass


class QuietUDP(socketserver.UDPServer):
    def handle_error(self, request, address):
        pass


def exact(sock, length):
    require(length <= 65536, "socket_read_budget_exceeded")
    data = bytearray()
    while len(data) < length:
        part = sock.recv(length - len(data))
        require(part, "unexpected_socket_eof")
        data.extend(part)
    return bytes(data)


def socks_address(sock):
    kind = exact(sock, 1)[0]
    if kind == 1:
        host = socket.inet_ntoa(exact(sock, 4))
    elif kind == 3:
        host = exact(sock, exact(sock, 1)[0]).decode("ascii")
    elif kind == 4:
        host = socket.inet_ntop(socket.AF_INET6, exact(sock, 16))
    else:
        raise Rejected("invalid_socks_address")
    return host, struct.unpack("!H", exact(sock, 2))[0]


def socks(port, command, destination_port):
    connection = socket.create_connection(("127.0.0.1", port), timeout=remaining(5))
    try:
        connection.settimeout(remaining(5))
        connection.sendall(b"\x05\x01\x00")
        require(exact(connection, 2) == b"\x05\x00", "socks_authentication_rejected")
        connection.sendall(bytes([5, command, 0, 1, 127, 0, 0, 1]) + struct.pack("!H", destination_port))
        header = exact(connection, 3)
        require(header == b"\x05\x00\x00", "socks_route_rejected")
        endpoint = socks_address(connection)
        return connection, endpoint
    except Exception:
        connection.close()
        raise


def grpc_stats(port):
    # Minimal h2c unary client for the pinned upstream stats.proto. Response
    # DATA must contain exactly one uncompressed protobuf message; errors that
    # only send trailers cannot be mistaken for an empty successful response.
    def frame(kind, flags, stream, data=b""):
        return len(data).to_bytes(3, "big") + bytes([kind, flags]) + struct.pack("!I", stream) + data
    def string(value):
        data = value.encode("ascii")
        require(len(data) < 127, "hpack_string_budget_exceeded")
        return bytes([len(data)]) + data
    headers = b"\x83\x86\x01" + string("127.0.0.1:" + str(port)) + b"\x04" + string("/v2ray.core.app.stats.command.StatsService/QueryStats")
    headers += b"\x0f\x10" + string("application/grpc") + b"\x00" + string("te") + string("trailers")
    with socket.create_connection(("127.0.0.1", port), timeout=remaining(3)) as client:
        client.settimeout(remaining(3))
        client.sendall(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" + frame(4, 0, 0) + frame(1, 4, 1, headers) + frame(0, 1, 1, b"\x00\x00\x00\x00\x00"))
        data = bytearray()
        for _ in range(128):
            header = exact(client, 9)
            length, kind, flags, stream = int.from_bytes(header[:3], "big"), header[3], header[4], int.from_bytes(header[5:], "big") & 0x7fffffff
            require(length <= 65536, "grpc_frame_budget_exceeded")
            body = exact(client, length)
            if kind == 4 and not flags & 1:
                client.sendall(frame(4, 1, 0))
            elif kind == 6 and not flags & 1:
                client.sendall(frame(6, 1, 0, body))
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
    require(len(data) >= 5 and data[0] == 0 and int.from_bytes(data[1:5], "big") == len(data) - 5, "grpc_unary_response_missing")
    def fields(raw):
        position = 0
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
        output = []
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
        name = values.get(1, b"").decode("ascii")
        value = values.get(2, 0)
        require(name.startswith("user>>>") and len(name) <= 512 and name not in result and isinstance(value, int) and value < 2**63, "invalid_user_counter")
        result[name] = value
    return result


def probe(inputs, selector, secret=True):
    target = "https://127.0.0.1:" + str(inputs["ports"]["https"]) + "/health"
    query = urllib.parse.urlencode({"url": target, "timeout": 5000})
    url = "http://127.0.0.1:18086/proxies/" + urllib.parse.quote(selector, safe="") + "/delay?" + query
    request = urllib.request.Request(url)
    if secret:
        request.add_header("Authorization", "Bearer " + inputs["credentials"]["controller_secret"])
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        with opener.open(request, timeout=remaining(7)) as response:
            body = response.read(8193)
            require(len(body) <= 8192, "native_probe_body_budget_exceeded")
            value = json.loads(body)
            require(response.status == 200 and isinstance(value.get("delay"), int) and value["delay"] >= 0, "native_selector_probe_invalid")
            return response.status
    except urllib.error.HTTPError as error:
        return error.code


def run_case(binary, parent, case, inputs, evidence, env):
    ports = inputs["ports"]
    four = case["name"] == "four"
    directory = evidence / ("run-" + case["name"])
    directory.mkdir(mode=0o700)
    processes, servers, threads, observations = {}, [], [], []
    reservations, observer = {}, None
    observations_lock = threading.Lock()
    def record(kind, peer, port, size=0):
        item = {"kind": kind, "owner": observer.final_owner("udp" if kind == "udp" else "tcp", peer, port), "bytes": size}
        with observations_lock:
            require(len(observations) < 128, "target_observation_budget_exceeded")
            observations.append(item)
        return item
    def tls_context(handshake=False):
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(str(evidence / "tls/server.crt"), str(evidence / "tls/server.key"))
        context.minimum_version = ssl.TLSVersion.TLSv1_3
        context.maximum_version = ssl.TLSVersion.TLSv1_3
        context.set_alpn_protocols(["h2", "http/1.1"] if handshake else ["http/1.1"])
        return context
    class Handshake(socketserver.BaseRequestHandler):
        def handle(self):
            self.request.settimeout(3)
            with contextlib.suppress(OSError, ssl.SSLError):
                with tls_context(True).wrap_socket(self.request, server_side=True) as secure:
                    secure.settimeout(2)
                    secure.recv(4096)
    class Echo(socketserver.BaseRequestHandler):
        def handle(self):
            self.request.settimeout(5)
            record("tcp_accept", self.client_address, ports["tcp_echo"])
            with contextlib.suppress(OSError, Rejected):
                payload = exact(self.request, 4096)
                record("tcp", self.client_address, ports["tcp_echo"], len(payload))
                self.request.sendall(payload)
                time.sleep(0.4)
    class HTTPS(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"
        def do_HEAD(self):
            if self.path != "/health":
                self.send_error(404)
                return
            record("https", self.client_address, ports["https"])
            time.sleep(0.2)
            self.send_response(200)
            self.send_header("Content-Length", "0")
            self.send_header("Connection", "close")
            self.end_headers()
        def log_message(self, *args):
            pass
    class UDP(socketserver.BaseRequestHandler):
        def handle(self):
            payload, sender = self.request
            if len(payload) <= 512:
                record("udp", self.client_address, ports["udp_echo"], len(payload))
                sender.sendto(payload, self.client_address)
    def release(name):
        for item in reservations.pop(name):
            item.close()
    def start_server(name, handler, udp=False, https=False):
        release(name)
        cls = QuietUDP if udp else BoundedTCP
        server = cls(("127.0.0.1", ports[name]), handler)
        if https:
            server.tls = tls_context()
        servers.append(server)
        thread = threading.Thread(target=server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)
        thread.start()
        threads.append(thread)
    def start_native(role, config, port_name):
        release(port_name)
        if role in ("A", "M", "B"):
            release("stats_" + role.lower())
        if role == "A":
            release("controller")
        process = NativeProcess(binary, config, env, directory / (role + ".private.log"))
        processes[role] = process
        wait_port(ports[port_name], process)
    def assert_graph(edges, udp=False):
        required = {("client", "A"), ("X", "B")}
        if not udp:
            required.add(("B", "target"))
        required |= {("A", "M"), ("M", "X")} if four else {("A", "X")}
        require(required <= set(map(tuple, edges)), "every_hop_was_not_observed")
        forbidden = {("A", "B"), ("A", "target"), ("M", "B"), ("M", "target"), ("X", "target"), ("client", "target")}
        require(not forbidden & set(map(tuple, edges)), "shorter_or_direct_route_observed")
    try:
        for name, port in ports.items():
            reservations[name] = reserve(port)
        x_file = directory / "external-X.fixture.json"
        private_json(x_file, {"inbounds": [{"type": "http", "tag": "TEST_ONLY_X", "listen": "127.0.0.1", "listen_port": ports["x"],
            "users": [{"username": inputs["credentials"]["x_username"], "password": inputs["credentials"]["x_password"]}]}],
            "outbounds": [{"type": "direct", "tag": "direct"}], "route": {"final": "direct"}, "log": {"level": "warn"}})
        for role, record_file in case["files"].items():
            for name in ("product_file", "fixture_file"):
                capture([str(binary), "check", "-c", str(checked_relative(parent, record_file[name]))], private_log=directory / ("check-" + role + "-" + name + ".private.log"))
        capture([str(binary), "check", "-c", str(x_file)], private_log=directory / "check-X.private.log")
        start_server("handshake", Handshake)
        start_server("tcp_echo", Echo)
        start_server("udp_echo", UDP, udp=True)
        start_server("https", HTTPS, https=True)
        for role in (["B", "M"] if four else ["B"]):
            start_native(role, checked_relative(parent, case["files"][role]["fixture_file"]), role.lower())
        start_native("X", x_file, "x")
        start_native("A", checked_relative(parent, case["files"]["A"]["fixture_file"]), "a")
        start_native("client", checked_relative(parent, case["files"]["client"]["fixture_file"]), "client")
        observer = Observer(processes, ports)
        require(probe(inputs, case["final_tag"], False) == 401, "unauthenticated_controller_access")
        require(probe(inputs, "INVALID_TEST_ONLY_SELECTOR") == 404, "unknown_selector_not_rejected")
        require(probe(inputs, case["final_tag"]) == 200, "concrete_selector_https_head_failed")
        require(any(item["kind"] == "https" and item["owner"] == "B" for item in observations), "selector_probe_final_exit_not_observed")
        baseline = grpc_stats(ports["stats_a"])
        observer.reset()
        payload = (b"TEST_ONLY_" + case["name"].encode() + b"_").ljust(4096, b"q")
        with contextlib.closing(socks(ports["client"], 1, ports["tcp_echo"])[0]) as connection:
            connection.sendall(payload)
            require(exact(connection, len(payload)) == payload, "tcp_echo_payload_differs")
            time.sleep(0.3)
        time.sleep(0.1)
        tcp_edges = observer.snapshot()
        assert_graph(tcp_edges)
        require(any(item["kind"] == "tcp" and item["owner"] == "B" for item in observations), "tcp_final_exit_not_observed")
        counters = grpc_stats(ports["stats_a"])
        for direction in ("uplink", "downlink"):
            key = "user>>>u7_n1>>>traffic>>>" + direction
            require(counters.get(key, 0) - baseline.get(key, 0) == len(payload), "entry_tcp_measurement_not_exactly_once")
        require(set(counters) == {"user>>>u7_n1>>>traffic>>>uplink", "user>>>u7_n1>>>traffic>>>downlink"}, "unexpected_entry_identity_counted")
        observer.reset()
        udp_baseline = counters
        control, relay = socks(ports["client"], 3, 0)
        packet_sizes = []
        try:
            host = "127.0.0.1" if relay[0] in ("0.0.0.0", "::") else relay[0]
            require(host == "127.0.0.1" and relay[1] > 0, "udp_relay_not_loopback")
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
                udp.settimeout(remaining(3))
                for sequence in range(5):
                    data = ("TEST_ONLY_" + case["name"] + "_UDP_" + str(sequence)).encode().ljust(192, b"u")
                    packet = b"\x00\x00\x00\x01\x7f\x00\x00\x01" + struct.pack("!H", ports["udp_echo"]) + data
                    udp.sendto(packet, (host, relay[1]))
                    response, peer = udp.recvfrom(1024)
                    require(peer[0] == "127.0.0.1" and response[:4] == b"\x00\x00\x00\x01" and response[4:10] == packet[4:10] and response[10:] == data, "udp_echo_payload_or_destination_differs")
                    packet_sizes.append(len(data))
                    time.sleep(0.1)
                udp_edges = observer.snapshot()
                # The last leg is a datagram, independently attributed below.
                assert_graph(udp_edges, udp=True)
                require(sum(item["kind"] == "udp" and item["owner"] == "B" for item in observations) == 5, "udp_final_exit_not_observed")
        finally:
            control.close()
        after_udp = grpc_stats(ports["stats_a"])
        for direction in ("uplink", "downlink"):
            key = "user>>>u7_n1>>>traffic>>>" + direction
            require(after_udp.get(key, 0) - udp_baseline.get(key, 0) == sum(packet_sizes), "entry_udp_measurement_not_exactly_once")
        internal = {role: grpc_stats(ports["stats_" + role.lower()]) for role in (["M", "B"] if four else ["B"])}
        require(all(not value for value in internal.values()), "internal_relay_was_metered")
        target_count = len(observations)
        processes["X"].stop()
        try:
            connection, _ = socks(ports["client"], 1, ports["tcp_echo"])
            with connection:
                connection.settimeout(remaining(2))
                connection.sendall(payload)
                received = connection.recv(len(payload))
                require(not received, "tcp_bypassed_stopped_external_hop")
        except (OSError, Rejected) as error:
            if isinstance(error, Rejected):
                require(error.args[0] in {"socks_route_rejected", "unexpected_socket_eof"}, "unexpected_negative_tcp_failure")
        control, relay = socks(ports["client"], 3, 0)
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
                udp.settimeout(remaining(2))
                udp.sendto(b"\x00\x00\x00\x01\x7f\x00\x00\x01" + struct.pack("!H", ports["udp_echo"]) + b"TEST_ONLY_X_STOPPED", ("127.0.0.1", relay[1]))
                try:
                    udp.recvfrom(1024)
                except socket.timeout:
                    pass
                else:
                    raise Rejected("udp_bypassed_stopped_external_hop")
        finally:
            control.close()
        require(probe(inputs, case["final_tag"]) != 200, "selector_probe_bypassed_stopped_external_hop")
        require(len(observations) == target_count, "target_reached_after_external_hop_stopped")
        require(all(item.process.poll() is None for role, item in processes.items() if role != "X"), "other_proxy_failed_during_negative_probe")
        require(all(not item.overflow for item in processes.values()), "native_log_budget_exceeded")
        result = {"case": case["name"], "native_check": True, "tcp_verified": True, "udp_verified": True, "selector_https_head_verified": True,
                  "every_hop_verified": True, "final_exit_process": "B", "failure_no_bypass_verified": True, "entry_measurement_exactly_once": True,
                  "internal_relay_metered": False, "tcp_edges": tcp_edges, "udp_tcp_carrier_edges": udp_edges, "target_observations": observations,
                  "entry_counters": after_udp, "internal_counters": internal, "agent_deployment_verified": False, "ledger_transaction_verified": False}
        return result
    finally:
        cleanup_errors = []
        if observer:
            try:
                observer.stop()
            except Exception:
                cleanup_errors.append("observer")
        for process in reversed(list(processes.values())):
            try:
                process.stop()
            except Exception:
                cleanup_errors.append("process")
        for server in servers:
            try:
                server.shutdown()
                server.server_close()
            except Exception:
                cleanup_errors.append("server")
        for thread in threads:
            thread.join(timeout=1)
        for handles in reservations.values():
            for item in handles:
                item.close()
        require(all(item.process.poll() is not None for item in processes.values()), "native_process_cleanup_not_confirmed")
        require(all(not item.overflow for item in processes.values()), "native_log_budget_exceeded")
        # Rebinding every reserved TCP/UDP port proves listener cleanup without
        # terminating any unrelated process or changing the host network.
        for port in ports.values():
            for item in reserve(port):
                item.close()
        require(not cleanup_errors, "owned_fixture_cleanup_failed")


def run(args):
    linux_dedicated(args)
    directory = path_absolute(args.evidence_dir)
    disk_budget(directory)
    inputs = load_json(directory / "compiler-input.json")
    receipt = load_json(directory / "prepare-receipt.json")
    require(inputs.get("test_only") is True and set(inputs["ports"]) == PORT_NAMES and len(set(inputs["ports"].values())) == len(PORT_NAMES), "invalid_fixture_input")
    require(sha_file(directory / "compiler-input.json") == receipt["compiler_input_sha256"], "prepared_input_changed")
    for name, digest in receipt["tls_files"].items():
        require(sha_file(checked_relative(directory / "tls", name)) == digest, "prepared_tls_material_changed")
    identity = binary_identity(path_absolute(args.native_binary), inputs["native_binary_sha256"])
    parent, manifest = compilation(args, directory, inputs)
    env = os.environ.copy()
    env["SSL_CERT_FILE"] = str(directory / "tls/ca.crt")
    env["SSL_CERT_DIR"] = str(directory / "tls/empty-ca-directory")
    for name in list(env):
        if name.lower() in {"http_proxy", "https_proxy", "all_proxy", "no_proxy"}:
            del env[name]
    results = []
    for case in manifest["cases"]:
        result = run_case(path_absolute(args.native_binary), parent, case, inputs, directory, env)
        results.append(result)
        private_json(directory / ("result-" + case["name"] + ".json"), result)
        print(json.dumps({"status": "case_passed_and_cleaned", "case": case["name"]}), flush=True)
    private_json(directory / "acceptance-result.json", {"schema": 1, "test_only": True, "native": identity,
        "compilation_manifest_sha256": args.compilation_manifest_sha256, "generator_source_sha256": args.generator_source_sha256,
        "compiler_sources": manifest["compiler_sources"], "cases": results, "cleanup_confirmed": True,
        "scope": manifest["scope"], "production_bundle_verified": False, "agent_deployment_verified": False, "ledger_transaction_verified": False})
    print(json.dumps({"status": "passed_controlled_product_graph", "evidence": str(directory / "acceptance-result.json")}), flush=True)


def main():
    global DEADLINE
    os.umask(0o077)
    DEADLINE = time.monotonic() + 300
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="mode", required=True)
    initial = commands.add_parser("prepare")
    initial.add_argument("--native-binary", required=True)
    initial.add_argument("--native-sha256", required=True)
    initial.add_argument("--evidence-dir", required=True)
    initial.add_argument("--dedicated-test-node", action="store_true")
    final = commands.add_parser("run")
    final.add_argument("--native-binary", required=True)
    final.add_argument("--evidence-dir", required=True)
    final.add_argument("--compiled-dir", required=True)
    final.add_argument("--compilation-manifest-sha256", required=True)
    final.add_argument("--generator-source-sha256", required=True)
    final.add_argument("--dedicated-test-node", action="store_true")
    args = parser.parse_args()
    try:
        prepare(args) if args.mode == "prepare" else run(args)
    except KeyboardInterrupt:
        print(json.dumps({"status": "interrupted_not_a_pass"}), flush=True)
        return 130
    except Exception as error:
        code = error.args[0] if isinstance(error, Rejected) and error.args else "unexpected_fixture_failure"
        directory = pathlib.Path(args.evidence_dir)
        if directory.is_absolute() and directory.is_dir() and not directory.is_symlink():
            with contextlib.suppress(Exception):
                private_json(directory / "failure-result.json", {"schema": 1, "test_only": True, "status": "rejected_or_failed_not_a_pass", "phase": args.mode, "code": code})
        print(json.dumps({"status": "rejected_or_failed_not_a_pass", "code": code}), flush=True)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
