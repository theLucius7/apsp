#!/usr/bin/env python3
"""Prepare and control one preserved Nexus runtime through the owner API.

No service is stopped by preview/prepare/export. Activation takes a fresh,
reviewed local cutover receipt; it never fabricates remote validation evidence.
`discard` removes a preparation that was never activated so a corrected
snapshot can be prepared; activated hosts can only be paused.
"""
import argparse
import json
from pathlib import Path

from nexus_agent_migration import Panel, environment, private_write

ROOT = '/api/plugins/sing-box/nexus-nodes/runtime'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['preview', 'prepare', 'candidate', 'activate', 'pause', 'discard'])
    parser.add_argument('--panel', required=True)
    parser.add_argument('--environment', type=Path, required=True)
    parser.add_argument('--input', type=Path)
    parser.add_argument('--server', type=int)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    env = environment(args.environment)
    panel = Panel(args.panel, env['APSP_PUBLIC_URL'], env['APSP_ADMIN_PASSWORD'])
    panel.login()

    def reauthenticate():
        # Every mutating runtime operation requires a fresh owner proof.
        panel.call('/api/control-center/reauth', 'POST', {'password': panel.password})
    if args.operation in ['preview', 'prepare']:
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
