#!/usr/bin/env python3
"""Keep legacy subscription/node endpoints while apsp owns member business.

Run locally with a mode-0600 JSON configuration. No credentials or member data
are written to stdout. Stop the old business writers before enabling this job.
"""
import argparse
import base64
from decimal import Decimal, ROUND_DOWN
import fcntl
import ipaddress
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


def apply_usage(connection, import_id, deliveries):
    """Commit native deltas and replay receipts in the same legacy transaction.

    Billing dates are read under the write lock, so a reset cannot race a
    delivery. A batch spanning a reset is prorated to the current cycle; the
    original complete counters remain in the native immutable ledger.
    """
    if not isinstance(deliveries, list) or len(deliveries) > 500:
        raise BridgeError('Invalid native usage delivery list')
    connection.execute('BEGIN IMMEDIATE')
    ids = []
    try:
        connection.execute('CREATE TABLE IF NOT EXISTS apsp_usage_receipts (import_id TEXT NOT NULL, delivery_id INTEGER NOT NULL, payload TEXT NOT NULL, applied_at INTEGER NOT NULL, PRIMARY KEY(import_id,delivery_id))')
        for entry in deliveries:
            delivery = entry['id']
            user = entry['user_id']
            if type(delivery) is not int or delivery <= 0 or type(user) is not int or user <= 0 or delivery in ids:
                raise BridgeError('Invalid native usage identity')
            payload = json.dumps(entry, sort_keys=True, separators=(',', ':'))
            old = connection.execute('SELECT payload FROM apsp_usage_receipts WHERE import_id=? AND delivery_id=?', (import_id, delivery)).fetchone()
            if old:
                if old[0] != payload:
                    raise BridgeError('Native usage replay changed payload')
                ids.append(delivery)
                continue
            current = connection.execute('SELECT last_reset_at,next_reset_at,u,d FROM v2_user WHERE id=?', (user,)).fetchone()
            if current is None:
                raise BridgeError('Native usage member is missing from compatibility storage')
            start, end = entry['period_start'], entry['period_end']
            if type(start) is not int or type(end) is not int or not 0 <= start <= end:
                raise BridgeError('Invalid native usage period')
            lower, upper = current[0] or 0, current[1] or 253402300799
            fraction = (Decimal(max(0, min(end, upper) - max(start, lower))) / Decimal(end - start)) if end > start else Decimal(int(lower <= end < upper))
            rate = Decimal(entry['rate'])
            if not rate.is_finite() or rate < 0 or rate > 999999:
                raise BridgeError('Invalid native usage multiplier')
            amounts = []
            for field, existing in [('uplink', current[2]), ('downlink', current[3])]:
                amount = Decimal(entry[field])
                if not amount.is_finite() or amount < 0 or amount != amount.to_integral_value():
                    raise BridgeError('Invalid native usage counter')
                delta = int((amount * rate * fraction).to_integral_value(rounding=ROUND_DOWN))
                if existing + delta > 9_000_000_000_000_000_000:
                    raise BridgeError('Native usage would exceed the legacy counter range')
                amounts.append(delta)
            connection.execute('UPDATE v2_user SET u=u+?,d=d+? WHERE id=?', (*amounts, user))
            connection.execute('INSERT INTO apsp_usage_receipts VALUES(?,?,?,?)', (import_id, delivery, payload, int(time.time())))
            ids.append(delivery)
        connection.commit()
        return ids
    except Exception:
        connection.rollback()
        raise


def deliver_usage(config, connection):
    deliveries = database(config, "SELECT COALESCE(jsonb_agg(to_jsonb(p)),'[]'::jsonb) FROM (SELECT id,user_id,uplink::text,downlink::text,rate::text,period_start,period_end FROM singbox_nexus_usage_deliveries WHERE delivered_at IS NULL ORDER BY id LIMIT 500) p;")
    ids = apply_usage(connection, config['import_id'], deliveries)
    if ids:
        # A crash here is harmless: SQLite receipts make the next delivery a
        # no-op, and the acknowledgement can be repeated independently.
        database(config, 'WITH acknowledged AS (UPDATE singbox_nexus_usage_deliveries SET delivered_at=FLOOR(EXTRACT(EPOCH FROM now()))::bigint WHERE id=ANY(ARRAY[' + ','.join(map(str, ids)) + ']::bigint[]) AND delivered_at IS NULL RETURNING id) SELECT jsonb_build_object(\'acknowledged\',COUNT(*)) FROM acknowledged;')
    return len(ids)


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


