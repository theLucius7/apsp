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


if __name__ == '__main__':
    unittest.main()
