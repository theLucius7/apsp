#!/usr/bin/env python3
"""Apply the independent IPQuality tool profile to the authenticated Debian factory.

The existing source authentication, native builder approval, exact package/source
closure, capacity guards and failure evidence are retained. Profile selection does
not approve a builder, grant a license or run a diagnostic.
"""
import argparse
import importlib.util
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
TOOLS = {
    'bash': 'bash', 'python3': 'python3', 'curl': 'curl', 'jq': 'jq',
    'base64': 'coreutils', 'head': 'coreutils', 'wc': 'coreutils',
    'date': 'coreutils', 'timeout': 'coreutils', 'numfmt': 'coreutils',
    'uname': 'coreutils', 'cat': 'coreutils', 'cut': 'coreutils',
    'sort': 'coreutils', 'tr': 'coreutils', 'printf': 'coreutils',
    'grep': 'grep', 'sed': 'sed', 'awk': 'gawk', 'bc': 'bc',
    'openssl': 'openssl', 'update-ca-certificates': 'ca-certificates',
}
KIND = 'apsp-ipquality-debian12-preparation'


def module(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


def factory():
    build = module('apsp_ipquality_debian_factory', ROOT / 'tools/nodequality-rootfs-build.py')
    build.TOOL_PACKAGES = dict(TOOLS)
    build.PROVENANCE_KIND = KIND
    build.PENDING = ['IPQuality: independent source policy, license review and node acceptance are separate']
    profile = module('apsp_ipquality_minimal_factory_profile', ROOT / 'tools/ipquality-profile.py')
    build.INPUT_PROFILE = profile.Profile(build, TOOLS)
    return build


def collector():
    collect = module('apsp_ipquality_debian_collector', ROOT / 'tools/nodequality-rootfs-collect.py')
    collect.BUILD = factory()
    collect.COLLECTION_KIND = 'apsp-ipquality-debian-input-collection'
    return collect


def main():
    if len(sys.argv) < 2 or sys.argv[1] not in ('collect', 'derive', 'bind', 'prepare', 'build', 'export'):
        raise SystemExit('Usage: ipquality-rootfs.py collect|derive|bind|prepare|build|export <explicit factory arguments>')
    chosen = sys.argv[1]
    sys.argv = [sys.argv[0], *sys.argv[1:]]
    if chosen == 'derive' or (chosen == 'bind' and any(
            value == '--derived-inputs' or value.startswith('--derived-inputs=') for value in sys.argv[2:])):
        return module('apsp_ipquality_input_derivation', ROOT / 'tools/ipquality-inputs.py').main()
    if chosen == 'prepare':
        build = factory()
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('operation', choices=('prepare',))
        for name in ('lock', 'cache', 'derived-inputs', 'derived-binding', 'output'):
            parser.add_argument('--' + name, type=Path, required=True)
        parser.add_argument('--approved-builder-image-sha256', required=True)
        parser.add_argument('--max-output-bytes', type=int, default=build.DEFAULT_MAX_OUTPUT)
        parser.add_argument('--reserve-free-bytes', type=int, default=build.DEFAULT_RESERVE_FREE)
        parser.add_argument('--reserve-free-inodes', type=int, default=build.DEFAULT_RESERVE_INODES)
        args = parser.parse_args()
        build.INPUT_PROFILE.prepare_context = {'derived_inputs': args.derived_inputs,
                                               'derived_binding': args.derived_binding}
        with build.cli_signals():
            result = build.prepare(args.lock, args.cache, args.output, args.approved_builder_image_sha256,
                                   args.max_output_bytes, args.reserve_free_bytes, args.reserve_free_inodes)
        print(build.canonical(result).decode('ascii'))
        return 0
    return (collector() if chosen in ('collect', 'bind') else factory()).main()


if __name__ == '__main__':
    raise SystemExit(main())
