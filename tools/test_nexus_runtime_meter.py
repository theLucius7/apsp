import sqlite3
import unittest

from nexus_compatibility import BridgeError, apply_usage


class MeterTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(':memory:', isolation_level=None)
        self.db.execute('CREATE TABLE v2_user(id INTEGER PRIMARY KEY,u INTEGER,d INTEGER,last_reset_at INTEGER,next_reset_at INTEGER)')
        self.db.execute('INSERT INTO v2_user VALUES(1,100,200,1000,2000)')

    def tearDown(self):
        self.db.close()

    def delivery(self, **changes):
        return dict(id=1, user_id=1, uplink='10', downlink='20', rate='2.500000', period_start=1100, period_end=1200, **changes)

    def counters(self):
        return self.db.execute('SELECT u,d FROM v2_user').fetchone()

    def test_lost_ack_replay_charges_once(self):
        entry = self.delivery()
        self.assertEqual(apply_usage(self.db, 'source', [entry]), [1])
        self.assertEqual(apply_usage(self.db, 'source', [entry]), [1])
        self.assertEqual(self.counters(), (125, 250))
        changed = {**entry, 'uplink': '99'}
        with self.assertRaises(BridgeError):
            apply_usage(self.db, 'source', [changed])
        self.assertEqual(self.counters(), (125, 250))

    def test_old_and_cross_cycle_replays(self):
        entry = {**self.delivery(), 'period_start': 900, 'period_end': 1100}
        apply_usage(self.db, 'source', [entry])
        self.assertEqual(self.counters(), (112, 225))
        self.db.execute('UPDATE v2_user SET u=0,d=0,last_reset_at=2000,next_reset_at=3000')
        apply_usage(self.db, 'source', [entry])
        apply_usage(self.db, 'source', [{**entry, 'id': 2}])
        self.assertEqual(self.counters(), (0, 0))

    def test_failed_batch_rolls_back_receipts_and_counters(self):
        with self.assertRaises(BridgeError):
            apply_usage(self.db, 'source', [self.delivery(), {**self.delivery(), 'id': 2, 'user_id': 99}])
        self.assertEqual(self.counters(), (100, 200))
        apply_usage(self.db, 'source', [self.delivery()])
        self.assertEqual(self.counters(), (125, 250))

    def test_free_multiplier_and_zero_duration(self):
        apply_usage(self.db, 'source', [{**self.delivery(), 'rate': '0'}])
        self.assertEqual(self.counters(), (100, 200))
        apply_usage(self.db, 'source', [{**self.delivery(), 'id': 2, 'period_start': 1200}])
        self.assertEqual(self.counters(), (125, 250))

    def test_counter_overflow_is_atomic(self):
        with self.assertRaises(BridgeError):
            apply_usage(self.db, 'source', [{**self.delivery(), 'uplink': '9000000000000000000'}])
        self.assertEqual(self.counters(), (100, 200))

