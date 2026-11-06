#!/usr/bin/env python3
"""Prepare and control one preserved Nexus runtime through the owner API.

No service is stopped by preview/prepare/export. Activation takes a fresh,
reviewed local cutover receipt; it never fabricates remote validation evidence.
`discard` removes a preparation that was never activated so a corrected
snapshot can be prepared; activated hosts can only be paused.
`extend-preview` and `extend` add lines to a host that already has a snapshot
(for example a tunnel origin beside lines taken over earlier): `--input` is a
snapshot built for just the added lines, `--digest` the host's current digest.
`extend` commits only the reviewed merge (`--extended-digest` from the
preview) after the services holding the added listeners were stopped.
"""
import argparse
import json
import time
from pathlib import Path

from nexus_agent_migration import Panel, environment, private_write

ROOT = '/api/plugins/sing-box/nexus-nodes/runtime'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['preview', 'prepare', 'candidate', 'activate', 'pause', 'discard',
                                              'extend-preview', 'extend'])
    parser.add_argument('--panel', required=True)
    parser.add_argument('--environment', type=Path, required=True)
    parser.add_argument('--input', type=Path)
    parser.add_argument('--server', type=int)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--digest', help='current snapshot digest of the host (extend-preview, extend)')
    parser.add_argument('--extended-digest', help='merged digest from the reviewed extend-preview (extend)')
    parser.add_argument('--stopped-service', action='append', default=[],
                        help='service stopped to free an added listener (extend); repeatable')
    parser.add_argument('--stopped-at', type=int, help='Unix time the services were stopped (extend; default now)')
    args = parser.parse_args()
    env = environment(args.environment)
    panel = Panel(args.panel, env['APSP_PUBLIC_URL'], env['APSP_ADMIN_PASSWORD'])
    panel.login()

    def reauthenticate():
        # Every mutating runtime operation requires a fresh owner proof.
        panel.call('/api/control-center/reauth', 'POST', {'password': panel.password})
    if args.operation in ['extend-preview', 'extend']:
        if not args.server or args.server <= 0 or not args.input or not args.digest:
            parser.error('--server, --input and --digest are required')
        added = json.loads(args.input.read_text())
        if added.get('server_id') != args.server:
            parser.error('the added-lines snapshot belongs to another server')
        body = {'expected_digest': args.digest, 'nodes': added['nodes']}
        reauthenticate()
        preview = panel.call(f'{ROOT}/{args.server}/extend/preview', 'POST', body)
        if args.operation == 'extend-preview':
            result = preview
        else:
            if preview.get('conflicts'):
                raise ValueError('Extension preview has conflicts; nothing was changed')
            if not args.extended_digest or preview.get('digest') != args.extended_digest:
                raise ValueError('The merge differs from the reviewed preview; preview and review it again')
            if not args.stopped_service:
                parser.error('--stopped-service is required for extend')
            reauthenticate()
            result = panel.call(f'{ROOT}/{args.server}/extend', 'POST', {
                **body, 'extended_digest': args.extended_digest, 'stopped_services': args.stopped_service,
                'stopped_at': args.stopped_at or int(time.time())})
    elif args.operation in ['preview', 'prepare']:
        if not args.input:
            parser.error('--input is required')
        payload = json.loads(args.input.read_text())
        body = {'payload': payload}
        preview = panel.call(ROOT + '/preview', 'POST', body)
        if args.operation == 'preview':
            result = preview
        else:
            if preview.get('conflicts'):
                raise ValueError('Runtime preview has conflicts; source services remain unchanged')
            reauthenticate()
            result = panel.call(ROOT + '/prepare', 'POST', {**body, 'expected_digest': preview['digest']})
    else:
        if not args.server or args.server <= 0:
            parser.error('--server is required')
        body = None
        if args.operation in ['activate', 'pause', 'discard']:
            if not args.input:
                parser.error('--input must contain the current cutover receipt or expected digest')
            body = json.loads(args.input.read_text())
        reauthenticate()
        result = panel.call(f'{ROOT}/{args.server}/{args.operation}', 'POST', body)
    private_write(args.output, result)
    print(json.dumps({'operation': args.operation, 'saved': True,
                      'state': result.get('state'), 'conflicts': len(result.get('conflicts', []))}))


if __name__ == '__main__':
    try:
        main()
    except Exception as error:
        # HTTP error bodies can include private configuration; never echo them.
        print(json.dumps({'status': 'failed', 'error_class': type(error).__name__}))
        raise SystemExit(1) from None
