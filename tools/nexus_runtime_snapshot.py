#!/usr/bin/env python3
"""Build preserved Nexus runtime snapshots from the legacy node's own output.

`export-legacy` reads, in the legacy panel container, each node's published
configuration (`ServerService::buildNodeConfig`) and its currently available
users. `request` writes one input per physical host for the legacy builder in
`tools/nexus-legacy-export`, which reproduces the exact sing-box configuration
the running node generates. `build` turns that effective configuration into
the `{source_id, server_id, nodes}` snapshot consumed by
`nexus_runtime_takeover.py`. `compare-users` checks a panel candidate against
the legacy user lists before any activation.

All outputs contain node keys or member identities: they are written with
mode 0600 and only counts and fixed reason codes are printed.
"""
import argparse
import base64
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

PLACEHOLDER_UUID = '00000000-0000-4000-8000-000000000001'
SS_METHODS = {'2022-blake3-aes-128-gcm': 16, '2022-blake3-aes-256-gcm': 32}
TOP_LEVEL = {'log', 'inbounds', 'outbounds', 'route', 'dns'}

LEGACY_EXPORT = r'''<?php
require '/www/vendor/autoload.php';
$app = require '/www/bootstrap/app.php';
$app->make(Illuminate\Contracts\Console\Kernel::class)->bootstrap();
$nodes = [];
foreach (App\Models\Server::orderBy('id')->get() as $node) {
    $users = [];
    foreach (App\Services\ServerService::getAvailableUsers($node) as $user) {
        $users[] = ['uuid' => (string) $user->uuid, 'speed_limit' => (int) ($user->speed_limit ?? 0),
                    'device_limit' => (int) ($user->device_limit ?? 0)];
    }
    $nodes[] = ['id' => $node->id, 'configuration' => App\Services\ServerService::buildNodeConfig($node),
                'users' => $users];
}
echo json_encode(['exported_at' => time(), 'nodes' => $nodes], JSON_THROW_ON_ERROR);
'''


class SnapshotError(Exception):
    pass


def private_write(path, value):
    path = Path(path)
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(str(path), os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, 'w') as handle:
        json.dump(value, handle, ensure_ascii=False, separators=(',', ':'))


def load(path):
    return json.loads(Path(path).read_text())


def export_legacy(container, output):
    result = subprocess.run(['docker', 'exec', '-i', container, 'php'], input=LEGACY_EXPORT, text=True,
                            capture_output=True, timeout=120, check=False)
    if result.returncode:
        # PHP errors can echo configuration; keep them out of the terminal.
        raise SnapshotError('Legacy export failed; no private error detail printed')
    value = json.loads(result.stdout)
    private_write(output, value)
    return {'nodes': len(value['nodes'])}


def host_nodes(inventory):
    machines = {machine['id']: machine['host_id'] for machine in inventory['machines']}
    hosts = {}
    for node in inventory['nodes']:
        if node['machine_id'] not in machines:
            raise SnapshotError(f"Node {node['id']} has no host mapping")
        hosts.setdefault(machines[node['machine_id']], []).append(node)
    return hosts


def requests(inventory, legacy, directory):
    configurations = {node['id']: node['configuration'] for node in legacy['nodes']}
    written = 0
    for host, nodes in sorted(host_nodes(inventory).items()):
        missing = [node['id'] for node in nodes if node['id'] not in configurations]
        if missing:
            raise SnapshotError(f'Legacy export lacks nodes {missing}; export again')
        private_write(Path(directory) / f'host-{host}-request.json', {'nodes': [
            {'legacy_node_id': node['id'], 'machine_id': node['machine_id'], 'type': node['type'],
             'node_config': configurations[node['id']]} for node in sorted(nodes, key=lambda n: n['id'])]})
        written += 1
    return {'hosts': written}


def references_files(value):
    if isinstance(value, dict):
        return any(((key == 'path' or key.endswith('_path')) and isinstance(item, str) and item)
                   or references_files(item) for key, item in value.items())
    if isinstance(value, list):
        return any(references_files(item) for item in value)
    return False


def as_port(value):
    # The legacy builder decodes ports weakly; the snapshot model is strict.
    if isinstance(value, bool):
        raise SnapshotError('port_invalid')
    if isinstance(value, str) and value.isdigit():
        value = int(value)
    if not isinstance(value, int) or not 1 <= value <= 65535:
        raise SnapshotError('port_invalid')
    return value


