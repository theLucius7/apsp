#!/usr/bin/env python3
"""Run real ACME challenge/renewal tests against an isolated local Pebble CA."""
import argparse
import json
import os
from pathlib import Path
import socket
import ssl
import subprocess
import tempfile
import time
import urllib.request


def port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--pebble-source", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    args = parser.parse_args()
    source = args.pebble_source.resolve()
    root = Path(__file__).resolve().parents[1]
    processes = []
    with tempfile.TemporaryDirectory(prefix="apsp-acme-") as temporary:
        directory = Path(temporary)
        api, management, dns, dns_management, http, tls = [port() for _ in range(6)]
        config = json.loads((source / "test/config/pebble-config.json").read_text())
        config["pebble"].update({
            "listenAddress": f"127.0.0.1:{api}",
            "managementListenAddress": f"127.0.0.1:{management}",
            "httpPort": http, "tlsPort": tls,
            "profiles": {"default": {"description": "TEST_ONLY short-lived certificates", "validityPeriod": 60}},
        })
        config_path = directory / "pebble.json"
        config_path.write_text(json.dumps(config))
        env = dict(os.environ, NO_PROXY="127.0.0.1,localhost,::1", no_proxy="127.0.0.1,localhost,::1",
                   PEBBLE_VA_NOSLEEP="1", PEBBLE_WFE_NONCEREJECT="0", PEBBLE_AUTHZREUSE="0")
        env.pop("PEBBLE_VA_ALWAYS_VALID", None)
        log_path = directory / "pebble.log"
        try:
            with log_path.open("wb") as log:
                processes.append(subprocess.Popen([
                    str(source / "bin/pebble-challtestsrv"), "-http01=", "-https01=", "-tlsalpn01=", "-doh=",
                    f"-dnsserver=127.0.0.1:{dns}", f"-management=127.0.0.1:{dns_management}",
                    "-defaultIPv4=127.0.0.1", "-defaultIPv6=::1",
                ], stdout=log, stderr=log, cwd=source, env=env))
                processes.append(subprocess.Popen([
                    str(source / "bin/pebble"), "-config", str(config_path), "-dnsserver", f"127.0.0.1:{dns}",
                ], stdout=log, stderr=log, cwd=source, env=env))
            bootstrap = source / "test/certs/pebble.minica.pem"
            context = ssl.create_default_context(cafile=str(bootstrap))
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), urllib.request.HTTPSHandler(context=context))
            deadline = time.monotonic() + 20
            while True:
                try:
                    with opener.open(f"https://127.0.0.1:{management}/roots/0", timeout=1) as response:
                        issuer = response.read()
                    break
                except OSError:
                    if time.monotonic() >= deadline or any(process.poll() is not None for process in processes):
                        raise RuntimeError("local Pebble failed to start") from None
                    time.sleep(0.1)
            issuer_path = directory / "issuer.pem"
            issuer_path.write_bytes(issuer)
            env.update(APSP_TEST_SINGBOX=str(args.runtime.resolve()),
                       APSP_TEST_ACME_URL=f"https://127.0.0.1:{api}/dir",
                       APSP_TEST_ACME_CA=str(bootstrap), APSP_TEST_ACME_ISSUER=str(issuer_path),
                       APSP_TEST_ACME_HTTP_PORT=str(http), APSP_TEST_ACME_TLS_PORT=str(tls))
            subprocess.run(["cargo", "test", "-p", "apsp-panel", "--test", "acme_runtime", "--", "--ignored", "--nocapture"],
                           cwd=root, env=env, check=True, timeout=240)
        except Exception:
            print(log_path.read_text(errors="replace"))
            raise
        finally:
            for process in reversed(processes):
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    main()
