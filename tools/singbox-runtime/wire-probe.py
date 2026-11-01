#!/usr/bin/env python3
"""Compare how endpoints respond to unauthenticated probes.

Point it at a REALITY inbound and at that inbound's real target. Any difference
in close type, timing or record lengths is something a censor can measure too.
Only run it against endpoints you operate plus their public target.

    wire-probe.py --target-sni example.com \
        --endpoint inbound=192.0.2.10:8443 --endpoint target=192.0.2.20:443
"""
import argparse
import json
import os
import socket
import ssl
import threading
import time

# Every endpoint must receive identical bytes for the comparison to mean anything.
RANDOM_BYTES = os.urandom(64)
SCENARIOS = ("random64", "http_plain", "idle", "partial_hello", "hello", "hello_other_sni", "hello_tls12", "hello_then_fin")


def records(buffer):
    found, index = [], 0
    while index + 5 <= len(buffer):
        length = int.from_bytes(buffer[index + 3:index + 5], "big")
        found.append((buffer[index], length))
        index += 5 + length
    return found


def client_hello(server_name, tls12=False):
    context = ssl.create_default_context()
    context.check_hostname = False
    context.verify_mode = ssl.CERT_NONE
    if tls12:
        context.maximum_version = ssl.TLSVersion.TLSv1_2
    incoming, outgoing = ssl.MemoryBIO(), ssl.MemoryBIO()
    connection = context.wrap_bio(incoming, outgoing, server_hostname=server_name)
    try:
        connection.do_handshake()
    except ssl.SSLWantReadError:
        pass
    return outgoing.read()


def observe(connection, wait):
    started, data = time.time(), b""
    connection.settimeout(wait)
    try:
        while True:
            chunk = connection.recv(65536)
            if not chunk:
                return data, "FIN", time.time() - started
            data += chunk
            connection.settimeout(max(0.1, wait - (time.time() - started)))
    except ConnectionResetError:
        return data, "RST", time.time() - started
    except socket.timeout:
        return data, "open", time.time() - started


def probe(scenario, address, server_name, wait):
    try:
        connection = socket.create_connection(address, timeout=10)
        connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        if scenario == "random64":
            connection.sendall(RANDOM_BYTES)
        elif scenario == "http_plain":
            connection.sendall(b"GET / HTTP/1.1\r\nHost: %s\r\n\r\n" % server_name.encode())
        elif scenario == "partial_hello":
            connection.sendall(client_hello(server_name)[:40])
        elif scenario == "hello":
            connection.sendall(client_hello(server_name))
        elif scenario == "hello_other_sni":
            connection.sendall(client_hello("nonexistent.example.com"))
        elif scenario == "hello_tls12":
            connection.sendall(client_hello(server_name, True))
        elif scenario == "hello_then_fin":
            connection.sendall(client_hello(server_name))
            time.sleep(1)
            connection.shutdown(socket.SHUT_WR)
        data, how, elapsed = observe(connection, wait)
        return {"close": how, "seconds": round(elapsed, 1), "records": records(data)[:8]}
    except OSError as error:
        return {"error": type(error).__name__}


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--target-sni", required=True)
    parser.add_argument("--endpoint", action="append", required=True, metavar="NAME=HOST:PORT")
    parser.add_argument("--wait", type=int, default=45)
    parser.add_argument("--scenario", action="append", choices=SCENARIOS)
    arguments = parser.parse_args()
    endpoints = {}
    for entry in arguments.endpoint:
        name, _, address = entry.partition("=")
        host, _, port = address.rpartition(":")
        endpoints[name] = (host.strip("[]"), int(port))
    results, threads = {}, []

    def one(name, address, scenario):
        results[name, scenario] = probe(scenario, address, arguments.target_sni, arguments.wait)

    scenarios = arguments.scenario or SCENARIOS
    for scenario in scenarios:
        for name, address in endpoints.items():
            thread = threading.Thread(target=one, args=(name, address, scenario))
            thread.start()
            threads.append(thread)
    for thread in threads:
        thread.join()
    for scenario in scenarios:
        print(json.dumps({"scenario": scenario, **{name: results[name, scenario] for name in endpoints}}))


if __name__ == "__main__":
    main()