class NativeRetirementTests(unittest.TestCase):
    def setUp(self):
        self.db = sqlite3.connect(':memory:', isolation_level=None)
        self.db.execute('CREATE TABLE v2_user(id INTEGER PRIMARY KEY,banned INTEGER,member_status TEXT,updated_at INTEGER,member_version INTEGER,uuid TEXT,token TEXT,u INTEGER,d INTEGER)')
        self.db.executemany('INSERT INTO v2_user VALUES(?,0,?,0,1,?,?,100,200)', [(1, 'ACTIVE', 'kept-uuid', 'kept-token'), (2, 'ARCHIVED', 'kept-uuid-2', 'kept-token-2')])

    def tearDown(self):
        self.db.close()

    def test_native_members_are_disabled_once_without_losing_identity_or_usage(self):
        from nexus_compatibility import apply_members
        first = apply_members(self.db, [], [1, 2])
        replay = apply_members(self.db, [], [1, 2])
        self.assertEqual(first['retired'], 2)
        self.assertEqual(replay['retired'], 0)
        self.assertEqual(self.db.execute('SELECT banned,member_status,member_version,uuid,token,u,d FROM v2_user WHERE id=1').fetchone(), (1, 'SUSPENDED', 2, 'kept-uuid', 'kept-token', 100, 200))
        self.assertEqual(self.db.execute('SELECT member_status FROM v2_user WHERE id=2').fetchone()[0], 'ARCHIVED')

    def test_bad_retirement_is_atomic(self):
        from nexus_compatibility import apply_members
        for retired in ([1, 1], [True], [-1]):
            with self.assertRaises(BridgeError):
                apply_members(self.db, [], retired)
        with self.assertRaises(BridgeError):
            apply_members(self.db, [{'id': 1}], [1])
        self.assertEqual(self.db.execute('SELECT SUM(banned) FROM v2_user').fetchone()[0], 0)

    def test_synchronize_handshake_retries_cleanly_and_refreshes_nodes_on_last_retirement(self):
        import tempfile
        from pathlib import Path
        from unittest import mock
        import nexus_compatibility as nc

        with tempfile.TemporaryDirectory() as directory:
            sqlite_path = Path(directory) / 'legacy.sqlite'
            conn = sqlite3.connect(str(sqlite_path), isolation_level=None)
            conn.execute('CREATE TABLE v2_user(id INTEGER PRIMARY KEY,email TEXT,banned INTEGER,member_status TEXT,plan_id INTEGER,group_id INTEGER,transfer_enable INTEGER,expired_at INTEGER,invite_user_id INTEGER,nexus_sponsor_required INTEGER,nexus_first_purchase_at INTEGER,updated_at INTEGER,member_version INTEGER,uuid TEXT,token TEXT,password TEXT,created_at INTEGER,balance INTEGER,u INTEGER,d INTEGER,last_reset_at INTEGER,next_reset_at INTEGER,device_limit INTEGER,speed_limit INTEGER)')
            conn.execute('CREATE TABLE v2_server(id INTEGER PRIMARY KEY,machine_id INTEGER,enabled INTEGER)')
            conn.execute("INSERT INTO v2_user(id,banned,member_status,member_version,uuid,token,u,d,last_reset_at,next_reset_at,device_limit) VALUES(1,0,'ACTIVE',1,'12345678-1234-4234-8234-123456789abc','tok',100,200,1000,2000,0)")
            conn.close()

            config = {'pg_container': 'pg', 'pg_user': 'apsp', 'pg_database': 'apsp',
                      'legacy_database': str(sqlite_path), 'legacy_container': 'legacy',
                      'import_id': '11111111-1111-4111-8111-111111111111', '_node_sync_pending': False, '_last_node_sync': 10**9}
            generation = '22222222-2222-4222-8222-222222222222'
            sql_calls, php_inputs = [], []
            fail_first_php = [True]

            def fake_db(_cfg, sql):
                sql_calls.append(sql)
                if 'singbox_nexus_bridge_begin' in sql:
                    return {'started': True}
                if 'FROM (SELECT id,user_id' in sql:
                    return [dict(id=7, user_id=1, uplink='10', downlink='20', rate='2.0', period_start=1100, period_end=1200)]
                if 'UPDATE singbox_nexus_usage_deliveries' in sql:
                    return {'acknowledged': 1}
                if 'singbox_nexus_bridge_exchange' in sql:
                    return {'generation': generation, 'members': [], 'retired_members': [1]}
                if 'UPDATE singbox_nexus_bridge SET last_sync_at' in sql:
                    return {'acknowledged': True, 'retired': 1}
                if 'singbox_nexus_device_exchange' in sql:
                    return {'managed': []}
                if 'singbox_nexus_node_status_exchange' in sql:
                    return {'ok': True}
                raise AssertionError(sql)

            def fake_run(cmd, input=None, **kwargs):
                php_inputs.append(input)
                if fail_first_php[0]:
                    fail_first_php[0] = False
                    return type('R', (), {'returncode': 1, 'stdout': '', 'stderr': 'err'})()
                if 'DEVICE_EXCHANGE' in (input or '') or '@INPUT@' not in (input or '') and 'legacy' in (input or ''):
                    return type('R', (), {'returncode': 0, 'stdout': '{"legacy":{},"nodes":[]}', 'stderr': ''})()
                return type('R', (), {'returncode': 0, 'stdout': '{"legacy":{},"nodes":[]}', 'stderr': ''})()

            with mock.patch.object(nc, 'database', side_effect=fake_db), mock.patch.object(nc.subprocess, 'run', side_effect=fake_run):
                with self.assertRaises(BridgeError):
                    nc.synchronize(config)
                # First attempt failed during initialize_cycles before ACK; retry reuses receipts and ACKs retirement.
                self.assertFalse(any('UPDATE singbox_nexus_bridge SET last_sync_at' in s for s in sql_calls))
                outcome = nc.synchronize(config)
                self.assertEqual(outcome['members'], 0)
            self.assertTrue(any('sync.users' in (code or '') for code in php_inputs))
            check = sqlite3.connect(str(sqlite_path))
            self.assertEqual(check.execute('SELECT banned,member_status,u,d FROM v2_user WHERE id=1').fetchone(), (1, 'SUSPENDED', 120, 240))
            check.close()


