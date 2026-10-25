import base64
import json
import sqlite3
import subprocess
import unittest
from unittest.mock import patch

import nexus_compatibility as bridge

IMPORT = '6f1e4c7a-9d2b-4a3e-8f10-1234567890ab'


class DeviceBridgeTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(':memory:', isolation_level=None)
        self.db.execute('CREATE TABLE v2_user(id INTEGER PRIMARY KEY,device_limit INTEGER)')
        self.db.executemany('INSERT INTO v2_user VALUES(?,?)', [(1, 3), (2, None), (3, 5000)])
        self.config = {'import_id': IMPORT, 'legacy_container': 'legacy', 'pg_container': 'pg',
                       'pg_user': 'apsp', 'pg_database': 'apsp'}
        self.samples = []
        self.statuses = []

    def tearDown(self):
        self.db.close()

    def database(self, managed):
        def run(config, sql):
            for function, found in (('singbox_nexus_device_exchange', self.samples), ('singbox_nexus_node_status_exchange', self.statuses)):
                prefix = f"SELECT {function}(convert_from(decode('"
                if sql.startswith(prefix):
                    encoded = sql[len(prefix):sql.index("'", len(prefix))]
                    found.append(json.loads(bytes.fromhex(encoded)))
                    return {'managed': managed} if found is self.samples else {'nodes': 0}
            self.fail('unexpected SQL')
        return run

    def legacy(self, legacy=None, nodes=None):
        return subprocess.CompletedProcess([], 0, json.dumps({'legacy': legacy or {}, 'nodes': nodes}), '')

    def test_limits_are_always_shared_and_live_addresses_only_once_nodes_are_taken_over(self):
        self.config['_last_node_status'] = bridge.time.monotonic()
        with patch.object(bridge, 'database', self.database([])), \
             patch.object(bridge.subprocess, 'run') as php:
            self.assertEqual(bridge.exchange_devices(self.config, self.db), 0)
        php.assert_not_called()
        sample = self.samples[0]
        self.assertEqual(sample['import_id'], IMPORT)
        self.assertEqual(sample['limits'], {'1': 3, '2': 0, '3': 1024})
        self.assertEqual(sample['legacy'], {})
        self.assertEqual(self.config['_device_managed'], [])
        self.assertEqual(self.statuses, [])

    def test_node_status_is_exported_every_thirty_seconds_without_live_addresses(self):
        nodes = [{'id': 7, 'online': True, 'checked_at': 1, 'load': {'cpu': 1.5}, 'latency': {'monitored': False, 'carriers': {}}}]
        completed = self.legacy({'1': ['198.51.100.1']}, nodes)
        with patch.object(bridge, 'database', self.database([])), \
             patch.object(bridge.subprocess, 'run', return_value=completed) as php:
            self.assertEqual(bridge.exchange_devices(self.config, self.db), 0)
            bridge.exchange_devices(self.config, self.db)
        self.assertEqual(php.call_count, 1, 'status is read once per window while nothing is taken over')
        payload = php.call_args.kwargs['input'].split("base64_decode('", 1)[1].split("'", 1)[0]
        self.assertEqual(json.loads(base64.b64decode(payload)), {'managed': [], 'status': True})
        self.assertEqual(self.samples[0]['legacy'], {}, 'legacy addresses stay out until a node is taken over')
        self.assertEqual(self.statuses, [{'import_id': IMPORT, 'observed_at': self.samples[0]['observed_at'], 'nodes': nodes}])

    def test_status_failures_never_block_limits_before_takeover(self):
        for completed in (subprocess.CompletedProcess([], 1, '', 'PHP Fatal error: secret'), self.legacy(nodes=None)):
            self.samples.clear()
            self.config.pop('_last_node_status', None)
            with patch.object(bridge, 'database', self.database([])), \
                 patch.object(bridge.subprocess, 'run', return_value=completed):
                if completed.returncode:
                    with self.assertRaises(bridge.BridgeError) as error:
                        bridge.exchange_devices(self.config, self.db)
                    self.assertNotIn('secret', str(error.exception))
                else:
                    bridge.exchange_devices(self.config, self.db)
            self.assertEqual(self.samples[0]['limits']['1'], 3)
        self.assertEqual(self.statuses, [], 'an unavailable status is never written as an empty catalogue')

    def test_apsp_addresses_are_written_as_data_and_legacy_ones_read_back_safely(self):
        managed = [{'legacy_node_id': 43, 'users': {'1': ['203.0.113.5']}}]
        self.config['_device_managed'] = managed
        legacy = {'legacy': {'1': ['198.51.100.1', 'not-an-address', '198.51.100.1'], '2': ['garbage']}}
        completed = subprocess.CompletedProcess([], 0, json.dumps(legacy), '')
        self.config['_last_node_status'] = bridge.time.monotonic()
        with patch.object(bridge, 'database', self.database(managed)), \
             patch.object(bridge.subprocess, 'run', return_value=completed) as php:
            self.assertEqual(bridge.exchange_devices(self.config, self.db), 1)
        code = php.call_args.kwargs['input']
        self.assertNotIn('203.0.113.5', code, 'addresses travel as an encoded literal, never as code')
        payload = code.split("base64_decode('", 1)[1].split("'", 1)[0]
        self.assertEqual(json.loads(base64.b64decode(payload)), {'managed': managed, 'status': False})
        self.assertEqual(self.samples[0]['legacy'], {'1': ['198.51.100.1']})

    def test_failed_legacy_exchange_raises_without_private_detail(self):
        self.config['_device_managed'] = [{'legacy_node_id': 43, 'users': {}}]
        failed = subprocess.CompletedProcess([], 1, '', 'PHP Fatal error: secret')
        with patch.object(bridge, 'database', self.database([])), \
             patch.object(bridge.subprocess, 'run', return_value=failed):
            with self.assertRaises(bridge.BridgeError) as error:
                bridge.exchange_devices(self.config, self.db)
        self.assertNotIn('secret', str(error.exception))


if __name__ == '__main__':
    unittest.main()
