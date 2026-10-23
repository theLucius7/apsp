#!/usr/bin/env python3
"""Private API fixtures; no real credentials or external requests are used."""
import importlib.util
import http.server
import ipaddress
import json
import multiprocessing
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
PLUGIN = ROOT / 'plugins/nodequality'
spec = importlib.util.spec_from_file_location('official_ip', PLUGIN / 'official-ip.py')
official = importlib.util.module_from_spec(spec)
spec.loader.exec_module(official)
FIXTURE_KEY = 'TEST_ONLY_OPERATOR_KEY'


class Response:
    def __init__(self, value, status=200):
        self.status = status
        self.body = json.dumps(value).encode() if not isinstance(value, bytes) else value

    def read(self, size):
        return self.body[:size]


class Connection:
    def __init__(self, response):
        self.response = response
        self.requests = []
        self.closed = False

    def request(self, method, path, headers):
        self.requests.append((method, path, headers))

    def getresponse(self):
        return self.response

    def close(self):
        self.closed = True


class Sender:
    def __init__(self):
        self.value = None

    def send(self, value):
        self.value = value

    def close(self):
        pass


def parked_child(provider, key, family, sender):
    time.sleep(30)


class OfficialNodeIpTests(unittest.TestCase):
    def ipregistry(self, family=4):
        return {'ip': '198.51.100.10' if family == 4 else '2001:db8::10',
                'type': 'IPv' + str(family), 'connection': {'type': 'hosting'},
                'company': {'type': 'business'},
                'security': {'is_proxy': False, 'is_tor': False, 'is_vpn': True},
                'user_agent': {'header': FIXTURE_KEY}}

    def dbip(self, family=4):
        return {'ipAddress': '198.51.100.10' if family == 4 else '2001:db8::10',
                'countryCode': 'US', 'countryName': 'Example', 'asNumber': 1,
                'isProxy': False, 'isCrawler': False,
                'unused_secret_metadata': FIXTURE_KEY}

    def parse(self, provider, value, family=4):
        with mock.patch.object(ipaddress.IPv4Address, 'is_global', new_callable=mock.PropertyMock, return_value=True), \
                mock.patch.object(ipaddress.IPv6Address, 'is_global', new_callable=mock.PropertyMock, return_value=True):
            return official.parse_response(provider, value, family)

    def test_official_fields_keep_false_and_zero_is_not_an_asn(self):
        for provider, value in (('ipregistry', self.ipregistry()), ('dbip', self.dbip())):
            address, fields = self.parse(provider, value)
            self.assertEqual(address, '198.51.100.10')
            self.assertIs(fields['代理'], False)
            self.assertNotIn(FIXTURE_KEY, json.dumps(fields))
        value = self.dbip()
        value['asNumber'] = 0
        with self.assertRaises(ValueError):
            self.parse('dbip', value)

    def test_family_public_status_schema_and_boolean_types_fail_closed(self):
        for change in ({'type': 'IPv6'}, {'success': False}, {'status': 'failed'},
                       {'error': 'denied'}, {'errors': ['denied']},
                       {'company': []}, {'security': {'is_proxy': 'false'}}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.parse('ipregistry', self.ipregistry() | change)
        for value in ('127.0.0.1', '198.51.100.10', '::ffff:198.51.100.10', '224.0.0.1'):
            with self.subTest(value=value), self.assertRaises(ValueError):
                official.parse_response('ipregistry', self.ipregistry() | {'ip': value}, 4)
        with self.assertRaises(ValueError):
            self.parse('dbip', self.dbip(), 6)

    def test_duplicate_keys_nonfinite_values_and_no_fields_are_invalid(self):
        for content in (b'{"success":false,"success":true}', b'{"score":NaN}', b'{"score":Infinity}'):
            with self.assertRaises(ValueError):
                official.decode(content)
        with self.assertRaises(ValueError):
            self.parse('dbip', {'ipAddress': '198.51.100.10'})

    def child(self, provider, response, family=4):
        connection, sender = Connection(response), Sender()
        with mock.patch.object(official.http.client, 'HTTPSConnection', return_value=connection) as factory, \
                mock.patch.object(official.ssl, 'create_default_context', return_value='TLS_TEST_ONLY'), \
                mock.patch.object(official.socket, 'getaddrinfo'), \
                mock.patch.object(ipaddress.IPv4Address, 'is_global', new_callable=mock.PropertyMock, return_value=True), \
                mock.patch.object(ipaddress.IPv6Address, 'is_global', new_callable=mock.PropertyMock, return_value=True):
            official.request_child(provider, FIXTURE_KEY, family, sender)
        self.assertTrue(connection.closed)
        self.assertEqual(factory.call_args.args[0], official.PROVIDERS[provider][2])
        return sender.value, connection.requests

    def test_node_self_urls_are_fixed_and_outputs_never_contain_access_material(self):
        for provider, payload in (('ipregistry', self.ipregistry()), ('dbip', self.dbip(6))):
            family = 4 if provider == 'ipregistry' else 6
            value, requests = self.child(provider, Response(payload), family)
            self.assertEqual(value['status'], 'success')
            self.assertEqual(value['execution'], 'node_self')
            self.assertEqual(value['ip_version'], family)
            self.assertNotIn(FIXTURE_KEY, json.dumps(value))
            self.assertNotIn(FIXTURE_KEY, official.render([value]))
            self.assertEqual(len(requests), 1)
            method, path, headers = requests[0]
            self.assertEqual(method, 'GET')
            self.assertIn(FIXTURE_KEY, path)
            self.assertTrue(path.startswith('/?') if provider == 'ipregistry' else path.endswith('/self'))
            self.assertNotIn('Cookie', headers)
            self.assertEqual(headers['User-Agent'], 'ApspNodeIp/1')

    def test_reflected_key_in_whitelisted_text_fails_without_reflection(self):
        payload = self.dbip() | {'countryName': FIXTURE_KEY}
        value, _ = self.child('dbip', Response(payload))
        self.assertEqual(value['error_kind'], 'schema_mismatch')
        self.assertNotIn(FIXTURE_KEY, json.dumps(value))

    def test_forbidden_rate_limit_redirect_and_oversize_are_not_success_or_retried(self):
        for status, kind in ((401, 'forbidden'), (403, 'forbidden'), (429, 'rate_limited'),
                             (302, 'redirect_rejected'), (500, 'http_error')):
            value, requests = self.child('dbip', Response({'error': FIXTURE_KEY}, status))
            self.assertEqual(value['error_kind'], kind)
            self.assertEqual(value['status'], 'failed')
            self.assertEqual(len(requests), 1)
            self.assertEqual(value['fields'], {})
            self.assertNotIn(FIXTURE_KEY, json.dumps(value))
        value, _ = self.child('dbip', Response(b'x' * (official.MAX_RESPONSE + 1)))
        self.assertEqual(value['error_kind'], 'response_too_large')
        for code, kind in (('INVALID_KEY', 'forbidden'), ('EXPIRED', 'forbidden'),
                           ('RESTRICTED', 'forbidden'), ('OVER_QUERY_LIMIT', 'rate_limited')):
            value, requests = self.child('dbip', Response({'errorCode': code, 'error': FIXTURE_KEY}))
            self.assertEqual(value['error_kind'], kind)
            self.assertEqual(len(requests), 1)
            self.assertNotIn(FIXTURE_KEY, json.dumps(value))

    def test_current_native_failure_does_not_log_request_url_or_key(self):
        sender = Sender()
        with mock.patch.object(official.http.client, 'HTTPSConnection', side_effect=OSError(FIXTURE_KEY)), \
                mock.patch.object(official.ssl, 'create_default_context'), \
                mock.patch.object(official.socket, 'getaddrinfo'):
            official.request_child('ipregistry', FIXTURE_KEY, 4, sender)
        self.assertEqual(sender.value['error_kind'], 'transport_error')
        self.assertNotIn(FIXTURE_KEY, json.dumps(sender.value))

    def test_missing_invalid_disabled_configuration_does_not_spawn_any_query(self):
        for config, failed in (({}, False), ({'ipregistry': {'enabled': False}}, False),
                               ({'ipregistry': {'enabled': True, 'api_key': FIXTURE_KEY}}, True)):
            with mock.patch.object(official, 'read_config', return_value=config), \
                    mock.patch.object(official, 'query') as query:
                values = official.collect('both')
            query.assert_not_called()
            self.assertEqual(len(values), 4)
            self.assertTrue(all(not value['attempted'] for value in values))
            self.assertEqual(values[0]['error_kind'], 'credential_invalid' if failed else 'credential_not_configured')
        with mock.patch.object(official, 'read_config', side_effect=PermissionError(FIXTURE_KEY)), \
                mock.patch.object(official, 'query') as query:
            values = official.collect('ipv4')
        query.assert_not_called()
        self.assertTrue(all(row['error_kind'] == 'credential_invalid' for row in values))

    def test_private_credential_fd_permissions_hardlink_and_symlink_are_checked(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            root.chmod(0o700)
            path = root / 'keys.json'
            config = {'schema': 1, 'providers': {'dbip': {'enabled': True,
                      'operator_owned_credentials': True, 'api_key': FIXTURE_KEY}}}
            path.write_text(json.dumps(config))
            path.chmod(0o600)
            self.assertEqual(official.read_config(path, os.geteuid()), config['providers'])
            self.assertEqual(official.credential(config['providers'], 'dbip'), FIXTURE_KEY)
            path.chmod(0o644)
            with self.assertRaises(ValueError):
                official.read_config(path, os.geteuid())
            path.chmod(0o600)
            os.link(path, root / 'alias')
            with self.assertRaises(ValueError):
                official.read_config(path, os.geteuid())
            (root / 'alias').unlink()
            alias = root / 'linked.json'
            alias.symlink_to(path)
            with self.assertRaises(OSError):
                official.read_config(alias, os.geteuid())
            directory_alias = root / 'linked-directory'
            directory_alias.symlink_to(root, target_is_directory=True)
            with self.assertRaises(OSError):
                official.read_config(directory_alias / path.name, os.geteuid())
            root.chmod(0o777)
            with self.assertRaises(ValueError):
                official.read_config(path, os.geteuid())
            root.chmod(0o700)
            with self.assertRaises(ValueError):
                official.read_config(path, os.geteuid() + 1)

    def test_real_loopback_connections_force_requested_family_without_proxy_or_redirect(self):
        if 'fork' not in multiprocessing.get_all_start_methods():
            self.skipTest('official node helper requires the Linux fork runtime')
        # Only TLS wrapping is a private substitute. The production HTTP client,
        # family filter, sockets, HTTP bytes, subprocess and parent cutoff are real.
        class PrivateTls:
            check_hostname = True
            verify_mode = official.ssl.CERT_REQUIRED

            def wrap_socket(self, connection, server_hostname):
                if server_hostname != 'api.db-ip.com':
                    raise ValueError('unexpected TLS server name')
                return connection

        for family in (4, 6):
            with self.subTest(family=family):
                observed = []
                payload = self.dbip(family)

                class Handler(http.server.BaseHTTPRequestHandler):
                    def do_GET(self):
                        observed.append((self.path, self.headers.get('Host'), self.headers.get('Cookie')))
                        content = json.dumps(payload).encode()
                        self.send_response(200)
                        self.send_header('Content-Length', str(len(content)))
                        self.end_headers()
                        self.wfile.write(content)

                    def log_message(self, *args):
                        pass

                kind = socket.AF_INET if family == 4 else socket.AF_INET6

                class Server(http.server.HTTPServer):
                    address_family = kind

                try:
                    server = Server(('127.0.0.1' if family == 4 else '::1', 0), Handler)
                except OSError:
                    if family == 6:
                        self.skipTest('IPv4 fixture completed; this host cannot bind an IPv6 loopback listener')
                    raise
                worker = threading.Thread(target=server.serve_forever, daemon=True)
                worker.start()
                address = server.server_address

                def fixture_dns(host, port, requested_family, sock_type):
                    if host != 'api.db-ip.com' or port != 443 or requested_family != kind:
                        raise ValueError('unexpected target or DNS address family')
                    return [(kind, sock_type, socket.IPPROTO_TCP, '', address)]

                try:
                    with mock.patch.object(official.socket, 'getaddrinfo', side_effect=fixture_dns), \
                            mock.patch.object(official.ssl, 'create_default_context', return_value=PrivateTls()), \
                            mock.patch.object(ipaddress.IPv4Address, 'is_global', new_callable=mock.PropertyMock, return_value=True), \
                            mock.patch.object(ipaddress.IPv6Address, 'is_global', new_callable=mock.PropertyMock, return_value=True), \
                            mock.patch.dict(os.environ, HTTPS_PROXY='http://127.0.0.1:1', HTTP_PROXY='http://127.0.0.1:1'):
                        row = official.query('dbip', FIXTURE_KEY, family, time.monotonic() + 4)
                    self.assertEqual(row['status'], 'success', row)
                    self.assertEqual(row['ip_version'], family)
                    self.assertEqual(observed, [('/v2/' + FIXTURE_KEY + '/self', 'api.db-ip.com', None)])
                    self.assertNotIn(FIXTURE_KEY, json.dumps(row))
                finally:
                    server.shutdown()
                    worker.join(timeout=1)
                    server.server_close()

    def test_hard_deadline_reaps_a_child_blocked_in_dns(self):
        if 'fork' not in multiprocessing.get_all_start_methods():
            self.skipTest('official node helper requires the Linux fork runtime')
        before = {process.pid for process in multiprocessing.active_children()}
        started = time.monotonic()
        with mock.patch.object(official, 'request_child', parked_child), \
                mock.patch.object(official, 'QUERY_SECONDS', 0.15):
            value = official.query('ipregistry', FIXTURE_KEY, 4, time.monotonic() + 1)
        self.assertEqual(value['error_kind'], 'timeout')
        self.assertLess(time.monotonic() - started, 1)
        self.assertEqual({process.pid for process in multiprocessing.active_children()}, before)

    def test_daily_report_keeps_protocol_chapter_shape_and_old_success_unchanged(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            previous, workspace, runtime = root / 'old-job', root / 'new-job', root / 'runtime'
            for path in (previous, workspace, runtime):
                path.mkdir()
            (previous / 'result.txt').write_text('Prior trusted success stays historical')
            (workspace / 'daily-targets.json').write_text('[]')
            (runtime / 'daily.py').write_bytes((PLUGIN / 'daily.py').read_bytes())
            failure = official.result('dbip', 4, 'rate_limited', True, status=429)
            (runtime / 'official-ip.py').write_text(
                'import json\nROWS = json.loads(' + repr(json.dumps([failure])) + ')\n'
                'def collect(version):\n    return ROWS\n'
                'def render(rows):\n    return "正式节点DB-IP：本次429未知，旧成功仍在历史\\n"\n')
            run = subprocess.run([sys.executable, str(runtime / 'daily.py'), str(workspace),
                                  str(workspace / 'daily-targets.json'), 'ipv4'],
                                 capture_output=True, text=True, timeout=3)
            self.assertEqual(run.returncode, 0, run.stderr)
            chapter = json.loads((workspace / 'section-net_quality.json').read_text())
            self.assertEqual(set(chapter), {'name', 'text', 'complete', 'revision', 'collected_at'})
            self.assertEqual(chapter['name'], 'net_quality')
            self.assertIn('429未知', chapter['text'])
            self.assertEqual(json.loads((workspace / 'node-ip-sources.json').read_text()), [failure])
            self.assertEqual((workspace / 'node-ip-sources.json').stat().st_mode & 0o777, 0o600)
            self.assertEqual((previous / 'result.txt').read_text(), 'Prior trusted success stays historical')


if __name__ == '__main__':
    unittest.main()