class StatefulBridgeDouble:
    """In-memory PostgreSQL + legacy PHP container double matching 0001_current.sql and adoption.rs."""

    def __init__(self, import_id):
        import uuid
        self.uuid = uuid
        self.import_id = import_id
        self.now = 1_700_000_000
        self.bridge = {
            'enabled': True,
            'import_id': import_id,
            'pending_generation': None,
            'native_control_ready': False,
            'last_sync_at': None,
        }
        self.deliveries = []
        self.metering = {}
        self.desired_members = []
        self.adoptions = {}
        self.issued_generations = []
        self.sql_log = []
        self.php_calls = []
        self.begin_hook = None
        self.delivery_ack_hook = None
        self.final_ack_hook = None
        self.initialize_hook = None

    def adoption_ready(self):
        return (
            self.bridge['enabled']
            and self.bridge['native_control_ready'] is True
            and self.bridge['pending_generation'] is None
            and self.bridge['last_sync_at'] is not None
            and all(item['delivered_at'] is not None for item in self.deliveries)
        )

    def retirement_ready(self):
        return (
            self.adoption_ready()
            and not self.desired_members
            and all(item['legacy_retired_at'] is not None for item in self.adoptions.values())
        )

    def database(self, _config, sql):
        import json
        import re
        self.sql_log.append(sql)
        if 'singbox_nexus_bridge_begin' in sql:
            if self.begin_hook is not None:
                hook, self.begin_hook = self.begin_hook, None
                return hook()
            requested = re.search(r"singbox_nexus_bridge_begin\('([0-9a-f-]+)'::uuid\)", sql).group(1)
            if not self.bridge['enabled'] or requested != self.bridge['import_id']:
                raise BridgeError('PostgreSQL exchange failed; no private error detail logged')
            if self.bridge['pending_generation'] is None:
                self.bridge['pending_generation'] = str(self.uuid.uuid4())
            self.bridge['native_control_ready'] = True
            return {'started': True}

        if 'FROM singbox_nexus_usage_deliveries WHERE delivered_at IS NULL' in sql:
            pending = [
                {k: item[k] for k in ('id', 'user_id', 'uplink', 'downlink', 'rate', 'period_start', 'period_end')}
                for item in sorted(self.deliveries, key=lambda row: row['id'])
                if item['delivered_at'] is None
            ][:500]
            return pending

        if 'UPDATE singbox_nexus_usage_deliveries SET delivered_at=' in sql:
            if self.delivery_ack_hook is not None:
                hook, self.delivery_ack_hook = self.delivery_ack_hook, None
                return hook()
            ids = {int(v) for v in re.search(r'ARRAY\[([0-9,]+)\]::bigint\[\]', sql).group(1).split(',')}
            acked = 0
            for item in self.deliveries:
                if item['id'] in ids and item['delivered_at'] is None:
                    item['delivered_at'] = self.now
                    acked += 1
            return {'acknowledged': acked}

        if 'singbox_nexus_bridge_exchange(' in sql:
            encoded = re.search(r"decode\('([0-9a-f]+)','hex'\)", sql).group(1)
            sample = json.loads(bytes.fromhex(encoded))
            if not self.bridge['enabled'] or sample.get('import_id') != self.bridge['import_id']:
                raise BridgeError('PostgreSQL exchange failed; no private error detail logged')
            for member in sample['members']:
                uid = int(member['id'])
                if uid not in self.adoptions:
                    self.metering[uid] = {
                        'upload': int(member['upload']),
                        'download': int(member['download']),
                        'last_reset_at': member['last_reset_at'],
                        'next_reset_at': member['next_reset_at'],
                        'observed_at': sample['observed_at'],
                    }
            was_pending = self.bridge['pending_generation'] is not None
            generation = self.bridge['pending_generation'] or str(self.uuid.uuid4())
            self.issued_generations.append(generation)
            for adoption in self.adoptions.values():
                if adoption['legacy_retired_at'] is None:
                    adoption['legacy_retired_generation'] = generation
            self.bridge['pending_generation'] = generation
            self.bridge['native_control_ready'] = bool(was_pending and self.bridge['native_control_ready'])
            return {
                'generation': generation,
                'members': [dict(item) for item in self.desired_members],
                'retired_members': sorted(self.adoptions),
            }

        if 'UPDATE singbox_nexus_bridge SET last_sync_at=' in sql:
            if self.final_ack_hook is not None:
                hook, self.final_ack_hook = self.final_ack_hook, None
                return hook(self)
            matches = re.findall(r"'([0-9a-f-]+)'::uuid", sql)
            if len(matches) != 2 or matches[0] != matches[1]:
                raise AssertionError(f'Mismatched generation bindings in ACK SQL: {sql}')
            generation = matches[0]
            if not self.bridge['enabled'] or self.bridge['pending_generation'] != generation:
                return {'acknowledged': False, 'retired': 0}
            self.bridge['last_sync_at'] = self.now
            self.bridge['pending_generation'] = None
            retired = 0
            for adoption in self.adoptions.values():
                if adoption['legacy_retired_generation'] == generation and adoption['legacy_retired_at'] is None:
                    adoption['legacy_retired_at'] = self.now
                    retired += 1
            return {'acknowledged': True, 'retired': retired}

        if 'singbox_nexus_device_exchange(' in sql:
            return {'managed': []}

        if 'singbox_nexus_node_status_exchange(' in sql:
            return {'nodes': 0}

        raise AssertionError(f'Unexpected SQL: {sql}')

    def php(self, _cmd, input=None, **_kwargs):
        code = input or ''
        self.php_calls.append(code)
        if 'TrafficResetService' in code or 'sync.users' in code:
            if self.initialize_hook is not None:
                hook, self.initialize_hook = self.initialize_hook, None
                return hook()
            return type('R', (), {'returncode': 0, 'stdout': '', 'stderr': ''})()
        return type('R', (), {'returncode': 0, 'stdout': '{"legacy":{},"nodes":[]}', 'stderr': ''})()


