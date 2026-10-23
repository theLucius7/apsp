#!/usr/bin/env python3
"""Run the pinned IPQuality inside an offline, privately mounted rootfs."""
import argparse
import contextlib
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import platform
import re
import resource
import signal
import stat
import subprocess
import sys
import time
import uuid

VERSION = '87397e2c3196ec796f5477c83343c2354df601ea-node-r1'
SOURCE_COMMIT = '87397e2c3196ec796f5477c83343c2354df601ea'
SOURCE_SHA256 = 'b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf'
MAX_SECTION = 64 * 1024
MAX_ATTEMPTS = 64
MAX_RAW = 48 * 1024
MAX_LOG = 256 * 1024
HASH = re.compile(r'[0-9a-f]{64}\Z')
ROOTFS_SOURCE = '@ROOTFS_HELPER@'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        require(key not in value, 'duplicate JSON key')
        value[key] = item
    return value


def decode(content):
    return json.loads(content, object_pairs_hook=unique_object,
                      parse_constant=lambda _: require(False, 'non-finite JSON number'))


def canonical(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True,
                      separators=(',', ':'), allow_nan=False).encode() + b'\n'


def runtime():
    scope = {'__name__': 'apsp_ipquality_rootfs'}
    require(ROOTFS_SOURCE != '@ROOTFS_HELPER@', 'runner must be packaged with the fixed rootfs verifier')
    exec(compile(ROOTFS_SOURCE, '<signed-rootfs-verifier>', 'exec'), scope)
    return type('Rootfs', (), scope)


def ordinary(path, maximum):
    helper = runtime()
    return helper.ordinary(path, maximum)


def atomic(path, content):
    require(0 < len(content) <= MAX_SECTION, 'report section exceeds its byte limit')
    temporary = path.with_name('.' + path.name + '-' + uuid.uuid4().hex)
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(descriptor, 'wb') as target:
            target.write(content)
            target.flush()
            os.fsync(target.fileno())
        os.replace(temporary, path)
        parent = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(parent)
        finally:
            os.close(parent)
    finally:
        temporary.unlink(missing_ok=True)


def identity(directory, arch):
    info = decode(ordinary(directory / 'build-info.json', 16 * 1024))
    fields = {'schema', 'plugin', 'version', 'arch', 'profile', 'source_commit', 'source_sha256',
              'source_lock_sha256', 'policy_sha256', 'transport_sha256', 'rootfs_sha256',
              'rootfs_manifest_sha256', 'license_review_sha256', 'source_archive_sha256',
              'factory_provenance_sha256'}
    require(isinstance(info, dict) and set(info) == fields and type(info['schema']) is int
            and info['schema'] == 1 and info['plugin'] == 'ipquality'
            and info['version'] == VERSION and info['profile'] == 'ipquality-node-v1'
            and info['arch'] == arch and info['source_commit'] == SOURCE_COMMIT
            and info['source_sha256'] == SOURCE_SHA256, 'IPQuality build identity differs')
    require(all(isinstance(info[key], str) and HASH.fullmatch(info[key])
                for key in fields if key.endswith('_sha256')), 'invalid build input digest')
    manifest_bytes = ordinary(directory / 'rootfs-manifest.json', 8 * 1024 * 1024)
    require(hashlib.sha256(manifest_bytes).hexdigest() == info['rootfs_manifest_sha256'],
            'rootfs manifest changed')
    manifest = runtime().load_manifest(manifest_bytes, arch)
    require(manifest['archive']['sha256'] == info['rootfs_sha256'], 'rootfs identity differs')
    return info, manifest


def operation(arguments):
    result = subprocess.run(arguments, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                            stderr=subprocess.PIPE, timeout=5, check=False,
                            env={'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LC_ALL': 'C'})
    require(result.returncode == 0, 'private mount operation failed')


def child_limits():
    # Protect diagnostic files even when the process exits between observations.
    resource.setrlimit(resource.RLIMIT_FSIZE, (MAX_LOG, MAX_LOG))


def valid_egress(upstream, family):
    try:
        value = upstream['Head']['IP']
        address = ipaddress.ip_address(value)
        require(address.version == int(family) and address.is_global and str(address) == value,
                'upstream egress identity is invalid')
        return value
    except (ValueError, KeyError, TypeError):
        return None


