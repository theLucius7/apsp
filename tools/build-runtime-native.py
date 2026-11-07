#!/usr/bin/env python3
"""Build the stable upstream kernel with APSP limits (tools/singbox-runtime) for macOS, FreeBSD and Windows."""
import argparse
import gzip
import io
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import sys
import tempfile
import time

from artifact_manifest import publish

sys.path.insert(0, str(Path(__file__).resolve().parent / 'singbox-runtime'))
import apply as runtime_patches
import cronet

VERSION = '1.14.4-apsp.2'
PATCHED_COMMIT = '5d74251bd02a4b243a2cf23a6f0e7bdfc2259642'
COMMIT = 'c005f35300ce76427dc285481ec5695f5b179cd2'
# Server build tags, including the statistics API the Agent reads.
TAGS = 'with_quic,with_utls,with_grpc,with_wireguard,with_clash_api,with_v2ray_api,with_acme,with_gvisor,with_cloudflared,with_openvpn,with_openconnect,with_tailscale'
TARGETS = ('macos-arm64', 'freebsd-amd64', 'freebsd-arm64', 'windows-amd64', 'windows-arm64')


def run(args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def checkout(directory, url, commit):
    run(['git', 'init', str(directory)])
    run(['git', '-C', str(directory), 'remote', 'add', 'origin', url])
    # The patch queue and its pinned commit need upstream's exact bytes on Windows too.
    run(['git', '-C', str(directory), 'config', 'core.autocrlf', 'false'])
    run(['git', '-C', str(directory), 'fetch', '--depth=1', '--filter=blob:none', 'origin', commit])
    run(['git', '-C', str(directory), 'checkout', '--detach', 'FETCH_HEAD'])
    actual = run(['git', '-C', str(directory), 'rev-parse', 'HEAD'], capture_output=True, text=True).stdout.strip()
    if actual != commit:
        raise ValueError('upstream commit mismatch')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('target', choices=TARGETS)
    parser.add_argument('artifact_root', type=Path)
    args = parser.parse_args()
    system, arch = args.target.split('-')
    if system != 'freebsd' and system != {'Darwin': 'macos', 'Windows': 'windows'}.get(platform.system()):
        parser.error('macOS and Windows builds require their native hosts')
    goversion = run(['go', 'version'], capture_output=True, text=True).stdout
    if 'go1.26.8 ' not in goversion:
        parser.error('use the pinned upstream Go 1.26.8 toolchain')
    output = args.artifact_root.resolve() / 'sing-box' / VERSION
    if (output / args.target).exists():
        parser.error('immutable artifact already exists')
    with tempfile.TemporaryDirectory(prefix='apsp-runtime-') as temporary:
        root = Path(temporary)
        source = root / 'source'
        checkout(source, 'https://github.com/SagerNet/sing-box.git', COMMIT)
        if runtime_patches.apply(source) != PATCHED_COMMIT:
            raise ValueError('patch queue produced an unexpected commit')
        binary = root / ('sing-box.exe' if system == 'windows' else 'sing-box')
        tags = TAGS
        cgo = '0'
        if system != 'freebsd':
            tags += ',with_naive_outbound'
        if system == 'windows':
            tags += ',with_purego'
        if system == 'macos':
            cgo = '1'
        # macOS links the pinned static Cronet module using the local Xcode toolchain.
        env = dict(os.environ, GOOS='darwin' if system == 'macos' else system, GOARCH=arch,
                   CGO_ENABLED=cgo, GOTOOLCHAIN='local', GOFLAGS='')
        for attempt in range(3):
            try:
                run(['go', 'build', '-mod=readonly', '-trimpath', '-o', str(binary), '-tags', tags,
                     '-ldflags', f'-X github.com/sagernet/sing-box/constant.Version={VERSION} -s -w -buildid=',
                     './cmd/sing-box'], cwd=source, env=env)
                break
            except subprocess.CalledProcessError:
                if attempt == 2:
                    raise
                print('Retrying runtime build with the existing module cache', flush=True)
                time.sleep(5)
        files = [binary]
        if system == 'windows':
            files.append(cronet.fetch('windows_' + arch, root))
        if system != 'freebsd' or platform.system() == 'FreeBSD':
            text = run([str(binary), 'version'], capture_output=True, text=True).stdout
            if f'sing-box version {VERSION}' not in text or f'Tags: {tags}' not in text:
                raise ValueError('runtime version or build tags are wrong')
            print(text)
        archive = io.BytesIO()
        with gzip.GzipFile(fileobj=archive, mode='wb', mtime=0, filename='') as compressed:
            with tarfile.open(fileobj=compressed, mode='w') as tar:
                for path in files:
                    info = tarfile.TarInfo(path.name)
                    info.size = path.stat().st_size
                    info.mode = 0o755
                    with path.open('rb') as file:
                        tar.addfile(info, file)
        publish(output, args.target, archive.getvalue())
        print(f'Artifact: {output / args.target}')


if __name__ == '__main__':
    main()