class StatefulSynchronizeEndToEndTests(unittest.TestCase):
    IMPORT_ID = '11111111-1111-4111-8111-111111111111'
    USER_1_UUID = '12345678-1234-4234-8234-123456789abc'
    USER_2_UUID = '87654321-4321-4321-8321-cba987654321'

    def setUp(self):
        import tempfile
        from pathlib import Path
        self.tempdir = tempfile.TemporaryDirectory()
        self.addCleanup(self.tempdir.cleanup)
        self.sqlite_path = Path(self.tempdir.name) / 'legacy.sqlite'
        conn = sqlite3.connect(str(self.sqlite_path), isolation_level=None)
        conn.execute(
            'CREATE TABLE v2_user(id INTEGER PRIMARY KEY,email TEXT,banned INTEGER,member_status TEXT,'
            'plan_id INTEGER,group_id INTEGER,transfer_enable INTEGER,expired_at INTEGER,'
            'invite_user_id INTEGER,nexus_sponsor_required INTEGER,nexus_first_purchase_at INTEGER,'
            'updated_at INTEGER,member_version INTEGER,uuid TEXT,token TEXT,password TEXT,'
            'created_at INTEGER,balance INTEGER,u INTEGER,d INTEGER,last_reset_at INTEGER,'
            'next_reset_at INTEGER,device_limit INTEGER,speed_limit INTEGER)'
        )
        conn.execute('CREATE TABLE v2_plan(id INTEGER PRIMARY KEY,speed_limit INTEGER,device_limit INTEGER)')
        conn.execute('CREATE TABLE v2_server(id INTEGER PRIMARY KEY,machine_id INTEGER,enabled INTEGER)')
        conn.execute('INSERT INTO v2_plan VALUES(5,100,3)')
        conn.execute(
            "INSERT INTO v2_user(id,email,banned,member_status,plan_id,group_id,transfer_enable,"
            "expired_at,invite_user_id,nexus_sponsor_required,nexus_first_purchase_at,updated_at,"
            "member_version,uuid,token,u,d,last_reset_at,next_reset_at,device_limit,speed_limit,balance) "
            "VALUES(1,'u1@member.invalid',0,'ACTIVE',5,1,10000,NULL,NULL,0,NULL,0,1,?,'tok-1',100,200,1000,2000,3,100,0)",
            (self.USER_1_UUID,),
        )
        conn.close()
        self.double = StatefulBridgeDouble(self.IMPORT_ID)
        self.double.deliveries.append(dict(
            id=1, user_id=1, uplink='10', downlink='20', rate='2.500000',
            period_start=1100, period_end=1200, delivered_at=None,
        ))
        self.double.adoptions[1] = {'legacy_retired_generation': None, 'legacy_retired_at': None}
        self.config = {
            'pg_container': 'pg',
            'pg_user': 'apsp',
            'pg_database': 'apsp',
            'legacy_database': str(self.sqlite_path),
            'legacy_container': 'legacy',
            'import_id': self.IMPORT_ID,
            '_node_sync_pending': False,
            '_last_node_sync': 10**9,
        }

    def desired_member(self, user_id=2, plan_id=5):
        return {
            'id': user_id,
            'email': f'u{user_id}@member.invalid',
            'uuid': self.USER_2_UUID,
            'subscription_token': f'tok-{user_id}',
            'balance': 0,
            'banned': 0,
            'member_status': 'ACTIVE',
            'plan_id': plan_id,
            'group_id': 1,
            'quota_bytes': '50000',
            'expires_at': None,
            'inviter_id': None,
            'sponsor_required': False,
            'first_purchase_at': None,
        }

    def user_row(self, user_id):
        conn = sqlite3.connect(str(self.sqlite_path))
        try:
            return conn.execute(
                'SELECT banned,member_status,u,d,plan_id,speed_limit,device_limit FROM v2_user WHERE id=?',
                (user_id,),
            ).fetchone()
        finally:
            conn.close()

    def receipt_count(self):
        conn = sqlite3.connect(str(self.sqlite_path))
        try:
            exists = conn.execute(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='apsp_usage_receipts'"
            ).fetchone()
            return conn.execute('SELECT COUNT(*) FROM apsp_usage_receipts').fetchone()[0] if exists else 0
        finally:
            conn.close()

    def run_sync(self):
        from unittest import mock
        import nexus_compatibility as nc
        with mock.patch.object(nc, 'database', side_effect=self.double.database), \
                mock.patch.object(nc.subprocess, 'run', side_effect=self.double.php):
            return nc.synchronize(self.config)

    def test_begin_failure_leaves_panel_and_legacy_untouched_and_retry_completes(self):
        for failure in (
            lambda: (_ for _ in ()).throw(BridgeError('PostgreSQL exchange failed; no private error detail logged')),
            lambda: {'started': False},
        ):
            self.double.begin_hook = failure
            with self.assertRaises(BridgeError):
                self.run_sync()
            self.assertIsNone(self.double.bridge['pending_generation'])
            self.assertFalse(self.double.bridge['native_control_ready'])
            self.assertFalse(self.double.adoption_ready())
            self.assertFalse(self.double.retirement_ready())
            self.assertIsNone(self.double.deliveries[0]['delivered_at'])
            self.assertIsNone(self.double.adoptions[1]['legacy_retired_at'])
            self.assertEqual(self.user_row(1), (0, 'ACTIVE', 100, 200, 5, 100, 3))
            self.assertEqual(self.receipt_count(), 0)
            self.assertEqual(self.double.php_calls, [])

        outcome = self.run_sync()
        self.assertEqual(outcome['retired'], 1)
        self.assertIsNone(self.double.bridge['pending_generation'])
        self.assertTrue(self.double.bridge['native_control_ready'])
        self.assertTrue(self.double.adoption_ready())
        self.assertTrue(self.double.retirement_ready())
        self.assertEqual(self.user_row(1), (1, 'SUSPENDED', 125, 250, 5, 100, 3))
        self.assertEqual(self.receipt_count(), 1)
        self.assertEqual(len(set(self.double.issued_generations)), 1)

    def test_midflight_apply_and_cycle_failures_keep_pending_generation_without_duplicate_usage(self):
        # Unbracketed exchange without begin must never leave native_control_ready true.
        self.double.bridge['native_control_ready'] = True
        import nexus_compatibility as nc
        self.double.database(
            self.config,
            f"SELECT singbox_nexus_bridge_exchange({nc.json_sql({'import_id': self.IMPORT_ID, 'observed_at': self.double.now, 'members': []})});",
        )
        self.assertFalse(self.double.bridge['native_control_ready'])
        self.double.bridge['pending_generation'] = None
        self.double.issued_generations.clear()
        self.double.adoptions[1]['legacy_retired_generation'] = None

        # 1) Mid-flight failure inside apply_members (missing plan 9 rolls back SQLite member batch).
        self.double.desired_members = [self.desired_member(user_id=2, plan_id=9)]
        with self.assertRaisesRegex(BridgeError, 'Mapped legacy plan no longer exists'):
            self.run_sync()
        first_gen = self.double.bridge['pending_generation']
        self.assertIsNotNone(first_gen)
        self.assertFalse(self.double.adoption_ready())
        self.assertFalse(self.double.retirement_ready())
        self.assertIsNone(self.double.adoptions[1]['legacy_retired_at'])
        self.assertIsNone(self.user_row(2))
        # Usage delivery committed once before apply_members; member retirement rolled back cleanly.
        self.assertEqual(self.user_row(1), (0, 'ACTIVE', 125, 250, 5, 100, 3))
        self.assertEqual(self.double.php_calls, [])

        # Fix missing plan, then fail mid-flight inside initialize_cycles.
        conn = sqlite3.connect(str(self.sqlite_path), isolation_level=None)
        conn.execute('INSERT INTO v2_plan VALUES(9,200,5)')
        conn.close()
        self.double.initialize_hook = lambda: type('R', (), {'returncode': 1, 'stdout': '', 'stderr': 'err'})()
        with self.assertRaisesRegex(BridgeError, 'Legacy billing schedule initialization failed'):
            self.run_sync()
        self.assertEqual(self.double.bridge['pending_generation'], first_gen)
        self.assertFalse(self.double.adoption_ready())
        self.assertIsNone(self.double.adoptions[1]['legacy_retired_at'])
        self.assertTrue(self.config['_node_sync_pending'])
        self.assertEqual(self.user_row(1), (1, 'SUSPENDED', 125, 250, 5, 100, 3))
        self.assertEqual(self.user_row(2), (0, 'ACTIVE', 0, 0, 9, 200, 5))

        # Final retry reuses exact generation, does not double-count usage, refreshes nodes and ACKs.
        outcome = self.run_sync()
        self.assertEqual(outcome['created'], 0)
        self.assertEqual(outcome['retired'], 0)
        self.assertEqual(self.double.issued_generations, [first_gen, first_gen, first_gen])
        self.assertIsNone(self.double.bridge['pending_generation'])
        self.assertIsNotNone(self.double.adoptions[1]['legacy_retired_at'])
        self.assertTrue(self.double.adoption_ready())
        self.assertFalse(self.config['_node_sync_pending'])
        self.assertEqual(self.user_row(1), (1, 'SUSPENDED', 125, 250, 5, 100, 3))
        self.assertTrue(any('sync.users' in code for code in self.double.php_calls[-2:]))

    def test_lost_delivery_and_final_ack_failures_reuse_generation_and_charge_once(self):
        # Attempt 1: SQLite commits usage + receipt, then PG delivery outbox ACK is lost.
        self.double.delivery_ack_hook = lambda: (_ for _ in ()).throw(
            BridgeError('PostgreSQL exchange failed; no private error detail logged')
        )
        with self.assertRaises(BridgeError):
            self.run_sync()
        gen = self.double.bridge['pending_generation']
        self.assertIsNotNone(gen)
        self.assertIsNone(self.double.deliveries[0]['delivered_at'])
        self.assertEqual(self.receipt_count(), 1)
        self.assertEqual(self.user_row(1), (0, 'ACTIVE', 125, 250, 5, 100, 3))
        self.assertIsNone(self.double.adoptions[1]['legacy_retired_at'])

        # Attempt 2: Replays undelivered row via SQLite receipt without double-counting, then loses final ACK.
        self.double.final_ack_hook = lambda _db: (_ for _ in ()).throw(
            BridgeError('PostgreSQL exchange failed; no private error detail logged')
        )
        with self.assertRaises(BridgeError):
            self.run_sync()
        self.assertEqual(self.double.bridge['pending_generation'], gen)
        self.assertIsNotNone(self.double.deliveries[0]['delivered_at'])
        self.assertIsNone(self.double.adoptions[1]['legacy_retired_at'])
        self.assertFalse(self.double.adoption_ready())
        self.assertFalse(self.double.retirement_ready())
        self.assertTrue(self.config['_node_sync_pending'])
        self.assertEqual(self.user_row(1), (1, 'SUSPENDED', 125, 250, 5, 100, 3))

        # Attempt 3: Unmatched generation ACK response is rejected without clearing pending_generation.
        self.double.final_ack_hook = lambda _db: {'acknowledged': False, 'retired': 0}
        with self.assertRaisesRegex(BridgeError, 'Generation acknowledgment failed'):
            self.run_sync()
        self.assertEqual(self.double.bridge['pending_generation'], gen)
        self.assertIsNone(self.double.adoptions[1]['legacy_retired_at'])
        self.assertTrue(self.config['_node_sync_pending'])

        # Attempt 4: Clean retry reuses exact generation, keeps single usage delta, and marks retirement.
        self.run_sync()
        self.assertEqual(self.double.issued_generations, [gen, gen, gen])
        self.assertIsNone(self.double.bridge['pending_generation'])
        self.assertEqual(self.double.adoptions[1]['legacy_retired_at'], self.double.now)
        self.assertTrue(self.double.adoption_ready())
        self.assertTrue(self.double.retirement_ready())
        self.assertFalse(self.config['_node_sync_pending'])
        self.assertEqual(self.user_row(1), (1, 'SUSPENDED', 125, 250, 5, 100, 3))


if __name__ == '__main__':
    unittest.main()