def snapshot(workspace, args, started, finished, revision):
    attempts_path = workspace / 'attempts.jsonl'
    attempts = []
    if attempts_path.exists():
        raw = ordinary(attempts_path, MAX_SECTION)
        for line in raw.splitlines():
            # A killed append can leave its last line incomplete; completed lines survive.
            try:
                receipt = decode(line)
            except (ValueError, UnicodeError):
                break
            require(isinstance(receipt, dict), 'invalid transport receipt')
            attempts.append(receipt)
            require(len(attempts) <= MAX_ATTEMPTS, 'too many transport receipts')
    upstream = None
    for name in ('upstream.json', 'partial.json'):
        path = workspace / name
        if path.exists():
            try:
                candidate = decode(ordinary(path, MAX_RAW))
                if isinstance(candidate, dict) and candidate:
                    upstream = candidate
                    break
            except (ValueError, UnicodeError):
                pass
    discovered = set()
    for receipt in attempts:
        if receipt.get('provider') == 'egress-discovery' and receipt.get('status') == 'succeeded':
            candidate = valid_egress({'Head': {'IP': receipt.get('target_ip')}}, args.ip_version)
            if candidate:
                discovered.add(candidate)
    egress = discovered.pop() if len(discovered) == 1 else None
    head = upstream.get('Head') if upstream is not None else None
    if upstream is not None and (egress is None or valid_egress(upstream, args.ip_version) != egress
                                 or not isinstance(head, dict) or head.get('Version') != 'v2026-09-16'):
        upstream = None
    envelope = {'schema': 1, 'plugin': 'ipquality', 'version': VERSION,
                'job_id': args.job_id, 'ip_version': args.ip_version,
                'artifact_sha256': args.artifact_sha256, 'source_commit': SOURCE_COMMIT,
                'source_sha256': SOURCE_SHA256, 'started_at': started,
                'finished_at': finished, 'egress_ip': egress, 'upstream': upstream,
                'attempts': attempts}
    payload = canonical(envelope)
    # Keep failure receipts when an upstream unexpectedly exceeds the joint section bound.
    if len(payload) > MAX_SECTION - 1024:
        envelope['upstream'] = None
        payload = canonical(envelope)
    atomic(workspace / 'result.json', payload)
    section = {'name': 'ipquality_result', 'text': payload.decode(), 'complete': finished is not None,
               'revision': revision, 'collected_at': finished or int(time.time())}
    atomic(workspace / 'section-ipquality_result.json', canonical(section))


