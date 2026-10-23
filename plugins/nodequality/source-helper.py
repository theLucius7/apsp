#!/usr/bin/env python3
"""Validate canonical sources and serve fixed script and reference policies."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys

MAX_FILE = 2 * 1024 * 1024
MAX_BUNDLE = 8 * 1024 * 1024
OFFICIAL_IP_SHA256 = 'b0de7a19cbe01a3cd6a1f4c00902bf97f36e99fce63933c95064eb44b2867be8'
PUBLIC_ACCESS_POLICY_SHA256 = '604c29102c145a38282d779e6236871198ecec096adde1593ad6b0da8043b846'
BROWSER_POLICY_SHA256 = 'a7d0f1bdfc9f2002b20770ef43eee3f352897ab70178e1f2a3ff9858fffc8f56'
NETFLIX_POLICY_SHA256 = '38b077bc7430fae4fba6eaa6570f738333a12c267e1c329ac92a86d9148cd8d5'
IP_SCORE_POLICY_SHA256 = '9ef1a5cf9a6c442d5abdf8ee91f2b42458c940a49f74c6d13a649cc5fb2ce303'
RANKING_POLICY_SHA256 = '4b6bdcfb8f03268479fd5eb70a00cdebb25b8fcab513dfc62bff226dd450d6c8'
LOADER_POLICY_SHA256 = '4a5c75b5756df7f3959229fbcfc61c467adc46bec2f232d911bf8e09a1173cab'
DATA_POLICY_SHA256 = '0115f90f8ce521eab1472d8426b8f8dfbf1fefca427ae0fdfd557b346a8fdab3'
DEPENDENCY_POLICY_SHA256 = '15e9ead93061a7bd2da3bf54a7244123ec6b4e1161695317a5fb9b539399d2b6'
REPORT_POLICY_SHA256 = '468311a213e820f920a9f556dfaaf6883002232d459c9e36d2622db2d7b43b26'
REPORT_ROLES = frozenset({'hardware.sh', 'ip.sh', 'net.sh'})
SWAP_POLICY_SHA256 = '63518145aa8e15d7778c6d2a26f54e6d7349d718b4cff244ec1d81c32b8d8187'
FILES = {
    'NodeQuality.sh': ('LloydAsp/NodeQuality', 'NodeQuality.sh', 'LICENSE.nodequality'),
    'header.sh': ('LloydAsp/NodeQuality', 'part/header.sh', 'LICENSE.nodequality'),
    'swap.sh': ('LloydAsp/NodeQuality', 'part/swap.sh', 'LICENSE.nodequality'),
    'hardware.sh': ('xykt/HardwareQuality', 'hardware.sh', 'LICENSE.hardware'),
    'ip.sh': ('xykt/IPQuality', 'ip.sh', 'LICENSE.ip'),
    'net.sh': ('xykt/NetQuality', 'net.sh', 'LICENSE.net'),
    'LICENSE.nodequality': ('LloydAsp/NodeQuality', 'LICENSE', None),
    'LICENSE.hardware': ('xykt/HardwareQuality', 'LICENSE', None),
    'LICENSE.ip': ('xykt/IPQuality', 'LICENSE', None),
    'LICENSE.net': ('xykt/NetQuality', 'LICENSE', None),
    'ip-iso3166.json': ('xykt/IPQuality', 'ref/iso3166.json', 'LICENSE.ip'),
    'ip-dnsbl.list': ('xykt/IPQuality', 'ref/dnsbl.list', 'LICENSE.ip'),
    'net-iso3166.json': ('xykt/NetQuality', 'ref/iso3166.json', 'LICENSE.net'),
    'net-province.json': ('xykt/NetQuality', 'ref/province.json', 'LICENSE.net'),
    'net-AS_Mapping.txt': ('xykt/NetQuality', 'ref/AS_Mapping.txt', 'LICENSE.net'),
    'net-iperf.json': ('xykt/NetQuality', 'ref/iperf.json', 'LICENSE.net'),
    'net-speedtest_cn.json': ('xykt/NetQuality', 'ref/speedtest_cn.json', 'LICENSE.net'),
}
ALIASES = {
    'https://raw.githubusercontent.com/LloydAsp/NodeQuality/refs/heads/main/part/header.sh': 'header.sh',
    'https://raw.githubusercontent.com/LloydAsp/NodeQuality/refs/heads/main/part/swap.sh': 'swap.sh',
    'https://Hardware.Check.Place': 'hardware.sh',
    'https://IP.Check.Place': 'ip.sh',
    'https://Net.Check.Place': 'net.sh',
}
FULL_EXECUTION_UNRESOLVED = [
    'complete-rootfs-and-secondary-tool-provenance',
    'version-specific-redistribution-and-unattended-execution-rights',
    'third-party-tool-upload-control',
    'complete-host-side-effect-and-cancellation-acceptance',
]
OBSERVED_ROOTFS = {
    'amd64': (312475959, '5f844e73941c3623175c5cdc16b01db34c155d0d1bd9b0cf71f3d72e8b1148e1'),
    'arm64': (359657375, 'a4dd4e55b129157a02dab437b78e41b5797a7a79a0f0b7febdecab8eb2a312c7'),
}


def ordinary(path, limit):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as stream:
        metadata = os.fstat(stream.fileno())
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > limit:
            raise ValueError('source input must be a bounded ordinary file')
        content = stream.read(limit + 1)
    if len(content) > limit:
        raise ValueError('source input exceeds its byte limit')
    return content


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate source manifest key')
        result[key] = value
    return result


def decode(content):
    return json.loads(content, object_pairs_hook=unique_object)


def execution_admission(path):
    # These observations are identities, never permission or runtime approval.
    record = decode(ordinary(path, 16384))
    expected = dict(
        schema=1, profile='daily-and-historical-recovery-only',
        full_start_allowed=False, runtime_dependency_downloads_allowed=False,
        first_level_source_files=len(FILES), unresolved=FULL_EXECUTION_UNRESOLVED,
        rootfs_observations={
            arch: dict(version='v0.0.2', size=size, sha256=digest,
                       upstream_signature_verified=False,
                       reproducible_recipe_verified=False,
                       redistribution_rights_verified=False)
            for arch, (size, digest) in OBSERVED_ROOTFS.items()},
        proprietary_tool_rights={
            'ookla-speedtest-1.2.0.84': 'not_verified',
            'geekbench-5.5.1': 'not_verified'})
    # JSON bool/int equality must not accept false=0 or schema=true.
    canonical = lambda value: json.dumps(value, sort_keys=True, separators=(',', ':'))
    if canonical(record) != canonical(expected):
        raise ValueError('execution admission must retain the unverified complete-toolchain gate')
    return record


def official_ip_identity():
    path = Path(__file__).with_name('official-ip.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != OFFICIAL_IP_SHA256:
        raise ValueError('signed official IP helper SHA256 mismatch')


def validate(lock):
    if (not isinstance(lock, dict) or set(lock) != {'schema', 'files'}
            or type(lock['schema']) is not int or lock['schema'] != 1):
        raise ValueError('unsupported source manifest')
    rows = {}
    if not isinstance(lock['files'], list):
        raise ValueError('source manifest requires files')
    for row in lock['files']:
        if not isinstance(row, dict) or row.get('name') not in FILES or row['name'] in rows:
            raise ValueError('unknown or duplicate source file')
        repository, source_path, license_file = FILES[row['name']]
        expected = {'name', 'repository', 'commit', 'path', 'sha256', 'size'}
        if license_file:
            expected.add('license_file')
        if set(row) != expected or row['repository'] != repository or row['path'] != source_path:
            raise ValueError('source identity differs from its pinned role')
        if license_file and row['license_file'] != license_file:
            raise ValueError('source license identity mismatch')
        if not isinstance(row['commit'], str) or not re.fullmatch('[0-9a-f]{40}', row['commit']):
            raise ValueError('source commit must be a full immutable SHA')
        if not isinstance(row['sha256'], str) or not re.fullmatch('[0-9a-f]{64}', row['sha256']):
            raise ValueError('source requires an exact SHA256')
        if type(row['size']) is not int or not 0 < row['size'] <= MAX_FILE:
            raise ValueError('source size exceeds its byte limit')
        rows[row['name']] = row
    if set(rows) != set(FILES):
        raise ValueError('source manifest lacks a script, reference or full license')
    for row in rows.values():
        related = [other for other in rows.values() if other['repository'] == row['repository']]
        if len({other['commit'] for other in related}) != 1:
            raise ValueError('one repository must have one pinned source commit')
    return rows


def verified(content, row):
    if len(content) != row['size'] or hashlib.sha256(content).hexdigest() != row['sha256']:
        raise ValueError('pinned source checksum or size mismatch: ' + row['name'])
    return content


def receive(path, stream):
    # Enforce the disk bound independently of curl version/Content-Length.
    content = stream.read(MAX_FILE + 1)
    if not content or len(content) > MAX_FILE:
        raise ValueError('download is empty or exceeds its byte limit')
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(descriptor, 'wb') as output:
            os.fchmod(output.fileno(), 0o600)
            output.write(content)
    except BaseException:
        path.unlink(missing_ok=True)
        raise


def report_policy():
    path = Path(__file__).with_name('report-policy.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != REPORT_POLICY_SHA256:
        raise ValueError('signed report policy helper SHA256 mismatch')
    namespace = {'__name__': 'apsp_report_policy', '__file__': str(path)}
    # Execute only the exact verified helper bytes, never a second path lookup.
    # Its bounded transform does not execute upstream code or perform I/O.
    exec(compile(content, str(path), 'exec'), namespace)
    return namespace


def swap_policy():
    path = Path(__file__).with_name('swap-policy.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != SWAP_POLICY_SHA256:
        raise ValueError('signed swap policy helper SHA256 mismatch')
    namespace = {'__name__': 'apsp_swap_policy', '__file__': str(path)}
    exec(compile(content, str(path), 'exec'), namespace)
    return namespace


def without_swap(name, content):
    policy = swap_policy()
    result = policy['transform'](name, content)
    if (not isinstance(result, bytes) or len(result) > MAX_FILE + 2048
            or hashlib.sha256(result).hexdigest() != policy['SOURCES'][name]['patched_sha256']):
        raise ValueError('served swap policy output SHA256 or byte limit mismatch')
    return result


def dependency_policy():
    path = Path(__file__).with_name('dependency-policy.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != DEPENDENCY_POLICY_SHA256:
        raise ValueError('signed dependency policy helper SHA256 mismatch')
    namespace = {'__name__': 'apsp_dependency_policy', '__file__': str(path)}
    exec(compile(content, str(path), 'exec'), namespace)
    return namespace


def offline_dependencies(name, content):
    policy = dependency_policy()
    result = policy['transform'](name, content)
    if (not isinstance(result, bytes) or len(result) > MAX_FILE + 4096
            or hashlib.sha256(result).hexdigest() != policy['SOURCES'][name]['patched_sha256']):
        raise ValueError('served dependency policy output SHA256 or byte limit mismatch')
    return result


def entrypoint(bundle):
    rows = validate(bundle['lock'])
    original = verified(base64.b64decode(bundle['files']['NodeQuality.sh'], validate=True), rows['NodeQuality.sh'])
    prior = offline_dependencies('NodeQuality.sh', without_swap('NodeQuality.sh', original))
    policy = loader_policy()
    result = policy['transform']('NodeQuality.sh', prior)
    if (not isinstance(result, bytes) or len(result) > MAX_FILE + 4096
            or hashlib.sha256(result).hexdigest() != policy['SOURCES']['NodeQuality.sh']['patched_sha256']):
        raise ValueError('entrypoint loader output SHA256 or byte limit mismatch')
    return result


def loader_policy():
    path = Path(__file__).with_name('loader-policy.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != LOADER_POLICY_SHA256:
        raise ValueError('signed loader policy helper SHA256 mismatch')
    namespace = {'__name__': 'apsp_loader_policy', '__file__': str(path)}
    exec(compile(content, str(path), 'exec'), namespace)
    return namespace


def data_policy():
    path = Path(__file__).with_name('data-policy.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != DATA_POLICY_SHA256:
        raise ValueError('signed static data policy helper SHA256 mismatch')
    namespace = {'__name__': 'apsp_data_policy', '__file__': str(path)}
    exec(compile(content, str(path), 'exec'), namespace)
    return namespace


def ranking_policy():
    path = Path(__file__).with_name('ranking-policy.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != RANKING_POLICY_SHA256:
        raise ValueError('signed ranking policy helper SHA256 mismatch')
    namespace = {'__name__': 'apsp_ranking_policy', '__file__': str(path)}
    exec(compile(content, str(path), 'exec'), namespace)
    return namespace


def ip_score_policy():
    path = Path(__file__).with_name('ip-score-policy.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != IP_SCORE_POLICY_SHA256:
        raise ValueError('signed IP score policy helper SHA256 mismatch')
    namespace = {'__name__': 'apsp_ip_score_policy', '__file__': str(path)}
    exec(compile(content, str(path), 'exec'), namespace)
    return namespace


def validated_ip_scores(name, content):
    if name != 'ip.sh':
        return content
    policy = ip_score_policy()
    result = policy['transform'](name, content)
    if (not isinstance(result, bytes) or len(result) > MAX_FILE + 4096
            or hashlib.sha256(result).hexdigest() != policy['SOURCES'][name]['patched_sha256']):
        raise ValueError('served IP score policy output SHA256 or byte limit mismatch')
    return result


def netflix_policy():
    path = Path(__file__).with_name('netflix-policy.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != NETFLIX_POLICY_SHA256:
        raise ValueError('signed Netflix policy helper SHA256 mismatch')
    namespace = {'__name__': 'apsp_netflix_policy', '__file__': str(path)}
    exec(compile(content, str(path), 'exec'), namespace)
    return namespace


def validated_netflix(name, content):
    if name != 'ip.sh':
        return content
    policy = netflix_policy()
    result = policy['transform'](name, content)
    if (not isinstance(result, bytes) or len(result) > MAX_FILE + 8192
            or hashlib.sha256(result).hexdigest() != policy['SOURCES'][name]['patched_sha256']):
        raise ValueError('served Netflix policy output SHA256 or byte limit mismatch')
    return result


def browser_policy():
    path = Path(__file__).with_name('browser-policy.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != BROWSER_POLICY_SHA256:
        raise ValueError('signed browser policy helper SHA256 mismatch')
    namespace = {'__name__': 'apsp_browser_policy', '__file__': str(path)}
    exec(compile(content, str(path), 'exec'), namespace)
    return namespace


def native_curl_identity(name, content):
    if name not in ('ip.sh', 'net.sh'):
        return content
    policy = browser_policy()
    result = policy['transform'](name, content)
    if (not isinstance(result, bytes) or len(result) > MAX_FILE + 8192
            or hashlib.sha256(result).hexdigest() != policy['SOURCES'][name]['patched_sha256']):
        raise ValueError('served browser policy output SHA256 or byte limit mismatch')
    return result


def public_access_policy():
    path = Path(__file__).with_name('public-access-policy.py')
    content = ordinary(path, 65536)
    if hashlib.sha256(content).hexdigest() != PUBLIC_ACCESS_POLICY_SHA256:
        raise ValueError('signed public access policy helper SHA256 mismatch')
    namespace = {'__name__': 'apsp_public_access_policy', '__file__': str(path)}
    exec(compile(content, str(path), 'exec'), namespace)
    return namespace


def authorized_access(name, content):
    if name != 'ip.sh':
        return content
    policy = public_access_policy()
    result = policy['transform'](name, content)
    if (not isinstance(result, bytes) or len(result) > MAX_FILE + 16384
            or hashlib.sha256(result).hexdigest() != policy['SOURCES'][name]['patched_sha256']):
        raise ValueError('served public access policy output SHA256 or byte limit mismatch')
    return result


def percentile_policy(name, content):
    if name != 'hardware.sh':
        return content
    policy = ranking_policy()
    result = policy['transform'](name, content)
    if (not isinstance(result, bytes) or len(result) > MAX_FILE + 4096
            or hashlib.sha256(result).hexdigest() != policy['SOURCES'][name]['patched_sha256']):
        raise ValueError('served ranking policy output SHA256 or byte limit mismatch')
    return result


def static_references(name, content, directory, rows):
    policy = data_policy()
    if name not in policy['SOURCES']:
        return content
    files = {filename: verified(ordinary(directory / filename, MAX_FILE), rows[filename])
             for filename in policy['REQUESTS'][name].values()}
    result = policy['transform'](name, content, files)
    if (not isinstance(result, bytes) or len(result) > MAX_FILE
            or hashlib.sha256(result).hexdigest() != policy['SOURCES'][name]['patched_sha256']):
        raise ValueError('served static data output SHA256 or byte limit mismatch')
    return result


def pack(lock, directory):
    rows = validate(lock)
    execution_admission(Path(__file__).with_name('execution-admission.json'))
    official_ip_identity()
    report_policy()
    swap_policy()
    dependency_policy()
    data_policy()
    loader_policy()
    ranking_policy()
    ip_score_policy()
    netflix_policy()
    browser_policy()
    public_access_policy()
    files = {name: base64.b64encode(verified(ordinary(directory / name, MAX_FILE), row)).decode()
             for name, row in rows.items()}
    result = (json.dumps(dict(schema=1, lock=lock, files=files), sort_keys=True, separators=(',', ':')) + '\n').encode()
    if len(result) > MAX_BUNDLE:
        raise ValueError('source bundle exceeds its byte limit')
    return result


def materialize(bundle, directory):
    if (not isinstance(bundle, dict) or set(bundle) != {'schema', 'lock', 'files'}
            or type(bundle['schema']) is not int or bundle['schema'] != 1):
        raise ValueError('unsupported source bundle')
    rows = validate(bundle['lock'])
    if not isinstance(bundle['files'], dict) or set(bundle['files']) != set(rows):
        raise ValueError('source bundle lacks an exact file set')
    content = {name: verified(base64.b64decode(bundle['files'][name], validate=True), row)
               for name, row in rows.items()}
    directory.mkdir(mode=0o700, exist_ok=False)
    for name, value in content.items():
        with (directory / name).open('xb') as output:
            os.fchmod(output.fileno(), 0o600)
            output.write(value)
    with (directory / 'source-lock.json').open('x') as output:
        os.fchmod(output.fileno(), 0o600)
        json.dump(bundle['lock'], output, sort_keys=True)


def serve(directory, arguments):
    if len(arguments) != 2 or arguments[0] not in ('-sL', '-Ls') or arguments[1] not in ALIASES:
        raise ValueError('unknown first-level source request; online fallback is forbidden')
    if directory.is_symlink() or not directory.is_dir():
        raise ValueError('source directory must be private and ordinary')
    rows = validate(decode(ordinary(directory / 'source-lock.json', 65536)))
    name = ALIASES[arguments[1]]
    canonical = verified(ordinary(directory / name, MAX_FILE), rows[name])
    if name not in REPORT_ROLES:
        return canonical
    policy = report_policy()
    patched = policy['transform'](name, canonical)
    if (not isinstance(patched, bytes) or len(patched) > MAX_FILE + 2048
            or hashlib.sha256(patched).hexdigest() != policy['SOURCES'][name]['patched_sha256']):
        raise ValueError('served report policy output SHA256 or byte limit mismatch')
    prior = without_swap(name, patched) if name == 'hardware.sh' else patched
    scored = validated_ip_scores(name, percentile_policy(name, static_references(name, offline_dependencies(name, prior), directory, rows)))
    return authorized_access(name, native_curl_identity(name, validated_netflix(name, scored)))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('operation', choices=['downloads', 'receive', 'pack', 'materialize', 'serve', 'entrypoint', 'admission'])
    parser.add_argument('input', type=Path)
    parser.add_argument('remaining', nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.operation == 'admission':
        if args.remaining:
            raise ValueError('admission accepts only an execution record')
        print(json.dumps(execution_admission(args.input), sort_keys=True, separators=(',', ':')))
        return
    if args.operation == 'entrypoint':
        if args.remaining:
            raise ValueError('entrypoint accepts only a pinned bundle')
        sys.stdout.buffer.write(entrypoint(decode(ordinary(args.input, MAX_BUNDLE))))
        return
    if args.operation == 'receive':
        if args.remaining:
            raise ValueError('receive accepts only a new private target')
        receive(args.input, sys.stdin.buffer)
        return
    if args.operation == 'serve':
        sys.stdout.buffer.write(serve(args.input, args.remaining))
        return
    if args.operation == 'materialize':
        if len(args.remaining) != 1:
            raise ValueError('materialize requires a private destination')
        materialize(decode(ordinary(args.input, MAX_BUNDLE)), Path(args.remaining[0]))
        return
    lock = decode(ordinary(args.input, 65536))
    rows = validate(lock)
    if args.operation == 'downloads':
        if args.remaining:
            raise ValueError('downloads accepts only the fixed manifest')
        for name, row in sorted(rows.items()):
            print(name + '\thttps://raw.githubusercontent.com/' + row['repository'] + '/' + row['commit'] + '/' + row['path'])
    else:
        if len(args.remaining) != 1:
            raise ValueError('pack requires a private source directory')
        sys.stdout.buffer.write(pack(lock, Path(args.remaining[0])))


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, TypeError, KeyError) as error:
        raise SystemExit('Error: ' + str(error)) from None
