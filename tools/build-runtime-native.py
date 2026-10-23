#!/usr/bin/env python3
"""Build unmodified, pinned upstream runtimes for macOS, FreeBSD and Windows."""
import argparse
import gzip
import io
import os
from pathlib import Path
import platform
import shutil
import subprocess
import tarfile
import tempfile
import time

from artifact_manifest import publish

VERSION = '1.14.2'
COMMIT = 'af6e64c3b69e6132ebaee0e1a3d24e93903f6709'
CRONET = '0d28acc44093df24b2526dea3d6ffefd6b0a54f0'
TARGETS = ('macos-arm64', 'freebsd-amd64', 'freebsd-arm64', 'windows-amd64', 'windows-arm64')


def run(args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def checkout(directory, url, commit, sparse=None):
    run(['git', 'init', str(directory)])
    run(['git', '-C', str(directory), 'remote', 'add', 'origin', url])
    if sparse:
        run(['git', '-C', str(directory), 'sparse-checkout', 'set', '--no-cone', '/' + sparse])
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
        tags_file = 'DEFAULT_BUILD_TAGS' if system == 'macos' else 'DEFAULT_BUILD_TAGS_WINDOWS' if system == 'windows' else 'DEFAULT_BUILD_TAGS_OTHERS'
        tags = (source / 'release' / tags_file).read_text().strip() + ',with_v2ray_api'
        ldflags = (source / 'release/LDFLAGS').read_text().strip()
        binary = root / ('sing-box.exe' if system == 'windows' else 'sing-box')
        env = dict(os.environ, GOOS='darwin' if system == 'macos' else system, GOARCH=arch,
                   CGO_ENABLED='1' if system == 'macos' else '0', GOTOOLCHAIN='local')
        for attempt in range(3):
            try:
                run(['go', 'build', '-trimpath', '-o', str(binary), '-tags', tags,
                     '-ldflags', f'-X github.com/sagernet/sing-box/constant.Version={VERSION} {ldflags} -s -w -buildid=',
                     './cmd/sing-box'], cwd=source, env=env)
                break
            except subprocess.CalledProcessError:
                if attempt == 2:
                    raise
                print('Retrying runtime build with the existing module cache', flush=True)
                time.sleep(5)
        files = [binary]
        if system == 'windows':
            if (source / '.github/CRONET_GO_VERSION').read_text().strip() != CRONET:
                raise ValueError('upstream changed its pinned runtime library')
            library = f'lib/windows_{arch}/libcronet.dll'
            checkout(root / 'cronet', 'https://github.com/SagerNet/cronet-go.git', CRONET, library)
            shutil.copyfile(root / 'cronet' / library, root / 'libcronet.dll')
            files.append(root / 'libcronet.dll')
        if system != 'freebsd' or platform.system() == 'FreeBSD':
            text = run([str(binary), 'version'], capture_output=True, text=True).stdout
            if f'sing-box version {VERSION}' not in text or 'with_v2ray_api' not in text:
                raise ValueError('runtime version or traffic accounting support is missing')
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
