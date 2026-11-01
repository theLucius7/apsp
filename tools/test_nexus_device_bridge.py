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


class MachineHeartbeatTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(':memory:', isolation_level=None)
        self.db.execute('CREATE TABLE v2_server(id INTEGER PRIMARY KEY,machine_id INTEGER,enabled INTEGER)')
        self.db.execute('CREATE TABLE v2_server_machine(id INTEGER PRIMARY KEY,is_active INTEGER,last_seen_at INTEGER,configuration TEXT)')
        self.db.executemany('INSERT INTO v2_server VALUES(?,?,?)',
                            [(101, 1, 1), (102, 1, 1), (201, 2, 0), (301, 3, 1), (401, 4, 1)])
        self.db.executemany('INSERT INTO v2_server_machine VALUES(?,?,?,?)',
                            [(1, 1, 900, 'original'), (2, 1, 900, 'disabled nodes'),
                             (3, 0, 900, 'inactive machine'), (4, 1, 1100, 'newer heartbeat')])

    def tearDown(self):
        self.db.close()

    def health(self, *nodes):
        return [{'legacy_node_id': node, 'healthy': True, 'users': {}} for node in nodes]

    def rows(self):
        return self.db.execute('SELECT * FROM v2_server_machine ORDER BY id').fetchall()

    def test_fresh_complete_health_updates_only_machine_heartbeat(self):
        before = self.rows()
        self.assertEqual(bridge.refresh_machine_heartbeats(self.db, self.health(101, 102), 1000, now=1000), 1)
        self.assertEqual(self.rows(), [(1, 1, 1000, 'original'), *before[1:]])
        self.assertEqual(self.db.execute('SELECT id,enabled FROM v2_server').fetchall(),
                         [(101, 1), (102, 1), (201, 0), (301, 1), (401, 1)])

    def test_partial_unhealthy_and_disabled_machines_never_receive_false_heartbeats(self):
        before = self.rows()
        for health in [self.health(101), self.health(101) + [{'legacy_node_id': 102, 'healthy': False}],
                       self.health(201, 301)]:
            self.assertEqual(bridge.refresh_machine_heartbeats(self.db, health, 1000, now=1000), 0)
            self.assertEqual(self.rows(), before)
        self.db.execute('UPDATE v2_server SET enabled=0 WHERE id=102')
        self.assertEqual(bridge.refresh_machine_heartbeats(self.db, self.health(101), 1000, now=1000), 1)

    def test_old_future_and_regressive_samples_do_not_extend_availability(self):
        before = self.rows()
        for stamp in [939, 1006, True, '1000']:
            self.assertEqual(bridge.refresh_machine_heartbeats(self.db, self.health(101, 102), stamp, now=1000), 0)
        self.assertEqual(bridge.refresh_machine_heartbeats(self.db, self.health(401), 1000, now=1000), 0)
        self.assertEqual(self.rows(), before)

    def test_paused_missing_and_ambiguous_inventory_fails_closed(self):
        before = self.rows()
        self.assertEqual(bridge.refresh_machine_heartbeats(self.db, [], 1000, now=1000), 0)
        for health in [self.health(999), self.health(101, 101),
                       [{'legacy_node_id': 101, 'healthy': 'true'}]]:
            with self.assertRaises(bridge.BridgeError):
                bridge.refresh_machine_heartbeats(self.db, health, 1000, now=1000)
            self.assertEqual(self.rows(), before)

    def test_write_failure_rolls_back_the_entire_heartbeat_batch(self):
        self.db.execute('UPDATE v2_server_machine SET last_seen_at=900 WHERE id=4')
        self.db.execute("CREATE TRIGGER fail_second BEFORE UPDATE ON v2_server_machine WHEN NEW.id=4 BEGIN SELECT RAISE(ABORT,'test failure'); END")
        before = self.rows()
        with self.assertRaises(sqlite3.DatabaseError):
            bridge.refresh_machine_heartbeats(self.db, self.health(101, 102, 401), 1000, now=1000)
        self.assertEqual(self.rows(), before)

    def test_catalogue_export_preserves_fresh_heartbeats_even_when_status_write_fails(self):
        self.db.execute('CREATE TABLE v2_user(id INTEGER PRIMARY KEY,device_limit INTEGER)')
        self.db.execute('INSERT INTO v2_user VALUES(1,3)')
        nodes = [{'id': 101, 'online': True, 'load': {}, 'latency': {}}]
        for failed in [False, True]:
            with self.subTest(status_write_failed=failed):
                self.db.execute('UPDATE v2_server_machine SET last_seen_at=900 WHERE id=1')
                config = {'import_id': IMPORT, '_device_managed': self.health(101)}
                calls = []

                def database(config, sql):
                    calls.append(sql)
                    if 'singbox_nexus_device_exchange(' in sql:
                        return {'managed': self.health(101, 102)}
                    self.assertIn('singbox_nexus_node_status_exchange(', sql)
                    # Heartbeats and admission state are committed before the optional catalogue.
                    self.assertEqual(self.rows()[0][2], 1000)
                    self.assertEqual(config['_device_managed'], self.health(101, 102))
                    if failed:
                        raise bridge.BridgeError('status unavailable')
                    return {'nodes': 1}

                with patch.object(bridge, 'legacy_devices', return_value=({}, nodes)) as legacy, \
                     patch.object(bridge, 'database', side_effect=database), \
                     patch.object(bridge.time, 'time', return_value=1000):
                    if failed:
                        with self.assertRaises(bridge.BridgeError):
                            bridge.exchange_devices(config, self.db)
                    else:
                        self.assertEqual(bridge.exchange_devices(config, self.db), 2)
                legacy.assert_called_once_with(config, self.health(101), True)
                self.assertEqual(len(calls), 2)
                self.assertEqual(config['_machine_heartbeats'], 1)
                self.assertEqual(self.rows()[0][2], 1000)

    def test_exchange_uses_the_fresh_database_result_not_cached_healthy_nodes(self):
        self.db.execute('CREATE TABLE v2_user(id INTEGER PRIMARY KEY,device_limit INTEGER)')
        config = {'import_id': IMPORT, '_device_managed': self.health(101, 102)}
        with patch.object(bridge, 'legacy_devices', return_value=({}, None)), \
             patch.object(bridge, 'database', return_value={'managed': [{'legacy_node_id': 101, 'healthy': False}]}), \
             patch.object(bridge.time, 'time', return_value=1000):
            self.assertEqual(bridge.exchange_devices(config, self.db), 1)
        self.assertEqual(config['_machine_heartbeats'], 0)
        self.assertEqual(self.rows()[0][2], 900)
        with patch.object(bridge, 'legacy_devices', return_value=({}, None)), \
             patch.object(bridge, 'database', return_value={'managed': self.health(101, 102)}), \
             patch.object(bridge.time, 'time', return_value=1000):
            self.assertEqual(bridge.exchange_devices(config, self.db), 2)
        self.assertEqual(config['_machine_heartbeats'], 1)
        with patch.object(bridge, 'legacy_devices', return_value=({}, None)), \
             patch.object(bridge, 'database', side_effect=bridge.BridgeError('unavailable')), \
             patch.object(bridge.time, 'time', return_value=1100):
            with self.assertRaises(bridge.BridgeError):
                bridge.exchange_devices(config, self.db)
        self.assertEqual(self.rows()[0][2], 1000)


if __name__ == '__main__':
    unittest.main()
