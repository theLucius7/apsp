#!/usr/bin/env python3
"""Use unchanged legacy SS clients before and after the configuration rewrite.

Run in an isolated container with a previously verified sing-box executable and
the JSON emitted by the preserved_runtime_fixture compiler example. The kept
client must pass TCP and UDP in both phases; the revoked client must work
before the rewrite and be refused afterwards.
"""
import argparse
import hashlib
import http.server
import json
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import threading
import time


PAYLOAD = b'APSP_TEST_ONLY_SS_CONTINUITY'
CLIENTS = {'client': 23544, 'revoked_client': 23547}


def receive(connection, size):
    result = b''
    while len(result) < size:
        chunk = connection.recv(size - len(result))
        if not chunk:
            raise RuntimeError('Unexpected proxy EOF')
        result += chunk
    return result


def socks(proxy, command, port):
    connection = socket.create_connection(('127.0.0.1', proxy), timeout=5)
    connection.sendall(b'\x05\x01\x00')
    if receive(connection, 2) != b'\x05\x00':
        raise RuntimeError('SOCKS negotiation failed')
    connection.sendall(b'\x05' + bytes([command]) + b'\x00\x01\x7f\x00\x00\x01' + struct.pack('!H', port))
    header = receive(connection, 4)
    if header[:2] != b'\x05\x00':
        raise RuntimeError('SOCKS request refused')
    address = receive(connection, {1: 4, 4: 16}[header[3]])
    remote_port = struct.unpack('!H', receive(connection, 2))[0]
    return connection, (socket.inet_ntop(socket.AF_INET if header[3] == 1 else socket.AF_INET6, address), remote_port)


def traffic(proxy):
    connection, _ = socks(proxy, 1, 23545)
    with connection:
        connection.sendall(b'GET / HTTP/1.0\r\nHost: fixture.invalid\r\n\r\n')
        data = b''
        while chunk := connection.recv(4096):
            data += chunk
        if PAYLOAD not in data:
            raise RuntimeError('TCP payload missing')
    connection, relay = socks(proxy, 3, 0)
    with connection, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        udp.settimeout(5)
        packet = b'\x00\x00\x00\x01\x7f\x00\x00\x01' + struct.pack('!H', 23546) + PAYLOAD
        udp.sendto(packet, (relay[0] if relay[0] != '0.0.0.0' else '127.0.0.1', relay[1]))
        data, _ = udp.recvfrom(4096)
        if not data.endswith(PAYLOAD):
            raise RuntimeError('UDP payload missing')


def eventually(proxy, check):
    deadline = time.monotonic() + 15
    while True:
        check()
        try:
            traffic(proxy)
            return
        except (OSError, RuntimeError):
            if time.monotonic() > deadline:
                raise
            time.sleep(.2)


def refused(proxy, check):
    # A refused SS 2022 identity is dropped by the server; the client then
    # reports EOF or a timeout. Repeat to rule out a transient start-up race.
    for _ in range(3):
        check()
        try:
            traffic(proxy)
        except (OSError, RuntimeError):
            time.sleep(.5)
            continue
        return False
    return True


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(PAYLOAD)

    def log_message(self, *_):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--runtime', required=True)
    parser.add_argument('--fixture', type=Path, required=True)
    args = parser.parse_args()
    fixture = json.loads(args.fixture.read_text())
    web = http.server.ThreadingHTTPServer(('127.0.0.1', 23545), Handler)
    threading.Thread(target=web.serve_forever, daemon=True).start()
    echo = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    echo.bind(('127.0.0.1', 23546))

    def udp_echo():
        while True:
            try:
                payload, address = echo.recvfrom(4096)
                echo.sendto(payload, address)
            except OSError:
                return

    threading.Thread(target=udp_echo, daemon=True).start()
    processes = []
    result = {}
    try:
        with tempfile.TemporaryDirectory(prefix='apsp-ss-test-') as directory:
            root = Path(directory)
            for name, config in fixture.items():
                (root / (name + '.json')).write_text(json.dumps(config))
                subprocess.run([args.runtime, 'check', '-c', str(root / (name + '.json'))], check=True, capture_output=True)
            hashes = {name: hashlib.sha256((root / (name + '.json')).read_bytes()).hexdigest() for name in CLIENTS}

            def launch(name):
                process = subprocess.Popen([args.runtime, 'run', '-c', str(root / (name + '.json'))],
                                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                processes.append(process)
                return process

            clients = [launch(name) for name in CLIENTS]
            for phase in ['before', 'after']:
                server = launch(phase)

                def alive():
                    if server.poll() is not None or any(c.poll() is not None for c in clients):
                        raise SystemExit('Fixture runtime exited')

                eventually(CLIENTS['client'], alive)
                result[f'{phase}_kept_tcp_udp'] = True
                if phase == 'before':
                    eventually(CLIENTS['revoked_client'], alive)
                    result['before_revoked_tcp_udp'] = True
                else:
                    result['after_revoked_refused'] = refused(CLIENTS['revoked_client'], alive)
                for name, digest in hashes.items():
                    if hashlib.sha256((root / (name + '.json')).read_bytes()).hexdigest() != digest:
                        raise SystemExit('Client configuration changed')
                server.terminate()
                server.wait(timeout=10)
            result['clients_unchanged'] = True
    finally:
        for process in processes:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
        web.shutdown()
        web.server_close()
        echo.close()
    print(json.dumps(result))
    if not all(result.values()) or len(result) != 5:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