def execute(args, directory):
    require(sys.platform == 'linux' and os.geteuid() == 0, 'IPQuality requires a managed Linux job')
    require(os.readlink('/proc/self/ns/mnt') != os.readlink('/proc/1/ns/mnt'),
            'IPQuality requires a private mount namespace')
    require(str(uuid.UUID(args.job_id)) == args.job_id, 'invalid job identity')
    require(HASH.fullmatch(args.artifact_sha256) is not None, 'invalid signed artifact digest')
    workspace = Path(args.workspace)
    require(workspace.is_absolute() and str(workspace) != '/' and workspace.resolve() == workspace,
            'workspace must be a canonical absolute directory')
    helper = runtime()
    descriptor = helper._directory(str(workspace))
    try:
        metadata = os.fstat(descriptor)
        require(metadata.st_uid == os.geteuid() and stat.S_IMODE(metadata.st_mode) == 0o700,
                'workspace must be owned and private')
    finally:
        os.close(descriptor)
    for name in ('result.json', 'attempts.jsonl', 'upstream.json', 'partial.json',
                 'section-ipquality_result.json', '.ipquality-run', 'IpRoot'):
        require(not os.path.lexists(workspace / name), 'workspace already contains execution state')
    marker = os.open(workspace / '.ipquality-run', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    os.close(marker)
    arch = {'x86_64': 'amd64', 'aarch64': 'arm64', 'arm64': 'arm64'}.get(platform.machine())
    require(arch is not None, 'unsupported node architecture')
    info, manifest = identity(directory, arch)
    del info
    started, revision = int(time.time()), 1
    end = time.monotonic() + 280
    root, child, mounts = workspace / 'IpRoot', None, []
    interrupted = False

    def stop(_number, _frame):
        nonlocal interrupted
        interrupted = True

    handlers = {number: signal.signal(number, stop)
                for number in (signal.SIGINT, signal.SIGTERM, signal.SIGHUP)}
    try:
        helper.extract(directory / 'rootfs.tar.gz', manifest, str(workspace), arch, 'IpRoot')
        require(not interrupted and time.monotonic() < end, 'IPQuality preparation interrupted')
        (root / 'work').mkdir(mode=0o700, exist_ok=True)
        (workspace / 'tmp').mkdir(mode=0o700)
        resolver = root / 'etc/resolv.conf'
        require(not resolver.is_symlink() and resolver.is_file(), 'rootfs resolver must be ordinary')
        with Path('/etc/resolv.conf').open('rb') as source:
            resolver_bytes = source.read(64 * 1024 + 1)
        require(len(resolver_bytes) <= 64 * 1024, 'host resolver exceeds its byte bound')
        resolver.write_bytes(resolver_bytes)
        (root / 'proc').mkdir(mode=0o755, exist_ok=True)
        (root / 'dev').mkdir(mode=0o755, exist_ok=True)
        for name in ('null', 'urandom', 'random'):
            target = root / 'dev' / name
            require(not os.path.lexists(target), 'rootfs contains an unexpected device')
            target.touch(mode=0o600)
        require(not os.path.lexists(root / 'dev/fd'), 'rootfs contains an unexpected fd alias')
        (root / 'dev/fd').symlink_to('/proc/self/fd')
        operation(['/bin/mount', '--bind', str(root), str(root)])
        mounts.append(root)
        operation(['/bin/mount', '-o', 'remount,bind,ro', str(root)])
        operation(['/bin/mount', '--bind', str(workspace), str(root / 'work')])
        mounts.append(root / 'work')
        operation(['/bin/mount', '-t', 'proc', '-o', 'ro,nosuid,nodev,noexec', 'proc', str(root / 'proc')])
        mounts.append(root / 'proc')
        for name in ('null', 'urandom', 'random'):
            target = root / 'dev' / name
            operation(['/bin/mount', '--bind', '/dev/' + name, str(target)])
            mounts.append(target)
            operation(['/bin/mount', '-o', 'remount,bind,ro', str(target)])
        require(not interrupted, 'IPQuality was cancelled before execution')
        environment = {'PATH': '/usr/local/bin:/usr/bin:/bin', 'LANG': 'C', 'LC_ALL': 'C',
                       'HOME': '/work', 'TMPDIR': '/work/tmp',
                       'APSP_IPQUALITY_ATTEMPTS': '/work/attempts.jsonl',
                       'APSP_IPQUALITY_PARTIAL': '/work/partial.json',
                       'APSP_IPQUALITY_FAMILY': args.ip_version,
                       'APSP_IPQUALITY_JOB_UUID': args.job_id}
        with (workspace / 'upstream.json').open('xb') as output, (workspace / 'log.txt').open('xb') as log:
            child = subprocess.Popen(['/usr/sbin/chroot', str(root), '/bin/bash',
                                      '/usr/local/lib/apsp-ipquality/patched-ip.sh', args.ip_version],
                                     stdin=subprocess.DEVNULL, stdout=output, stderr=log,
                                     env=environment, start_new_session=True, preexec_fn=child_limits)
            published = time.monotonic()
            while child.poll() is None and not interrupted and time.monotonic() < end:
                require(output.tell() <= MAX_RAW and log.tell() <= MAX_LOG, 'IPQuality output limit exceeded')
                if time.monotonic() - published >= 2:
                    snapshot(workspace, args, started, None, revision)
                    revision += 1
                    published = time.monotonic()
                time.sleep(0.1)
            completed = child.poll() is not None and not interrupted
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=2)
            require((workspace / 'upstream.json').stat().st_size <= MAX_RAW
                    and (workspace / 'log.txt').stat().st_size <= MAX_LOG,
                    'IPQuality final output exceeds its byte limit')
            snapshot(workspace, args, started, int(time.time()) if completed else None, revision)
            return 0 if completed and child.returncode == 0 else 1
    finally:
        if child is not None:
            # The Bash parent can exit before a failed helper; drain its entire group.
            with contextlib.suppress(ProcessLookupError):
                os.killpg(child.pid, signal.SIGKILL)
            with contextlib.suppress(subprocess.TimeoutExpired):
                child.wait(timeout=2)
        cleanup_errors = []
        for target in reversed(mounts):
            try:
                operation(['/bin/umount', '--', str(target)])
            except (ValueError, OSError, subprocess.TimeoutExpired):
                cleanup_errors.append(str(target.relative_to(workspace)))
        for number, handler in handlers.items():
            signal.signal(number, handler)
        require(not cleanup_errors, 'private rootfs cleanup remains unconfirmed')


def main():
    if sys.argv[1:] == ['--version']:
        print('ipquality ' + VERSION)
        return 0
    directory = Path(__file__).resolve().parent
    if sys.argv[1:] == ['--build-info']:
        print(ordinary(directory / 'build-info.json', 16 * 1024).decode(), end='')
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--workspace', required=True)
    parser.add_argument('--job-id', required=True)
    parser.add_argument('--ip-version', choices=('4', '6'), required=True)
    parser.add_argument('--artifact-sha256', required=True)
    try:
        return execute(parser.parse_args(), directory)
    except (ValueError, OSError, subprocess.TimeoutExpired) as error:
        print('IPQuality: ' + str(error), file=sys.stderr)
        return 70


if __name__ == '__main__':
    raise SystemExit(main())
