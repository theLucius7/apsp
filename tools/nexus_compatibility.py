#!/usr/bin/env python3
"""Keep legacy subscription/node endpoints while apsp owns member business.

Run locally with a mode-0600 JSON configuration. No credentials or member data
are written to stdout. Stop the old business writers before enabling this job.
"""
import argparse
import fcntl
import json
import os
from pathlib import Path
import secrets
import sqlite3
import subprocess
import time
import uuid


class BridgeError(Exception):
    pass


def database(config, sql):
    command = ['docker', 'exec', '-i', config['pg_container'], 'psql', '-X', '-q', '-t', '-A',
               '-v', 'ON_ERROR_STOP=1', '--username', config['pg_user'], '--dbname', config['pg_database']]
    result = subprocess.run(command, input=sql, text=True, capture_output=True, timeout=45, check=False)
    if result.returncode:
        # psql can include private query parameters in its error output.
        raise BridgeError('PostgreSQL exchange failed; no private error detail logged')
    if len(result.stdout) > 32 * 1024 * 1024:
        raise BridgeError('PostgreSQL result exceeded the safety bound')
    return json.loads(result.stdout.strip())


def json_sql(value):
    encoded = json.dumps(value, separators=(',', ':')).encode().hex()
    return f"convert_from(decode('{encoded}','hex'),'UTF8')::jsonb"


def observe(connection, import_id):
    connection.execute('BEGIN')
    try:
        rows = connection.execute('SELECT id,u,d,last_reset_at,next_reset_at FROM v2_user ORDER BY id LIMIT 20001').fetchall()
        if len(rows) > 20000:
            raise BridgeError('Legacy member count exceeded the safety bound')
        result = {'import_id': import_id, 'observed_at': int(time.time()), 'members': [
            {'id': r[0], 'upload': str(r[1]), 'download': str(r[2]), 'last_reset_at': r[3], 'next_reset_at': r[4]} for r in rows]}
        connection.commit()
        return result
    except Exception:
        connection.rollback()
        raise


def apply_members(connection, members):
    if not isinstance(members, list) or len(members) > 20000:
        raise BridgeError('Invalid desired member list')
    connection.execute('BEGIN IMMEDIATE')
    changed = created = 0
    try:
        ids = set()
        for member in members:
            user = member['id']
            if not isinstance(user, int) or user <= 0 or user in ids:
                raise BridgeError('Invalid or duplicate member ID')
            ids.add(user)
            identity = str(uuid.UUID(member['uuid']))
            quota = int(member['quota_bytes'])
            if not 0 <= quota <= 9_000_000_000_000_000_000 or not 0 <= member['balance'] <= 2_147_483_647:
                raise BridgeError('Member quota or balance is outside the supported range')
            existing = connection.execute('SELECT uuid,token,member_status FROM v2_user WHERE id=?', (user,)).fetchone()
            if existing and (str(uuid.UUID(existing[0])) != identity or existing[1] != member['subscription_token']):
                raise BridgeError('Legacy identity mismatch; entire member batch rolled back')
            # Financial history stays frozen in the legacy system. Its access
            # protocols do not read wallet balances, and its ledger guards must remain.
            status = 'SUSPENDED' if member['banned'] else 'ACTIVE'
            if existing and existing[2] == 'ARCHIVED':
                if not member['banned']:
                    raise BridgeError('Archived legacy members cannot be reactivated during compatibility')
                status = 'ARCHIVED'
            values = (member['email'], member['banned'], status, member['plan_id'],
                      member['group_id'], quota, member['expires_at'], member['inviter_id'],
                      int(member['sponsor_required']), member['first_purchase_at'])
            now = int(time.time())
            old_plan = connection.execute('SELECT plan_id FROM v2_user WHERE id=?', (user,)).fetchone()
            if existing:
                fields = 'email,banned,member_status,plan_id,group_id,transfer_enable,expired_at,invite_user_id,nexus_sponsor_required,nexus_first_purchase_at'
                previous = connection.execute(f'SELECT {fields} FROM v2_user WHERE id=?', (user,)).fetchone()
                if tuple(previous) != values:
                    connection.execute('UPDATE v2_user SET email=?,banned=?,member_status=?,plan_id=?,group_id=?,transfer_enable=?,expired_at=?,invite_user_id=?,nexus_sponsor_required=?,nexus_first_purchase_at=?,updated_at=?,member_version=member_version+1 WHERE id=?', (*values, now, user))
                    changed += 1
            else:
                connection.execute('INSERT INTO v2_user(email,banned,member_status,plan_id,group_id,transfer_enable,expired_at,invite_user_id,nexus_sponsor_required,nexus_first_purchase_at,id,uuid,token,password,created_at,updated_at,balance) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,0)', (*values, user, identity, member['subscription_token'], '!APSP-MANAGED!' + secrets.token_hex(24), now, now))
                created += 1
            if member['plan_id'] is not None and (not old_plan or old_plan[0] != member['plan_id']):
                limits = connection.execute('SELECT speed_limit,device_limit FROM v2_plan WHERE id=?', (member['plan_id'],)).fetchone()
                if limits is None:
                    raise BridgeError('Mapped legacy plan no longer exists')
                connection.execute('UPDATE v2_user SET speed_limit=?,device_limit=? WHERE id=?', (*limits, user))
        connection.commit()
        return {'members': len(members), 'created': created, 'changed': changed}
    except Exception:
        connection.rollback()
        raise