def protocol_settings(raw):
    value = raw.get('protocol_settings')
    if isinstance(value, str):
        value = json.loads(value or '{}')
    return value or {}


def preserved(raw, effective):
    """Convert one legacy effective configuration into a PreservedNode."""
    configuration = copy.deepcopy(effective)
    if set(configuration) - TOP_LEVEL:
        raise SnapshotError('global_settings_require_reconciliation')
    if references_files(configuration):
        raise SnapshotError('external_files')
    inbounds = configuration.get('inbounds') or []
    if len(inbounds) != 1:
        raise SnapshotError('listener_count')
    inbound = inbounds[0]
    kind = inbound.get('type')
    if kind not in ('vless', 'shadowsocks'):
        # Hysteria2 also needs its certificate files and port-hopping rules.
        raise SnapshotError('protocol_requires_conversion')
    users = inbound.get('users') or []
    if len(users) != 1 or users[0].get('name') != PLACEHOLDER_UUID:
        raise SnapshotError('unexpected_user_list')
    if 'transport' in inbound:
        raise SnapshotError('transport_requires_conversion')
    if inbound.get('multiplex', {}).get('brutal'):
        raise SnapshotError('brutal_requires_review')
    port = as_port(inbound.get('listen_port'))
    node = {'id': raw['id'], 'name': raw['name'], 'port': port, 'public_host': raw['host'],
            'sni': '', 'private_key': '', 'public_key': '', 'short_id': '', 'users': [],
            'enabled': bool(raw.get('enabled', 1)),
            'settings': {'listen': inbound.get('listen'), 'public_port': as_port(raw.get('port'))}}
    if kind == 'vless':
        tls = inbound.get('tls') or {}
        reality = tls.get('reality') or {}
        if not tls.get('enabled') or not reality.get('enabled'):
            raise SnapshotError('vless_without_reality')
        short_ids = reality.get('short_id') or []
        if len(short_ids) != 1:
            raise SnapshotError('reality_short_id_count')
        handshake = reality.get('handshake') or {}
        public_key = (protocol_settings(raw).get('reality_settings') or {}).get('public_key')
        if not public_key:
            raise SnapshotError('reality_public_key_missing')
        flow = users[0].get('flow', '')
        if flow not in ('', 'xtls-rprx-vision'):
            raise SnapshotError('vless_flow_unsupported')
        node.update(sni=tls.get('server_name') or '', private_key=reality.get('private_key') or '',
                    public_key=public_key, short_id=short_ids[0], protocol_config={'type': 'vless-reality'})
        node['settings']['reality'] = {'handshake_server': handshake.get('server'),
                                       'handshake_port': as_port(handshake.get('server_port', 443)),
                                       'flow': 'vision' if flow else 'none'}
    else:
        method = inbound.get('method')
        if method not in SS_METHODS:
            raise SnapshotError('shadowsocks_method_unsupported')
        expected = base64.b64encode(PLACEHOLDER_UUID.encode()[:SS_METHODS[method]]).decode()
        if users[0].get('password') != expected:
            raise SnapshotError('shadowsocks_user_key_derivation')
        node['protocol_config'] = {'type': 'shadowsocks2022', 'method': method, 'password': inbound.get('password')}
    return {'legacy_node_id': raw['id'], 'preserved': {'node': node, 'configuration': configuration}}


def build(inventory, directory):
    servers = {host['legacy_id']: host['server_id'] for host in inventory['hosts']}
    summary = []
    for host, nodes in sorted(host_nodes(inventory).items()):
        report = {'legacy_host_id': host, 'server_id': servers.get(host), 'nodes': len(nodes), 'blocked': []}
        source = Path(directory) / f'host-{host}-effective.json'
        if not source.exists():
            report['blocked'].append({'reason': 'effective_configuration_missing'})
        else:
            results = {entry['legacy_node_id']: entry for entry in load(source)}
            prepared = []
            for raw in sorted(nodes, key=lambda n: n['id']):
                entry = results.get(raw['id'])
                if entry is None or entry.get('error'):
                    report['blocked'].append({'id': raw['id'], 'reason': 'legacy_builder_refused'})
                    continue
                try:
                    prepared.append(preserved(raw, entry['configuration']))
                except SnapshotError as error:
                    report['blocked'].append({'id': raw['id'], 'reason': str(error)})
            if report['server_id'] is None:
                report['blocked'].append({'reason': 'host_not_staged'})
            if not report['blocked']:
                snapshot = {'source_id': inventory['source_id'], 'server_id': report['server_id'], 'nodes': prepared}
                encoded = json.dumps(snapshot, sort_keys=True, separators=(',', ':')).encode()
                report['snapshot_sha256'] = hashlib.sha256(encoded).hexdigest()
                private_write(Path(directory) / f'host-{host}-snapshot.json', snapshot)
        report['ready'] = not report['blocked']
        summary.append(report)
    private_write(Path(directory) / 'snapshot-summary.json', {'built_at': int(time.time()), 'hosts': summary})
    return {'hosts': len(summary), 'ready': sum(r['ready'] for r in summary),
            'blocked_nodes': sum(len(r['blocked']) for r in summary)}


