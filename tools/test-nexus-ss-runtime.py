#!/usr/bin/env python3
"""Use one unchanged legacy SS client before and after the configuration rewrite.

Run in an isolated container with a previously verified sing-box executable and
the JSON emitted by the preserved_runtime_fixture compiler example.
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


def receive(connection, size):
    result = b''
    while len(result) < size:
        chunk = connection.recv(size - len(result))
        if not chunk:
            raise RuntimeError('Unexpected proxy EOF')
        result += chunk
    return result


def socks(command, port):
    connection = socket.create_connection(('127.0.0.1', 23544), timeout=5)
    connection.sendall(b'\x05\x01\x00')
    assert receive(connection, 2) == b'\x05\x00'
    connection.sendall(b'\x05' + bytes([command]) + b'\x00\x01\x7f\x00\x00\x01' + struct.pack('!H', port))
    header = receive(connection, 4)
    assert header[:2] == b'\x05\x00'
    address = receive(connection, {1: 4, 4: 16}[header[3]])
    remote_port = struct.unpack('!H', receive(connection, 2))[0]
    return connection, (socket.inet_ntop(socket.AF_INET if header[3] == 1 else socket.AF_INET6, address), remote_port)


def traffic():
    connection, _ = socks(1, 23545)
    with connection:
        connection.sendall(b'GET / HTTP/1.0\r\nHost: fixture.invalid\r\n\r\n')
        data = b''
        while chunk := connection.recv(4096):
            data += chunk
        assert PAYLOAD in data
    connection, relay = socks(3, 0)
    with connection, socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        udp.settimeout(5)
        packet = b'\x00\x00\x00\x01\x7f\x00\x00\x01' + struct.pack('!H', 23546) + PAYLOAD
        udp.sendto(packet, (relay[0] if relay[0] != '0.0.0.0' else '127.0.0.1', relay[1]))
        data, _ = udp.recvfrom(4096)
        assert data.endswith(PAYLOAD)


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
    http = http.server.ThreadingHTTPServer(('127.0.0.1', 23545), Handler)
    threading.Thread(target=http.serve_forever, daemon=True).start()
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
    try:
        with tempfile.TemporaryDirectory(prefix='apsp-ss-test-') as directory:
            root = Path(directory)
            for name, config in fixture.items():
                (root / (name + '.json')).write_text(json.dumps(config))
                subprocess.run([args.runtime, 'check', '-c', str(root / (name + '.json'))], check=True, capture_output=True)
            client_hash = hashlib.sha256((root / 'client.json').read_bytes()).hexdigest()

            def launch(name):
                process = subprocess.Popen([args.runtime, 'run', '-c', str(root / (name + '.json'))], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                processes.append(process)
                return process

            client = launch('client')
            for phase in ['before', 'after']:
                server = launch(phase)
                deadline = time.monotonic() + 15
                while True:
                    if server.poll() is not None or client.poll() is not None:
                        raise RuntimeError('Fixture runtime exited')
                    try:
                        traffic()
                        break
                    except (OSError, AssertionError, RuntimeError):
                        if time.monotonic() > deadline:
                            raise
                        time.sleep(.2)
                assert hashlib.sha256((root / 'client.json').read_bytes()).hexdigest() == client_hash
                server.terminate()
                server.wait(timeout=10)
            print(json.dumps({'legacy_tcp': True, 'legacy_udp': True, 'rewritten_tcp': True, 'rewritten_udp': True, 'client_unchanged': True}))
    finally:
        for process in processes:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
        http.shutdown()
        http.server_close()
        echo.close()


if __name__ == '__main__':
    main()
