#!/usr/bin/env python3
"""Exercise real authenticated TCP/UDP sessions against the local patched runtime."""
import argparse
import concurrent.futures
import contextlib
import json
from pathlib import Path
import socket
import socketserver
import struct
import sys
import subprocess
import tempfile
import threading
import time
import uuid


class TCPHandler(socketserver.BaseRequestHandler):
    def handle(self):
        try:
            while data := self.request.recv(65536):
                self.request.sendall(data)
        except OSError:
            pass


class UDPHandler(socketserver.BaseRequestHandler):
    def handle(self):
        data, conn = self.request
        conn.sendto(data, self.client_address)


class TCPServer(socketserver.ThreadingTCPServer):
    daemon_threads = True


class UDPServer(socketserver.ThreadingUDPServer):
    daemon_threads = True


def read(conn, count):
    result = bytearray()
    while len(result) < count:
        data = conn.recv(count-len(result))
        if not data:
            raise ConnectionError('unexpected connection close')
        result.extend(data)
    return bytes(result)


def port():
    with socket.socket() as conn:
        conn.bind(('127.0.0.1', 0))
        return conn.getsockname()[1]


def connect(inbound, target, user=1, udp=False):
    conn = socket.create_connection(('127.0.0.1', inbound), timeout=5)
    conn.settimeout(5)
    packet = b'\0' + uuid.UUID(int=user).bytes + bytes([0, 2 if udp else 1]) + struct.pack('!H', target) + bytes([1,127,0,0,1])
    payload = b'hello'
    conn.sendall(packet + (struct.pack('!H', len(payload)) if udp else b'') + payload)
    try:
        assert read(conn, 2) == b'\0\0'
        if udp:
            assert read(conn, 2) == struct.pack('!H', len(payload))
        assert read(conn, len(payload)) == payload
        return conn
    except BaseException:
        conn.close()
        raise


def transfer(inbound, target, amount):
    with connect(inbound, target) as conn:
        payload = b'x' * amount
        with concurrent.futures.ThreadPoolExecutor() as pool:
            sender = pool.submit(conn.sendall, payload)
            received = read(conn, amount)
            sender.result()
        assert received == payload


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--direction', choices=['both','upload','download'], default='both')
    args = parser.parse_args()
    binary = args.binary.resolve()
    with TCPServer(('127.0.0.1', 0), TCPHandler) as tcp, UDPServer(('127.0.0.1', 0), UDPHandler) as udp, tempfile.TemporaryDirectory(prefix='apsp-limits-') as temporary:
        for server in (tcp, udp):
            threading.Thread(target=server.serve_forever, daemon=True).start()
        ports = [port(), port()]
        limits = [{'principals':['u1_n1','u1_n2'], 'max_connections':2, 'upload_bytes_per_second':0 if args.direction=='download' else 262144, 'download_bytes_per_second':0 if args.direction=='upload' else 262144}]
        config = {'log':{'level':'error'}, 'inbounds':[{'type':'vless', 'tag':f'node-{i}', 'listen':'127.0.0.1','listen_port':p,
            'users':[{'name':f'u{u}_n{i}', 'uuid':str(uuid.UUID(int=u))} for u in (1,2)]} for i,p in enumerate(ports,1)],
            'outbounds':[{'type':'direct','tag':'direct'}], 'route':{'apsp_user_limits':limits}}
        path = Path(temporary)/'config.json'
        path.write_text(json.dumps(config))
        subprocess.run([str(binary),'check','-c',str(path)],check=True,capture_output=True)
        with (Path(temporary)/'runtime.log').open('w+') as log:
            process = subprocess.Popen([str(binary),'run','-c',str(path)],stdout=log,stderr=log)
            try:
                for _ in range(100):
                    if process.poll() is not None:
                        log.seek(0)
                        raise RuntimeError(log.read()[-4000:])
                    try:
                        with socket.create_connection(('127.0.0.1',ports[0]),timeout=.1): pass
                        break
                    except OSError: time.sleep(.05)
                target = tcp.server_address[1]
                with connect(ports[0],target) as first, connect(ports[1],udp.server_address[1],udp=True) as packet:
                    try:
                        escaped=connect(ports[1],target)
                    except (OSError,ConnectionError): pass
                    else:
                        escaped.close()
                        raise AssertionError('TCP + UDP across two nodes escaped the limit')
                    with connect(ports[1],target,user=2): pass
                    first.sendall(b'still-active')
                    assert read(first,12)==b'still-active'
                    packet.sendall(struct.pack('!H',5)+b'again')
                    assert read(packet,7)==struct.pack('!H',5)+b'again'
                # Closed sessions release slots. UDP over VLESS has stream EOF.
                for _ in range(100):
                    try:
                        restored=connect(ports[0],target)
                        restored.close()
                        break
                    except (OSError,ConnectionError): time.sleep(.05)
                else: raise AssertionError('closed sessions leaked slots')
                time.sleep(.15)
                start=time.monotonic()
                with concurrent.futures.ThreadPoolExecutor() as pool:
                    futures=[pool.submit(transfer,p,target,512*1024) for p in ports]
                    for future in futures: future.result()
                elapsed=time.monotonic()-start
                assert 3.5 <= elapsed <= 10, f'aggregate rate escaped or stalled: {elapsed:.3f}s'
                # Authentication failures never consume a budget.
                try:
                    escaped=connect(ports[0],target,user=999)
                except (OSError,ConnectionError): pass
                else:
                    escaped.close()
                    raise AssertionError('invalid credentials admitted')
                with connect(ports[0],target): pass
                print(json.dumps({'direction':args.direction,'tcp_udp_shared_cap':True,'cross_node_cap':True,'other_user_isolated':True,'released_slots':True,'invalid_auth_rejected':True,'aggregate_bytes':1048576,'transfer_seconds':round(elapsed,3)}))
            finally:
                if sys.exc_info()[0] is not None:
                    log.flush(); log.seek(0); print(log.read()[-4000:], file=sys.stderr)
                process.terminate()
                try: process.wait(timeout=5)
                except subprocess.TimeoutExpired: process.kill(); process.wait()
                tcp.shutdown(); udp.shutdown()


if __name__ == '__main__':
    main()
