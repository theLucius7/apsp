#!/usr/bin/env python3
"""Exercise Naive outbound with the bundled Cronet library over local trusted TLS."""
import argparse
import contextlib
import http.server
import json
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import threading
import time


def port():
    with socket.socket() as value:
        value.bind(('127.0.0.1', 0))
        return value.getsockname()[1]


def read(conn, count):
    data = b''
    while len(data) < count:
        chunk = conn.recv(count - len(data))
        if not chunk:
            raise ConnectionError('proxy closed before response')
        data += chunk
    return data


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b'TEST_ONLY Cronet local success')
    def log_message(self, *args):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    with tempfile.TemporaryDirectory(prefix='apsp-naive-') as temporary, http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler) as target:
        root = Path(temporary)
        cert, key = root / 'cert.pem', root / 'key.pem'
        subprocess.run(['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '1', '-keyout', str(key), '-out', str(cert), '-subj', '/CN=proxy.example.com', '-addext', 'subjectAltName=DNS:proxy.example.com'], check=True, capture_output=True)
        threading.Thread(target=target.serve_forever, daemon=True).start()
        inbound, proxy = port(), port()
        tls = {'enabled': True, 'server_name': 'proxy.example.com', 'certificate': cert.read_text(), 'key': key.read_text()}
        server = {'log': {'level': 'error'}, 'inbounds': [{'type': 'naive', 'tag': 'node-1', 'listen': '127.0.0.1', 'listen_port': inbound, 'network': 'tcp', 'tls': tls, 'users': [{'username': 'TEST_ONLY', 'password': 'TEST_ONLY-secret'}]}], 'outbounds': [{'type': 'direct', 'tag': 'direct'}]}
        client = {'log': {'level': 'error'}, 'inbounds': [{'type': 'socks', 'tag': 'local', 'listen': '127.0.0.1', 'listen_port': proxy}], 'outbounds': [{'type': 'naive', 'tag': 'proxy', 'server': '127.0.0.1', 'server_port': inbound, 'username': 'TEST_ONLY', 'password': 'TEST_ONLY-secret', 'tls': {'enabled': True, 'server_name': 'proxy.example.com', 'certificate': cert.read_text()}}]}
        with contextlib.ExitStack() as stack:
            processes = []
            try:
                for name, config, listen in [('server', server, inbound), ('client', client, proxy)]:
                    path = root / (name + '.json')
                    path.write_text(json.dumps(config))
                    log = stack.enter_context((root / (name + '.log')).open('w+'))
                    process = subprocess.Popen([str(binary), 'run', '-c', str(path)], stdout=log, stderr=log)
                    processes.append(process)
                    for _ in range(100):
                        if process.poll() is not None:
                            log.seek(0)
                            raise RuntimeError(log.read()[-3000:])
                        try:
                            with socket.create_connection(('127.0.0.1', listen), timeout=.1):
                                break
                        except OSError:
                            time.sleep(.05)
                with socket.create_connection(('127.0.0.1', proxy), timeout=10) as conn:
                    conn.settimeout(10)
                    conn.sendall(b'\x05\x01\x00')
                    assert read(conn, 2) == b'\x05\x00'
                    conn.sendall(b'\x05\x01\x00\x01\x7f\x00\x00\x01' + struct.pack('!H', target.server_port))
                    reply = read(conn, 4)
                    assert reply[:2] == b'\x05\x00', reply
                    read(conn, (4 if reply[3] == 1 else 16) + 2)
                    conn.sendall(b'GET / HTTP/1.0\r\nHost: local.example.com\r\n\r\n')
                    response = b''
                    while chunk := conn.recv(4096):
                        response += chunk
                    assert b'TEST_ONLY Cronet local success' in response
                print(json.dumps({'naive_outbound': True, 'cronet_loaded': True, 'local_tls_proxy_request': True}))
            finally:
                for process in processes:
                    process.terminate()
                for process in processes:
                    try:
                        process.wait(timeout=5)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait()
                target.shutdown()

if __name__ == '__main__':
    main()
