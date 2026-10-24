"""Snapshot conversion from the legacy builder's own output (ADR 0095).

The effective fixture was produced by tools/nexus-legacy-export against the
TEST_ONLY request and config.yml next to it, so weak port decoding, the SS
user-key derivation and custom outbounds are the legacy builder's behaviour.
"""
import base64
import copy
import json
from pathlib import Path
import stat
import tempfile
import unittest

import nexus_runtime_snapshot as snapshot

HERE = Path(__file__).parent / 'nexus-legacy-export'
UUID_A = '12345678-1234-4234-8234-123456789abc'
UUID_B = '87654321-4321-4321-8321-cba987654321'


def inventory():
    return {
        'source_id': 'fixture', 'hosts': [{'legacy_id': 15, 'server_id': 20}],
        'machines': [{'id': 30, 'host_id': 15, 'name': 'TEST_ONLY', 'is_active': 1}],
        'nodes': [
            {'id': 43, 'machine_id': 30, 'type': 'shadowsocks', 'name': 'TEST_ONLY SS', 'host': 'ss.example.invalid',
             'port': '51002', 'enabled': 1, 'protocol_settings': '{"cipher":"2022-blake3-aes-128-gcm"}'},
            {'id': 44, 'machine_id': 30, 'type': 'vless', 'name': 'TEST_ONLY VLESS', 'host': 'vless.example.invalid',
             'port': '443', 'enabled': 1,
             'protocol_settings': json.dumps({'reality_settings': {'public_key': 'AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI'}})},
        ],
    }


def effective():
    return json.loads((HERE / 'fixture-effective.json').read_text())


class SnapshotTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.root = Path(self.directory.name)

    def tearDown(self):
        self.directory.cleanup()

    def write_effective(self, value):
        (self.root / 'host-15-effective.json').write_text(json.dumps(value))

    def built(self):
        result = snapshot.build(inventory(), self.root)
        return result, json.loads((self.root / 'snapshot-summary.json').read_text())

    def test_request_matches_the_exporter_fixture(self):
        request = json.loads((HERE / 'fixture-request.json').read_text())
        legacy = {'nodes': [{'id': n['legacy_node_id'], 'configuration': n['node_config'], 'users': []}
                            for n in request['nodes']]}
        self.assertEqual(snapshot.requests(inventory(), legacy, self.root), {'hosts': 1})
        path = self.root / 'host-15-request.json'
        self.assertEqual(json.loads(path.read_text()), request)
        self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
        with self.assertRaises(snapshot.SnapshotError):
            snapshot.requests(inventory(), {'nodes': legacy['nodes'][:1]}, self.root)

    def test_builds_strict_snapshot_from_legacy_output(self):
        self.write_effective(effective())
        result, summary = self.built()
        self.assertEqual(result, {'hosts': 1, 'ready': 1, 'blocked_nodes': 0})
        self.assertTrue(summary['hosts'][0]['ready'])
        value = json.loads((self.root / 'host-15-snapshot.json').read_text())
        self.assertEqual((value['source_id'], value['server_id']), ('fixture', 20))
        ss, vless = (entry['preserved'] for entry in value['nodes'])
        self.assertEqual(ss['node']['protocol_config'], {
            'type': 'shadowsocks2022', 'method': '2022-blake3-aes-128-gcm',
            'password': base64.b64encode(b'0123456789abcdef').decode()})
        self.assertEqual(ss['node']['port'], 51002)
        self.assertEqual(ss['node']['settings'], {'listen': '::', 'public_port': 51002})
        node = vless['node']
        self.assertEqual((node['short_id'], node['sni'], node['public_key'][:4]), ('aabbccddeeff', 'handshake.example.invalid', 'AgIC'))
        self.assertEqual(node['settings']['reality'], {'handshake_server': 'handshake.example.invalid',
                                                       'handshake_port': 443, 'flow': 'vision'})
        self.assertEqual(node['settings']['public_port'], 443)
        self.assertEqual(node['users'], [])
        # The listener stays exactly as the legacy builder produced it.
        self.assertEqual(vless['configuration'], effective()[1]['configuration'])
        # crates/compiler/tests/preserved_runtime.rs compiles this same file.
        self.assertEqual(value, json.loads((HERE / 'fixture-snapshot.json').read_text()))

    def test_unconvertible_nodes_block_the_whole_host(self):
        cases = {
            'protocol_requires_conversion': lambda c: c['inbounds'][0].update(type='hysteria2'),
            'transport_requires_conversion': lambda c: c['inbounds'][0].update(transport={'type': 'ws'}),
            'global_settings_require_reconciliation': lambda c: c.update(experimental={'cache_file': {}}),
            'external_files': lambda c: c['outbounds'].append({'type': 'direct', 'tag': 'x', 'cache_path': '/x'}),
            'reality_short_id_count': lambda c: c['inbounds'][0]['tls']['reality'].update(short_id=['aa', 'bb']),
            'brutal_requires_review': lambda c: c['inbounds'][0]['multiplex'].update(brutal={'enabled': True}),
            'unexpected_user_list': lambda c: c['inbounds'][0].update(users=[]),
        }
        for reason, mutate in cases.items():
            value = effective()
            mutate(value[1]['configuration'])
            self.write_effective(value)
            result, summary = self.built()
            self.assertEqual(result['ready'], 0, reason)
            self.assertEqual(summary['hosts'][0]['blocked'], [{'id': 44, 'reason': reason}])
            self.assertFalse((self.root / 'host-15-snapshot.json').exists(), reason)
        value = effective()
        value[0] = {'legacy_node_id': 43, 'error': 'legacy validation: TEST_ONLY'}
        self.write_effective(value)
        self.assertEqual(self.built()[1]['hosts'][0]['blocked'], [{'id': 43, 'reason': 'legacy_builder_refused'}])
        (self.root / 'host-15-effective.json').unlink()
        self.assertEqual(self.built()[1]['hosts'][0]['blocked'], [{'reason': 'effective_configuration_missing'}])

    def test_vision_flow_follows_the_legacy_users(self):
        value = effective()
        del value[1]['configuration']['inbounds'][0]['users'][0]['flow']
        self.write_effective(value)
        self.built()
        built = json.loads((self.root / 'host-15-snapshot.json').read_text())
        self.assertEqual(built['nodes'][1]['preserved']['node']['settings']['reality']['flow'], 'none')

    def candidate(self, ss_users, vless_users):
        inbounds = [{'type': 'vless', 'listen_port': 11452,
                     'users': [{'name': f'u{i}_n2', 'uuid': u} for i, u in enumerate(vless_users)]}]
        if ss_users is not None:
            inbounds.append({'type': 'shadowsocks', 'listen_port': 51002,
                             'users': [{'name': f'u{i}_n1', 'password': p} for i, p in enumerate(ss_users)]})
        config = json.dumps({'inbounds': inbounds})
        return {'payload': {'bundle': json.dumps({'files': {'config.json': config}})}}

    def test_compare_users_reports_authorization_drift(self):
        self.write_effective(effective())
        self.built()
        built = json.loads((self.root / 'host-15-snapshot.json').read_text())
        legacy = {'nodes': [
            {'id': 43, 'users': [{'uuid': UUID_A, 'speed_limit': 0, 'device_limit': 0}]},
            {'id': 44, 'users': [{'uuid': UUID_A, 'speed_limit': 0, 'device_limit': 0},
                                 {'uuid': UUID_B, 'speed_limit': 100, 'device_limit': 0}]}]}
        key = base64.b64encode(UUID_A.encode()[:16]).decode()
        out = self.root / 'compare.json'
        result = snapshot.compare_users(built, self.candidate([key], [UUID_A, UUID_B]), legacy, out)
        self.assertEqual(result, {'nodes': 2, 'equal': True, 'limited_users': 1})
        result = snapshot.compare_users(built, self.candidate([key], [UUID_A]), legacy, out)
        self.assertFalse(result['equal'])
        report = json.loads(out.read_text())
        self.assertEqual(report['nodes'][1]['missing'], 1)
        self.assertEqual(report['differences'][1]['missing'], [UUID_B])
        self.assertEqual(stat.S_IMODE(out.stat().st_mode), 0o600)
        disabled = copy.deepcopy(built)
        disabled['nodes'][0]['preserved']['node']['enabled'] = False
        result = snapshot.compare_users(disabled, self.candidate([key], [UUID_A, UUID_B]), legacy, out)
        self.assertFalse(result['equal'])
        result = snapshot.compare_users(disabled, self.candidate([], [UUID_A, UUID_B]), legacy, out)
        self.assertFalse(result['equal'], 'an empty listener for a disabled instance is still exposed')
        result = snapshot.compare_users(disabled, self.candidate(None, [UUID_A, UUID_B]), legacy, out)
        self.assertTrue(result['equal'])


if __name__ == '__main__':
    unittest.main()
