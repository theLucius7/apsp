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

    def tearDown(self):
        self.db.close()

    def database(self, managed):
        def run(config, sql):
            prefix = "SELECT singbox_nexus_device_exchange(convert_from(decode('"
            self.assertTrue(sql.startswith(prefix))
            encoded = sql[len(prefix):sql.index("'", len(prefix))]
            self.samples.append(json.loads(bytes.fromhex(encoded)))
            return {'managed': managed}
        return run

    def test_limits_are_always_shared_and_live_addresses_only_once_nodes_are_taken_over(self):
        with patch.object(bridge, 'database', self.database([])), \
             patch.object(bridge.subprocess, 'run') as php:
            self.assertEqual(bridge.exchange_devices(self.config, self.db), 0)
        php.assert_not_called()
        sample = self.samples[0]
        self.assertEqual(sample['import_id'], IMPORT)
        self.assertEqual(sample['limits'], {'1': 3, '2': 0, '3': 1024})
        self.assertEqual(sample['legacy'], {})
        self.assertEqual(self.config['_device_managed'], [])

    def test_apsp_addresses_are_written_as_data_and_legacy_ones_read_back_safely(self):
        managed = [{'legacy_node_id': 43, 'users': {'1': ['203.0.113.5']}}]
        self.config['_device_managed'] = managed
        legacy = {'legacy': {'1': ['198.51.100.1', 'not-an-address', '198.51.100.1'], '2': ['garbage']}}
        completed = subprocess.CompletedProcess([], 0, json.dumps(legacy), '')
        with patch.object(bridge, 'database', self.database(managed)), \
             patch.object(bridge.subprocess, 'run', return_value=completed) as php:
            self.assertEqual(bridge.exchange_devices(self.config, self.db), 1)
        code = php.call_args.kwargs['input']
        self.assertNotIn('203.0.113.5', code, 'addresses travel as an encoded literal, never as code')
        payload = code.split("base64_decode('", 1)[1].split("'", 1)[0]
        self.assertEqual(json.loads(base64.b64decode(payload)), {'managed': managed})
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
