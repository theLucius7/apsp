#!/usr/bin/env python3
"""Test authenticated connection attribution and targeted deletion on loopback."""
import argparse
import contextlib
import importlib.util
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
import urllib.request
import uuid

spec = importlib.util.spec_from_file_location('limits_fixture', Path(__file__).with_name('test-user-limits.py'))
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve()
    with fixture.TCPServer(('127.0.0.1', 0), fixture.TCPHandler) as echo, tempfile.TemporaryDirectory(prefix='apsp-session-') as temporary:
        threading.Thread(target=echo.serve_forever, daemon=True).start()
        inbound, controller = fixture.port(), fixture.port()
        secret = 'TEST_ONLY-local-controller-secret'
        config = {'log': {'level': 'error'}, 'inbounds': [{'type': 'vless', 'tag': 'node-1', 'listen': '127.0.0.1', 'listen_port': inbound, 'users': [{'name': f'u{user}_n1', 'uuid': str(uuid.UUID(int=user))} for user in (1, 2)]}], 'outbounds': [{'type': 'direct', 'tag': 'direct'}], 'experimental': {'clash_api': {'external_controller': f'127.0.0.1:{controller}', 'secret': secret}}}
        path = Path(temporary) / 'config.json'
        path.write_text(json.dumps(config))
        def request(path='/connections', method='GET'):
            req = urllib.request.Request(f'http://127.0.0.1:{controller}{path}', method=method, headers={'Authorization': 'Bearer ' + secret})
            with urllib.request.urlopen(req, timeout=5) as response:
                return response.read()
        with (Path(temporary) / 'runtime.log').open('w+') as log:
            process = subprocess.Popen([str(binary), 'run', '-c', str(path)], stdout=log, stderr=log)
            try:
                for _ in range(100):
                    if process.poll() is not None:
                        log.seek(0)
                        raise RuntimeError(log.read()[-4000:])
                    try:
                        request()
                        break
                    except OSError:
                        time.sleep(.05)
                with fixture.connect(inbound, echo.server_address[1], 1) as first, fixture.connect(inbound, echo.server_address[1], 2) as second:
                    snapshot = json.loads(request())
                    assert snapshot['apsp_authenticated_sessions'] is True
                    entries = {row['metadata']['apsp_principal']: row for row in snapshot['connections']}
                    assert set(entries) == {'u1_n1', 'u2_n1'}
                    assert entries['u1_n1']['metadata']['sourceIP'] == entries['u2_n1']['metadata']['sourceIP']
                    request('/connections/' + entries['u1_n1']['id'], 'DELETE')
                    first.settimeout(2)
                    try:
                        assert first.recv(1) == b''
                    except (ConnectionResetError, BrokenPipeError):
                        pass
                    second.sendall(b'other-user-remains')
                    assert fixture.read(second, 18) == b'other-user-remains'
                    remaining = json.loads(request())['connections']
                    assert all(row['metadata']['apsp_principal'] != 'u1_n1' for row in remaining)
                    print(json.dumps({'authenticated_identity': True, 'shared_nat_isolation': True, 'targeted_disconnect': True}))
            finally:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
                echo.shutdown()

if __name__ == '__main__':
    main()