def initialize_cycles(config, members, refresh_nodes=False):
    # The legacy scheduler remains the authority for its billing dates. Initialize
    # only missing schedules; never reset a counter or replace an existing date.
    ids = [str(int(member['id'])) for member in members]
    if not ids:
        return
    code = "<?php require '/www/vendor/autoload.php'; $app=require '/www/bootstrap/app.php'; $app->make(Illuminate\\Contracts\\Console\\Kernel::class)->bootstrap(); $service=app(App\\Services\\TrafficResetService::class); foreach(App\\Models\\User::whereIn('id',[" + ','.join(ids) + "])->whereNull('next_reset_at')->whereNotNull('plan_id')->where('banned',0)->get() as $user) { $service->setInitialResetTime($user); }"
    if refresh_nodes:
        # Refresh only user lists; node configuration and credentials stay intact.
        # A periodic full list also removes users from a previous permission group.
        code += " foreach(App\\Models\\Server::all() as $node) { if (App\\Services\\NodeSyncService::isNodeOnline($node->id)) { Illuminate\\Support\\Facades\\Redis::publish('node:push', json_encode(['node_id'=>$node->id,'event'=>'sync.users','data'=>['users'=>App\\Services\\ServerService::getAvailableUsers($node)->toArray()]], JSON_THROW_ON_ERROR)); } }"
    result = subprocess.run(['docker', 'exec', '-i', config['legacy_container'], 'php'], input=code,
                            text=True, capture_output=True, timeout=45, check=False)
    if result.returncode:
        raise BridgeError('Legacy billing schedule initialization failed')


def synchronize(config):
    path = Path(config['legacy_database']).resolve(strict=True)
    connection = sqlite3.connect(f'{path.as_uri()}?mode=rw', uri=True, timeout=20, isolation_level=None)
    try:
        sample = observe(connection, str(uuid.UUID(config['import_id'])))
        desired = database(config, f'SELECT singbox_nexus_bridge_exchange({json_sql(sample)});')
        generation = str(uuid.UUID(desired['generation']))
        result = apply_members(connection, desired['members'])
        config['_node_sync_pending'] = config.get('_node_sync_pending', True) or bool(result['created'] or result['changed'])
        refresh = config['_node_sync_pending'] or time.monotonic() - config.get('_last_node_sync', 0) >= 60
        initialize_cycles(config, desired['members'], refresh)
        if refresh:
            config['_node_sync_pending'] = False
            config['_last_node_sync'] = time.monotonic()
        ack = database(config, f"WITH acknowledged AS (UPDATE singbox_nexus_bridge SET last_sync_at=EXTRACT(EPOCH FROM clock_timestamp())::bigint,pending_generation=NULL WHERE singleton AND enabled AND pending_generation='{generation}'::uuid RETURNING 1) SELECT jsonb_build_object('acknowledged',EXISTS(SELECT 1 FROM acknowledged));")
        if not ack.get('acknowledged'):
            raise BridgeError('Generation acknowledgment failed; check for another bridge process')
        return result
    finally:
        connection.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--config', required=True)
    parser.add_argument('--once', action='store_true')
    args = parser.parse_args()
    path = Path(args.config).resolve(strict=True)
    if path.stat().st_mode & 0o077:
        raise BridgeError('Configuration must be readable only by its owner (0600)')
    config = json.loads(path.read_text())
    interval = max(5, min(60, int(config.get('interval', 10))))
    # Hold one lock for the entire daemon lifetime, including exchanges and retries.
    fd = os.open(str(path) + '.lock', os.O_CREAT | os.O_RDWR, 0o600)
    with os.fdopen(fd, 'w') as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        while True:
            try:
                result = synchronize(config)
                print(json.dumps({'status': 'ok', **result}), flush=True)
            except Exception as error:
                print(json.dumps({'status': 'failed', 'error_class': type(error).__name__}), flush=True)
                if args.once:
                    raise SystemExit(1) from None
            if args.once:
                return
            time.sleep(interval)


if __name__ == '__main__':
    main()
