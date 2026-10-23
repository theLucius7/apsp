#!/usr/bin/env python3
"""Sign disposable CI fixtures with the publicly disclosed TEST_ONLY key only."""
import base64
import hashlib
import io
import json
import os
import subprocess
from pathlib import Path
import platform
import tarfile

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / 'crates/protocol/tests/fixtures'
COMMENT = 'Apsp TEST ONLY CI fixture; never publish as an official release'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def sign(data):
    # No configurable private key: production signing is deliberately impossible.
    signer = os.environ.get('APSP_CI_FIXTURE_SIGNER')
    if signer:
        result = subprocess.run([signer], input=data, capture_output=True, check=True, timeout=30)
        return result.stdout.decode('utf-8')
    try:
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    except ImportError:
        import shutil
        import tempfile
        verifier = shutil.which('minisign')
        if not verifier:
            raise RuntimeError('TEST_ONLY fixture signing needs cryptography or minisign') from None
        with tempfile.TemporaryDirectory(prefix='apsp-test-sign-') as directory:
            message = Path(directory) / 'checksums'
            message.write_bytes(data)
            subprocess.run([verifier, '-S', '-q', '-m', str(message), '-s', str(FIXTURES / 'TEST_ONLY.key'),
                            '-t', COMMENT], check=True, capture_output=True)
            return message.with_name(message.name + '.minisig').read_text()
    private = base64.b64decode((FIXTURES / 'TEST_ONLY.key').read_text().splitlines()[1], validate=True)
    public = base64.b64decode((FIXTURES / 'TEST_ONLY.pub').read_text().splitlines()[1], validate=True)
    assert len(private) == 158 and private[2:4] == b'\0\0'
    assert private[54:62] == public[2:10]
    key = Ed25519PrivateKey.from_private_bytes(private[62:94])
    signature = key.sign(hashlib.blake2b(data).digest())
    record = b'ED' + public[2:10] + signature
    global_signature = key.sign(signature + COMMENT.encode())
    return ('untrusted comment: ' + COMMENT + '\n' + base64.b64encode(record).decode() + '\n'
            + 'trusted comment: ' + COMMENT + '\n' + base64.b64encode(global_signature).decode() + '\n')


def architecture():
    return {'x86_64': 'amd64', 'amd64': 'amd64', 'aarch64': 'arm64', 'arm64': 'arm64'}[platform.machine().lower()]


def target():
    arch = architecture()
    system = platform.system().lower()
    if system == 'darwin':
        return 'macos-' + arch
    if system == 'linux':
        # Native Linux service tests run on GNU Ubuntu; static Agent uses legacy identity.
        return arch
    return system + '-' + arch


def asset_name(name, version, arch, format):
    if arch in ('amd64', 'arm64'):
        return f'{name}-{version}-linux-musl-{arch}' if format == 'raw' else f'{name}-{version}-linux-{arch}.tar.gz'
    return f'{name}-{version}-{arch}' + ('.tar.gz' if format == 'tar.gz' else '')


def proof(name, version, binary_name, data, format='raw', arch=None):
    arch = arch or target()
    files = {}
    if format == 'raw':
        binary = data
    else:
        with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
            for member in archive.getmembers():
                assert member.isfile() and '/' not in member.name and '\\' not in member.name
                content = archive.extractfile(member).read()
                files[member.name] = dict(sha256=digest(content), size=len(content))
        assert binary_name in files
        with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
            binary = archive.extractfile(binary_name).read()
    entry = dict(name=name, version=version, arch=arch, format=format, binary_name=binary_name,
                 archive_size=len(data), binary_sha256=digest(binary), binary_size=len(binary),
                 asset_name=asset_name(name, version, arch, format))
    if len(files) > 1:
        entry['auxiliary_files'] = {key: value for key, value in files.items() if key != binary_name}
    metadata_json = json.dumps(dict(schema=1, source_repo='theLucius7/apsp', tag='agent-v' + (version if name == 'agent' else '0.2.0'),
                              protocol_min=1, protocol_max=1, artifacts=[entry]), separators=(',', ':'))
    sums = {'release.json': digest(metadata_json.encode()), 'install.sh': digest(b'#!/bin/sh\nexit 0\n'),
            f'{name}/{version}/{arch}': digest(data)}
    checksums = ''.join(f'{sums[path]}  {path}\n' for path in sorted(sums))
    return dict(metadata_json=metadata_json, checksums=checksums, signature=sign(checksums.encode()))


def install(directory, release):
    for name, field in [('release.json', 'metadata_json'), ('SHA256SUMS', 'checksums'), ('SHA256SUMS.minisig', 'signature')]:
        (Path(directory) / name).write_bytes(release[field].encode('utf-8'))