DEVICE_EXCHANGE = r"""<?php
require '/www/vendor/autoload.php';
$app = require '/www/bootstrap/app.php';
$app->make(Illuminate\Contracts\Console\Kernel::class)->bootstrap();
$input = json_decode(base64_decode('@INPUT@', true), true, 16, JSON_THROW_ON_ERROR);
$service = app(App\Services\DeviceStateService::class);
$managed = [];
// Publish apsp-admitted addresses under each taken-over legacy node, exactly
// as that node's own device report would, so legacy nodes count them.
foreach ($input['managed'] as $node) {
    $nodeId = (int) $node['legacy_node_id'];
    $managed[] = $nodeId;
    $new = [];
    foreach ($node['users'] as $uid => $ips) {
        $new[(int) $uid] = array_values(array_map('strval', $ips));
    }
    foreach (array_diff_key($service->getNodeDevices($nodeId), $new) as $uid => $ips) {
        $service->removeNodeDevices($nodeId, (int) $uid);
        $service->notifyUpdate((int) $uid);
    }
    foreach ($new as $uid => $ips) {
        $service->setDevices($uid, $nodeId, $ips);
    }
    // The old node software no longer reports this node; keep its status
    // truthful while apsp confirms the node healthy, and let it lapse otherwise.
    $server = App\Models\Server::find($nodeId);
    if (($node['healthy'] ?? false) === true && $server) {
        $type = strtoupper($server->type);
        Illuminate\Support\Facades\Cache::put(App\Utils\CacheKey::get("SERVER_{$type}_LAST_CHECK_AT", $nodeId), time(), 3600);
        Illuminate\Support\Facades\Cache::put(App\Utils\CacheKey::get("SERVER_{$type}_LAST_PUSH_AT", $nodeId), time(), 3600);
        Illuminate\Support\Facades\Cache::put(App\Utils\CacheKey::get("SERVER_{$type}_ONLINE_USER", $nodeId), count($new), 3600);
    }
}
$prefix = config('database.redis.options.prefix', '');
$now = time();
$seen = [];
foreach (Illuminate\Support\Facades\Redis::keys('user_devices:*') as $key) {
    $actual = ($prefix && str_starts_with($key, $prefix)) ? substr($key, strlen($prefix)) : $key;
    $uid = (int) substr($actual, strlen('user_devices:'));
    foreach (Illuminate\Support\Facades\Redis::hgetall($actual) as $field => $at) {
        $split = strpos($field, ':');
        if ($uid <= 0 || $split === false || $now - (int) $at > 300) continue;
        if (in_array((int) substr($field, 0, $split), $managed, true)) continue;
        $seen[$uid][substr($field, $split + 1)] = true;
    }
}
$legacy = [];
foreach ($seen as $uid => $ips) {
    $legacy[(string) $uid] = array_slice(array_keys($ips), 0, 64);
}
echo json_encode(['legacy' => (object) $legacy], JSON_THROW_ON_ERROR);
"""


def legacy_devices(config, managed):
    """Write apsp-admitted addresses to the legacy device store; read the rest."""
    payload = base64.b64encode(json.dumps({'managed': managed}, separators=(',', ':')).encode()).decode()
    result = subprocess.run(['docker', 'exec', '-i', config['legacy_container'], 'php'],
                            input=DEVICE_EXCHANGE.replace('@INPUT@', payload), text=True,
                            capture_output=True, timeout=45, check=False)
    if result.returncode or len(result.stdout) > 8 * 1024 * 1024:
        raise BridgeError('Legacy device exchange failed; no private error detail logged')
    legacy = json.loads(result.stdout)['legacy']
    if not isinstance(legacy, dict):
        raise BridgeError('Legacy device exchange returned an invalid result')
    result = {}
    for uid, ips in legacy.items():
        valid = []
        for ip in ips if isinstance(ips, list) else []:
            try:
                valid.append(str(ipaddress.ip_address(ip)))
            except ValueError:
                continue
        if valid:
            result[str(int(uid))] = sorted(set(valid))[:64]
    return result


def exchange_devices(config, connection):
    """Share device limits and live client addresses with apsp (ADR 0099)."""
    rows = connection.execute('SELECT id,device_limit FROM v2_user ORDER BY id LIMIT 20001').fetchall()
    if len(rows) > 20000:
        raise BridgeError('Legacy member count exceeded the safety bound')
    limits = {str(r[0]): max(0, min(int(r[1] or 0), 1024)) for r in rows}
    managed = config.get('_device_managed', [])
    # Live addresses matter only once apsp serves taken-over nodes.
    legacy = legacy_devices(config, managed) if managed else {}
    sample = {'import_id': str(uuid.UUID(config['import_id'])), 'observed_at': int(time.time()),
              'limits': limits, 'legacy': legacy}
    result = database(config, f'SELECT singbox_nexus_device_exchange({json_sql(sample)});')
    config['_device_managed'] = result['managed']
    return len(result['managed'])


def synchronize(config):
    path = Path(config['legacy_database']).resolve(strict=True)
    connection = sqlite3.connect(f'{path.as_uri()}?mode=rw', uri=True, timeout=20, isolation_level=None)
    try:
        deliver_usage(config, connection)
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
        try:
            result['device_nodes'] = exchange_devices(config, connection)
        except (BridgeError, KeyError, TypeError, ValueError, subprocess.TimeoutExpired) as error:
            # Device limits never block member, quota or traffic compatibility.
            result['device_error'] = type(error).__name__
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
