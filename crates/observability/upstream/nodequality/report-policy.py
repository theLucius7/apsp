#!/usr/bin/env python3
"""Patch only the public report POSTs in three exact AGPL source snapshots."""
import argparse
import hashlib
import sys

MAX_SOURCE = 2 * 1024 * 1024
POLICY = b'''# Apsp modification (2026-10-01): gate only public report POSTs.
# Original upstream copyright and AGPL-3.0 license remain applicable.
# Privacy mode, benchmarks, report serialization and local output are unchanged.
case "${APSP_UPLOAD_REPORT-false}" in
true|false) readonly apsp_public_report_upload="${APSP_UPLOAD_REPORT-false}" ;;
*) printf '%s\\n' 'Error: APSP_UPLOAD_REPORT must be true or false' >&2; exit 2 ;;
esac
'''
SOURCES = {
    'hardware.sh': {
        'source_sha256': '73e032ef5409e014cca411a71c677a76db19a94ef0a96a73827b41b2059cd86c',
        'patched_sha256': '0729052a2f9f11787ab71b34b0d8c0dbeaf4928d28de68178555a8aaee281847',
        'original_guard': b'[[ mode_privacy -eq 0 ]]&&report_link=$(curl -$2 -s -X POST http://upload.check.place -d "type=hardware" --data-urlencode "json=$hwjson" --data-urlencode "content=$hw_report")\n',
        'patched_guard': b'[[ $apsp_public_report_upload == true && mode_privacy -eq 0 ]]&&report_link=$(curl -$2 -s -X POST http://upload.check.place -d "type=hardware" --data-urlencode "json=$hwjson" --data-urlencode "content=$hw_report")\n',
    },
    'ip.sh': {
        'source_sha256': 'b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf',
        'patched_sha256': 'a063a1563280d2d0ad25b2921d6003c53c69163c1e015d2a82c98ab721927141',
        'original_guard': b'[[ $mode_lite -eq 0 && mode_privacy -eq 0 ]]&&report_link=$(curl -$2 -s -X POST https://upload.check.place -d "type=ip" --data-urlencode "json=$ipjson" --data-urlencode "content=$ip_report")\n',
        'patched_guard': b'[[ $apsp_public_report_upload == true && $mode_lite -eq 0 && mode_privacy -eq 0 ]]&&report_link=$(curl -$2 -s -X POST https://upload.check.place -d "type=ip" --data-urlencode "json=$ipjson" --data-urlencode "content=$ip_report")\n',
    },
    'net.sh': {
        'source_sha256': '6c40fe1ae40d969255cb63075c94882733b82ba43831341eb1aadeea7b1fbfcd',
        'patched_sha256': 'd1d9995233601480c94c88aec6a87e0c35e8a2621ddf52487d2303382b1457e9',
        'original_guard': b'[[ mode_privacy -eq 0 ]]&&report_link=$(curl -$2 -s -X POST https://upload.check.place -d "type=net" --data-urlencode "json=$netdata" --data-urlencode "content=$net_report")\n',
        'patched_guard': b'[[ $apsp_public_report_upload == true && mode_privacy -eq 0 ]]&&report_link=$(curl -$2 -s -X POST https://upload.check.place -d "type=net" --data-urlencode "json=$netdata" --data-urlencode "content=$net_report")\n',
    },
}
NET_OUTPUT = b'[[ mode_json -eq 1 || mode_output -eq 1 || mode_privacy -eq 0 ]]&&save_json $2\n'


def exact_replace(content, before, after):
    if content.count(before) != 1:
        raise ValueError('exact report policy anchor must occur exactly once')
    return content.replace(before, after, 1)


def transform(role, content):
    if role not in SOURCES:
        raise ValueError('unknown fixed public report role')
    if not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('public report source exceeds its byte limit')
    spec = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != spec['source_sha256']:
        raise ValueError('canonical public report source SHA256 mismatch')
    result = exact_replace(content, b'check_bash(){\n', POLICY + b'check_bash(){\n')
    result = exact_replace(result, spec['original_guard'], spec['patched_guard'])
    if role == 'net.sh':
        # HW/IP already initialize the link; prevent an inherited stale Net link.
        result = exact_replace(result, NET_OUTPUT, b'local report_link=""\n' + NET_OUTPUT)
    if hashlib.sha256(result).hexdigest() != spec['patched_sha256']:
        raise ValueError('patched public report source SHA256 mismatch')
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('role', choices=tuple(SOURCES))
    args = parser.parse_args()
    content = sys.stdin.buffer.read(MAX_SOURCE + 1)
    sys.stdout.buffer.write(transform(args.role, content))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, TypeError) as error:
        raise SystemExit('Error: ' + str(error)) from None
