"""Synthetic-only migration and compatibility regression cases."""
import json
from contextlib import closing
from pathlib import Path
import sqlite3
import tempfile
import unittest
from export_nexus import snapshot as nexus
from export_wallos import units
from migration_snapshot import connect, write_private
from nexus_compatibility import apply_members, BridgeError, observe


class MigrationTests(unittest.TestCase):
    def test_private_files_are_read_only_and_exclusive(self):
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / 'source.db'
            sqlite3.connect(database).execute('CREATE TABLE fixture(id INTEGER)').connection.close()
            with closing(connect(database)) as reader:
                with self.assertRaises(sqlite3.OperationalError):
                    reader.execute('INSERT INTO fixture VALUES(1)')
            output = Path(directory) / 'snapshot.json'
            write_private(output, {'fixture': True})
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)
            with self.assertRaises(FileExistsError):
                write_private(output, {})
            self.assertEqual(json.loads(output.read_text()), {'fixture': True})

    def test_amounts_have_exact_four_digit_precision(self):
        self.assertEqual(units('1.2345'), '12345')
        self.assertEqual(units('900000000000'), '9000000000000000')
        for value in ['1.00001', '-1', 'NaN', 'Infinity', '900000000001']:
            with self.assertRaises(ValueError):
                units(value)

    def test_export_uses_allowlist_and_opt_in_password_hashes(self):
        c = sqlite3.connect(':memory:')
        self.addCleanup(c.close)
        c.row_factory = sqlite3.Row
        c.execute('CREATE TABLE v2_user(id INTEGER,email TEXT,balance INTEGER,banned INTEGER,uuid TEXT,plan_id INTEGER,invite_user_id INTEGER,transfer_enable INTEGER,u INTEGER,d INTEGER,expired_at INTEGER,next_reset_at INTEGER,token TEXT,nexus_sponsor_required INTEGER,nexus_first_purchase_at INTEGER,member_status TEXT,last_reset_at INTEGER,password TEXT,admin_secret TEXT,remind_expire INTEGER,remind_traffic INTEGER)')
        c.execute("INSERT INTO v2_user VALUES(41,'fixture@example.com',123,0,'00000000-0000-4000-8000-000000000041',7,NULL,1000,10,20,9999999999,9999999000,'fixture-subscription-token',0,1,'ACTIVE',1,'fixture-hash','never-export',0,1)")
        c.execute('CREATE TABLE v2_plan(id INTEGER,name TEXT,transfer_enable INTEGER,prices TEXT,group_id INTEGER)')
        c.execute('INSERT INTO v2_plan VALUES(7,?,?,?,9)', ('Fixture', 1, '{"monthly":"1.25"}'))
        c.execute('CREATE TABLE v2_traffic_reset_logs(id INTEGER,user_id INTEGER,created_at INTEGER)')
        c.execute('INSERT INTO v2_traffic_reset_logs VALUES(1,99,100)')
        exported = nexus(c, 'fixture')
        self.assertIsNone(exported['history'][0]['member_id'])
        self.assertEqual(exported['history'][0]['record']['user_id'], 99)
        self.assertEqual(exported['plans'][0]['prices'][0]['cents'], 125)
        self.assertEqual(exported['members'][0]['balance_cents'], 123)
        self.assertFalse(exported['members'][0]['remind_expire'])
        self.assertTrue(exported['members'][0]['remind_traffic'])
        self.assertIsNone(exported['members'][0]['legacy_password_hash'])
        self.assertNotIn('never-export', json.dumps(exported))
        self.assertNotIn('fixture-hash', json.dumps(exported))
        self.assertEqual(nexus(c, 'fixture', True)['members'][0]['legacy_password_hash'], 'fixture-hash')
        c.execute('UPDATE v2_plan SET prices=?', ('{"unknown":100}',))
        with self.assertRaises(ValueError):
            nexus(c, 'fixture')

    def test_bridge_preserves_credentials_counters_and_rolls_back_mismatches(self):
        c = sqlite3.connect(':memory:', isolation_level=None)
        self.addCleanup(c.close)
        c.execute('CREATE TABLE v2_user(id INTEGER PRIMARY KEY,email TEXT,balance INTEGER,banned INTEGER,member_status TEXT,plan_id INTEGER,group_id INTEGER,transfer_enable INTEGER,expired_at INTEGER,invite_user_id INTEGER,nexus_sponsor_required INTEGER,nexus_first_purchase_at INTEGER,uuid TEXT,token TEXT,password TEXT,created_at INTEGER,updated_at INTEGER,member_version INTEGER DEFAULT 1,u INTEGER DEFAULT 0,d INTEGER DEFAULT 0,last_reset_at INTEGER,next_reset_at INTEGER,speed_limit INTEGER,device_limit INTEGER)')
        c.execute('CREATE TABLE v2_plan(id INTEGER,speed_limit INTEGER,device_limit INTEGER)')
        c.execute('INSERT INTO v2_plan VALUES(7,100,3)')
        c.execute("CREATE TRIGGER wallet_initial_guard BEFORE INSERT ON v2_user WHEN NEW.balance != 0 BEGIN SELECT RAISE(ABORT, 'initial credit requires wallet ledger'); END")
        c.execute("CREATE TRIGGER wallet_snapshot_guard BEFORE UPDATE OF balance ON v2_user WHEN NEW.balance != OLD.balance BEGIN SELECT RAISE(ABORT, 'balance changes require wallet ledger'); END")
        c.execute("CREATE TRIGGER archive_guard BEFORE UPDATE ON v2_user WHEN NEW.member_status = 'ARCHIVED' BEGIN SELECT RAISE(ABORT, 'archive requires legacy audit'); END")
        record = {'id': 41, 'email': 'fixture@example.com', 'balance': 123, 'banned': 0, 'member_status': 'ACTIVE', 'plan_id': 7, 'group_id': 9, 'quota_bytes': '1000', 'expires_at': 9999999999, 'inviter_id': None, 'sponsor_required': False, 'first_purchase_at': 1, 'uuid': '00000000-0000-4000-8000-000000000041', 'subscription_token': 'original-token'}
        self.assertEqual(apply_members(c, [record])['created'], 1)
        c.execute('UPDATE v2_user SET u=50,d=100,last_reset_at=20,next_reset_at=80,password=?', ('unchanged-legacy-hash',))
        self.assertEqual(apply_members(c, [record])['changed'], 0)
        self.assertEqual(tuple(c.execute('SELECT u,d,last_reset_at,next_reset_at,password FROM v2_user').fetchone()), (50,100,20,80,'unchanged-legacy-hash'))
        wrong = dict(record, subscription_token='different-token')
        with self.assertRaises(BridgeError):
            apply_members(c, [dict(record,id=42,uuid='00000000-0000-4000-8000-000000000042'), wrong])
        self.assertEqual(c.execute('SELECT count(*) FROM v2_user').fetchone()[0], 1)
        changed = dict(record,balance=999,expires_at=9999999998)
        self.assertEqual(apply_members(c,[changed])['changed'],1)
        self.assertEqual(c.execute('SELECT balance FROM v2_user').fetchone()[0],0)
        suspended = dict(changed,banned=1,member_status='ARCHIVED')
        self.assertEqual(apply_members(c,[suspended])['changed'],1)
        self.assertEqual(c.execute('SELECT member_status FROM v2_user').fetchone()[0],'SUSPENDED')
        self.assertEqual(apply_members(c,[suspended])['changed'],0)
        self.assertEqual(apply_members(c,[changed])['changed'],1)
        observed = observe(c, '00000000-0000-4000-8000-000000000001')
        self.assertEqual(observed['members'][0]['upload'], '50')
        self.assertNotIn('password', json.dumps(observed))


if __name__ == '__main__':
    unittest.main()