def wire_identity(protocol, method, uuid):
    if protocol == 'shadowsocks':
        return base64.b64encode(uuid.encode()[:SS_METHODS[method]]).decode()
    return uuid


def compare_users(snapshot, candidate, legacy, output):
    """Match each legacy listener's users with the candidate by listen port."""
    bundle = json.loads(candidate['payload']['bundle'])
    config = json.loads(bundle['files']['config.json'])
    listeners = {inbound['listen_port']: inbound for inbound in config.get('inbounds', [])}
    users = {node['id']: node['users'] for node in legacy['nodes']}
    report, differences = [], []
    for entry in snapshot['nodes']:
        node = entry['preserved']['node']
        inbound = listeners.get(node['port'])
        protocol = 'shadowsocks' if node['protocol_config'].get('type') == 'shadowsocks2022' else 'vless'
        method = node['protocol_config'].get('method')
        enabled = node.get('enabled', True)
        # A disabled legacy instance serves nobody; the candidate must not
        # open its listener either.
        expected = {wire_identity(protocol, method, user['uuid'])
                    for user in users.get(node['id'], [])} if enabled else set()
        actual = set()
        for user in (inbound or {}).get('users', []):
            actual.add(user['uuid'] if protocol == 'vless' else user['password'])
        limited = sum(1 for user in users.get(node['id'], []) if user['speed_limit'] or user['device_limit'])
        row = {'legacy_node_id': node['id'], 'legacy_users': len(expected), 'candidate_users': len(actual),
               'missing': len(expected - actual), 'extra': len(actual - expected),
               'listener_present': inbound is not None, 'limited_users': limited if enabled else 0}
        if not enabled:
            row['legacy_disabled'] = True
            if inbound is not None:
                row['extra'] = max(row['extra'], 1)
        report.append(row)
        differences.append({'legacy_node_id': node['id'], 'missing': sorted(expected - actual),
                            'extra': sorted(actual - expected)})
    private_write(output, {'nodes': report, 'differences': differences})
    equal = all(row['missing'] == 0 and row['extra'] == 0 for row in report)
    return {'nodes': len(report), 'equal': equal, 'limited_users': sum(r['limited_users'] for r in report)}


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest='command', required=True)
    command = commands.add_parser('export-legacy')
    command.add_argument('--container', required=True)
    command.add_argument('output')
    command = commands.add_parser('request')
    command.add_argument('inventory')
    command.add_argument('legacy')
    command.add_argument('directory')
    command = commands.add_parser('build')
    command.add_argument('inventory')
    command.add_argument('directory')
    command = commands.add_parser('compare-users')
    command.add_argument('snapshot')
    command.add_argument('candidate')
    command.add_argument('legacy')
    command.add_argument('output')
    args = parser.parse_args()
    if args.command == 'export-legacy':
        result = export_legacy(args.container, args.output)
    elif args.command == 'request':
        result = requests(load(args.inventory), load(args.legacy), args.directory)
    elif args.command == 'build':
        result = build(load(args.inventory), args.directory)
    else:
        result = compare_users(load(args.snapshot), load(args.candidate), load(args.legacy), args.output)
    print(json.dumps({'command': args.command, **result}))
    if args.command == 'compare-users' and not result['equal']:
        raise SystemExit(2)


if __name__ == '__main__':
    try:
        main()
    except (SnapshotError, KeyError, ValueError, OSError) as error:
        print(json.dumps({'status': 'failed', 'error_class': type(error).__name__,
                          'reason': str(error) if isinstance(error, SnapshotError) else None}))
        raise SystemExit(1) from None
