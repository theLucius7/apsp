#!/usr/bin/env python3
"""Owned bytes exercise signed expansion and solver capacity contracts.

Only the gpgv machine interface and APT producer are mocked. These fixtures do
not certify a real Debian signature, actual APT peak, builder, or usable image.
"""

import copy
import gzip
import hashlib
import importlib.util
import lzma
import os
from pathlib import Path
import tempfile
import types
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]


def load(name, filename):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'tools' / filename)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


CAPACITY = load('ipquality_input_capacity_tests', 'ipquality-inputs-capacity.py')
COLLECT = load('ipquality_collect_capacity_tests', 'nodequality-rootfs-collect.py')
BUILD = CAPACITY.BUILD


def identity(content):
    return {'size': len(content), 'sha256': hashlib.sha256(content).hexdigest()}


def signed_status(archive):
    signer = sorted(BUILD.SIGNERS[archive])[0]
    return ('[GNUPG:] VALIDSIG ' + signer + ' 2026-09-30 1790726400 0 4 0 1 8 01 ' + signer + '\n').encode()


def fixture(root, extension='.gz'):
    """Schema-valid parent metadata; signatures remain owned mock evidence."""
    cache = root / 'input-cache'
    cache.mkdir(mode=0o700)

    def descriptor(content, suffix):
        value = identity(content)
        value['blob'] = value['sha256'] + suffix
        path = cache / value['blob']
        if not path.exists():
            path.write_bytes(content)
        return value

    result = {'schema': 1, 'arch': 'arm64', 'source_epoch': 1790864840,
              'keyring': descriptor(b'owned keyring', '.gpg'), 'repositories': [],
              'packages': [], 'sources': []}
    releases, bounds = {}, {}
    for repo_id, archive, suite, timestamp in (
            ('main', 'debian', 'bookworm', '20261001T142720Z'),
            ('security', 'debian-security', 'bookworm-security', '20261001T142623Z')):
        raw = ('Package: owned-' + repo_id + '\nVersion: 1\nArchitecture: arm64\n\n').encode()
        compressed = gzip.compress(raw, mtime=0) if extension == '.gz' else lzma.compress(raw, format=lzma.FORMAT_XZ)
        index_path = 'main/binary-arm64/Packages' + extension
        row = dict(descriptor(compressed, extension), kind='Packages', path=index_path)
        sources = dict(descriptor(gzip.compress(b'Package: fixture\nVersion: 1\n\n', mtime=0), '.gz'),
                       kind='Sources', path='main/source/Sources.gz')
        release = ('Origin: Debian\nCodename: ' + suite + '\nSHA256:\n ' + row['sha256']
                   + ' ' + str(row['size']) + ' ' + index_path + '\n '
                   + identity(raw)['sha256'] + ' ' + str(len(raw)) + ' ' + index_path.rsplit('.', 1)[0]
                   + '\n ' + sources['sha256'] + ' ' + str(sources['size']) + ' ' + sources['path'] + '\n').encode()
        inrelease = descriptor(b'owned signature envelope ' + repo_id.encode(), '.InRelease')
        result['repositories'].append({'id': repo_id, 'archive': archive, 'suite': suite,
            'timestamp': timestamp, 'inrelease': inrelease, 'indices': [row, sources]})
        releases[inrelease['blob']] = (release, archive)
        bounds[repo_id] = {index_path: dict(identity(raw), uncompressed_path=index_path.rsplit('.', 1)[0],
            compressed_size=row['size'], compressed_sha256=row['sha256'])}
    binary = descriptor(b'owned binary fixture', '.deb')
    for name in sorted(set(BUILD.TOOL_PACKAGES.values())):
        result['packages'].append(dict(binary, repository='main', name=name, version='1', architecture='arm64',
            filename='pool/main/f/fixture/' + name + '_1_arm64.deb', source_name='fixture', source_version='1'))
    result['sources'] = [{'repository': 'main', 'name': 'fixture', 'version': '1',
                         'directory': 'pool/main/f/fixture',
                         'files': [dict(descriptor(b'owned dsc', '.source'), name='fixture.dsc'),
                                   dict(descriptor(b'owned source archive', '.source'), name='fixture.orig.tar.xz')]}]
    return result, cache, releases, bounds


class SignedExpansionContracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='apsp-ip-input-capacity-')
        self.root = Path(self.temporary.name).resolve()

    def tearDown(self):
        self.temporary.cleanup()

    def test_complete_streaming_gzip_and_xz_check_both_identities_without_output(self):
        for suffix in ('.gz', '.xz'):
            raw = b'Package: owned\nDescription: ' + b'a' * (5 * CAPACITY.CHUNK) + b'\n\n'
            encoded = gzip.compress(raw, mtime=0) if suffix == '.gz' else lzma.compress(raw, format=lzma.FORMAT_XZ)
            path = self.root / ('index' + suffix)
            path.write_bytes(encoded)
            before = sorted(self.root.iterdir())
            with self.subTest(suffix=suffix):
                actual = CAPACITY.stream_expansion(path, identity(encoded), identity(raw), BUILD.Deadline(10))
                self.assertEqual(actual, identity(raw))
                self.assertEqual(path.read_bytes(), encoded)
                self.assertEqual(sorted(self.root.iterdir()), before)

    def test_encoded_and_decoded_mismatch_cannot_be_authenticated(self):
        raw = b'owned Packages bytes\n'
        encoded = gzip.compress(raw, mtime=0)
        path = self.root / 'index.gz'
        path.write_bytes(encoded)
        for encoded_identity, decoded_identity in (
                (dict(identity(encoded), sha256='f' * 64), identity(raw)),
                (identity(encoded), dict(identity(raw), sha256='e' * 64)),
                (identity(encoded), dict(identity(raw), size=len(raw) - 1)),
                (identity(encoded), dict(identity(raw), size=len(raw) + 1)),
                (dict(identity(encoded), size=len(encoded) - 1), identity(raw))):
            with self.subTest(encoded=encoded_identity, decoded=decoded_identity), self.assertRaises(ValueError):
                CAPACITY.stream_expansion(path, encoded_identity, decoded_identity, BUILD.Deadline(10))

    def test_truncated_trailing_concatenated_wrong_type_and_corrupt_trailer_are_rejected(self):
        raw = b'owned index\n' * 10000
        for suffix in ('.gz', '.xz'):
            encoded = gzip.compress(raw, mtime=0) if suffix == '.gz' else lzma.compress(raw, format=lzma.FORMAT_XZ)
            other = lzma.compress(raw, format=lzma.FORMAT_XZ) if suffix == '.gz' else gzip.compress(raw, mtime=0)
            for label, malformed in (('truncated', encoded[:-1]), ('garbage', encoded + b'x'),
                                     ('padding', encoded + b'\0' * 4), ('concatenated', encoded + encoded),
                                     ('wrong_type', other), ('corrupt', encoded[:-4] + b'xxxx')):
                path = self.root / ('invalid-' + label + suffix)
                path.write_bytes(malformed)
                with self.subTest(suffix=suffix, label=label), self.assertRaises(ValueError):
                    CAPACITY.stream_expansion(path, identity(malformed), identity(raw), BUILD.Deadline(10))

    def test_non_integer_zero_and_oversized_signed_bounds_fail_before_open(self):
        valid = identity(b'owned')
        for invalid in (True, 0, -1, 1.5, '4', BUILD.MAX_INDEX + 1):
            for target in ('encoded', 'decoded'):
                compressed = dict(valid, size=invalid) if target == 'encoded' else valid
                expanded = dict(valid, size=invalid) if target == 'decoded' else valid
                with self.subTest(invalid=invalid, target=target), self.assertRaises(ValueError), \
                        mock.patch.object(BUILD, 'open_regular', side_effect=AssertionError('must reject before opening')):
                    CAPACITY.stream_expansion(self.root / 'absent.gz', compressed, expanded, BUILD.Deadline(10))

    def test_deadline_protection_is_applied_during_streaming(self):
        raw = b'owned' * 10000
        encoded = gzip.compress(raw, mtime=0)
        path = self.root / 'deadline.gz'
        path.write_bytes(encoded)
        deadline = types.SimpleNamespace(check=mock.Mock(side_effect=ValueError('owned deadline exceeded')))
        with self.assertRaisesRegex(ValueError, 'deadline'):
            CAPACITY.stream_expansion(path, identity(encoded), identity(raw), deadline)
        self.assertEqual(path.read_bytes(), encoded)

    def test_actual_gpgv_cleartext_supplies_both_signed_identities(self):
        materials, cache, releases, expected = fixture(self.root, '.xz')
        guard = types.SimpleNamespace(output=self.root, check=mock.Mock())
        deadline = BUILD.Deadline(10, capacity=guard)
        commands = []

        def gpgv(argv, child_deadline, bound, **kwargs):
            commands.append(argv)
            self.assertEqual(argv[0], '/usr/bin/gpgv')
            self.assertIs(child_deadline.capacity, guard)
            self.assertEqual(bound, CAPACITY.CHUNK)
            cleartext, archive = releases[Path(argv[-1]).name]
            Path(argv[argv.index('--output') + 1]).write_bytes(cleartext)
            return signed_status(archive)

        before = {path: path.read_bytes() for path in cache.iterdir()}
        with mock.patch.object(BUILD, 'run_bounded', side_effect=gpgv):
            actual = CAPACITY.authenticated_expansion(materials, cache, deadline)
        self.assertEqual(actual, expected)
        self.assertEqual(len(commands), 2)
        self.assertEqual({path: path.read_bytes() for path in cache.iterdir()}, before)
        self.assertEqual(sorted(path.name for path in self.root.iterdir()), ['input-cache'])

    def test_missing_unsigned_or_bad_signature_bounds_never_fall_back_to_observed_size(self):
        materials, cache, releases, expected = fixture(self.root)

        def gpgv(argv, child_deadline, bound, **kwargs):
            cleartext, archive = releases[Path(argv[-1]).name]
            Path(argv[argv.index('--output') + 1]).write_bytes(cleartext)
            return signed_status(archive)

        original = copy.deepcopy(releases)
        for mutation in ('missing_raw', 'wrong_raw_hash', 'wrong_encoded_hash', 'bad_origin', 'duplicate_raw'):
            releases.clear()
            releases.update(copy.deepcopy(original))
            repo = materials['repositories'][0]
            key = repo['inrelease']['blob']
            text, archive = releases[key]
            raw = expected[repo['id']][repo['indices'][0]['path']]
            raw_line = (' ' + raw['sha256'] + ' ' + str(raw['size']) + ' ' + raw['uncompressed_path'] + '\n').encode()
            if mutation == 'missing_raw':
                text = text.replace(raw_line, b'')
            elif mutation == 'wrong_raw_hash':
                text = text.replace(raw['sha256'].encode(), b'f' * 64)
            elif mutation == 'wrong_encoded_hash':
                text = text.replace(raw['compressed_sha256'].encode(), b'e' * 64)
            elif mutation == 'bad_origin':
                text = text.replace(b'Origin: Debian', b'Origin: Owned')
            else:
                text += raw_line
            releases[key] = (text, archive)
            with self.subTest(mutation=mutation), mock.patch.object(BUILD, 'run_bounded', side_effect=gpgv), \
                    self.assertRaises(ValueError):
                CAPACITY.authenticated_expansion(materials, cache, BUILD.Deadline(10))
        with mock.patch.object(BUILD, 'run_bounded', return_value=b'[GNUPG:] BADSIG owned\n'), \
                self.assertRaisesRegex(ValueError, 'signature'), \
                mock.patch.object(CAPACITY, 'stream_expansion', side_effect=AssertionError('signature must fail first')):
            CAPACITY.authenticated_expansion(materials, cache, BUILD.Deadline(10))

    def test_capacity_uses_multiple_signed_copies_and_never_claims_hard_quota(self):
        materials, _, _, bounds = fixture(self.root)
        plan = CAPACITY.expansion_budget(materials['repositories'], bounds)
        decoded = sum(row['size'] for paths in bounds.values() for row in paths.values())
        encoded = sum(row['compressed_size'] for paths in bounds.values() for row in paths.values())
        self.assertEqual(plan['reserved_expansion_bytes'],
                         decoded * 3 + encoded * 3 + CAPACITY.SOLVER_METADATA_BYTES)
        self.assertEqual(plan['expanded_index_bytes'], decoded)
        self.assertEqual(plan['compressed_index_bytes'], encoded)
        self.assertIs(plan['hard_quota'], False)
        self.assertNotIn('continuous_peak', plan)

    def test_internal_bounds_require_exact_repositories_paths_identities_and_types(self):
        materials, _, _, original = fixture(self.root)
        for mutation in ('missing_repo', 'extra_repo', 'extra_path', 'wrong_path', 'bool_size',
                         'oversized', 'wrong_hash', 'wrong_compressed', 'extra_field'):
            altered = copy.deepcopy(original)
            path, row = next(iter(altered['main'].items()))
            if mutation == 'missing_repo':
                del altered['security']
            elif mutation == 'extra_repo':
                altered['other'] = {}
            elif mutation == 'extra_path':
                altered['main']['main/binary-arm64/Other.gz'] = row
            elif mutation == 'wrong_path':
                row['uncompressed_path'] = 'main/binary-amd64/Packages'
            elif mutation == 'bool_size':
                row['size'] = True
            elif mutation == 'oversized':
                row['size'] = BUILD.MAX_INDEX + 1
            elif mutation == 'wrong_hash':
                row['sha256'] = 'invalid'
            elif mutation == 'wrong_compressed':
                row['compressed_sha256'] = 'f' * 64
            else:
                row['observed_peak'] = 61 * 1024**2
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                CAPACITY.expansion_budget(materials['repositories'], altered)

    def test_solver_default_reserve_stays_legacy_and_small_disk_refuses_before_apt(self):
        materials, cache, _, bounds = fixture(self.root)
        repositories = materials['repositories']
        expected = CAPACITY.expansion_budget(repositories, bounds)['reserved_expansion_bytes']
        for supplied, first_reserve in ((None, 2 * COLLECT.BUILD.MAX_INDEX), (bounds, expected)):
            output = self.root / ('solver-output-' + ('legacy' if supplied is None else 'signed'))
            output.mkdir(mode=0o700)
            observations = []

            def refuse(additional=0):
                observations.append(additional)
                raise ValueError('owned free-disk reserve would be crossed')

            collector = types.SimpleNamespace(output=output, cache=cache, arch='arm64',
                deadline=COLLECT.BUILD.Deadline(10), budget=refuse)
            with self.subTest(supplied=supplied is not None), \
                    mock.patch.object(COLLECT, 'essential_seeds', return_value=(['bash'], [])), \
                    mock.patch.object(COLLECT.BUILD, 'run_bounded', side_effect=AssertionError('APT cannot start')), \
                    self.assertRaisesRegex(ValueError, 'reserve'):
                COLLECT.solve(collector, repositories, {}, cache / materials['keyring']['blob'],
                              authenticated_expansion=supplied)
            self.assertEqual(observations, [first_reserve])

    def test_solver_passes_dynamic_guard_to_both_isolated_producers_and_keeps_borrowed_cache(self):
        materials, cache, _, bounds = fixture(self.root)
        output = self.root / 'derived'
        output.mkdir(mode=0o700)
        guard = types.SimpleNamespace(check=mock.Mock())
        collector = types.SimpleNamespace(output=output, cache=cache, arch='arm64',
            deadline=COLLECT.BUILD.Deadline(10, capacity=guard), budget=mock.Mock())
        before = {path: (path.stat().st_ino, path.read_bytes()) for path in cache.iterdir()}
        seen = []
        actual_stat = os.stat

        def namespace_stat(path, *args, **kwargs):
            if str(path) == '/proc/self/ns/net':
                return types.SimpleNamespace(st_ino=42)
            return actual_stat(path, *args, **kwargs)

        def producer(argv, deadline, limit):
            self.assertIs(deadline.capacity, guard)
            operation = argv[argv.index('--operation') + 1]
            evidence = Path(argv[argv.index('--evidence') + 1])
            evidence.write_bytes(COLLECT.BUILD.canonical({'operation': operation, 'owned_mock': True}))
            seen.append(operation)
            return b'owned producer bytes\n'

        package = dict(materials['packages'][0])
        package['name'] = 'bash'
        package.pop('blob')
        with mock.patch.object(COLLECT, 'essential_seeds', return_value=(['bash'], [])), \
                mock.patch.object(COLLECT.os, 'stat', side_effect=namespace_stat), \
                mock.patch.object(COLLECT.BUILD, 'run_bounded', side_effect=producer), \
                mock.patch.object(COLLECT, 'map_print_uris', return_value=[package]):
            _, report = COLLECT.solve(collector, materials['repositories'], {},
                                     cache / materials['keyring']['blob'], authenticated_expansion=bounds)
        self.assertEqual(seen, ['update', 'plan'])
        self.assertIs(report['authenticated_expansion_admission']['hard_quota'], False)
        self.assertEqual({path: (path.stat().st_ino, path.read_bytes()) for path in cache.iterdir()}, before)
        for name in ('lists', 'archives', 'mirrors'):
            self.assertFalse((output / 'solver' / name).exists())
        self.assertTrue((output / 'solver/selection.json').is_file())

    def test_tool_ownership_and_all_version_children_inherit_dynamic_capacity_guard(self):
        guard = types.SimpleNamespace(check=mock.Mock())
        deadline = COLLECT.BUILD.Deadline(10, capacity=guard)
        children = []

        def bounded(argv, child_deadline, limit):
            self.assertIs(child_deadline.capacity, guard)
            self.assertEqual(limit, 65536)
            children.append(argv)
            return b'owned interface fixture, not an actual tool certification\n'

        # Resolution and byte reads are inert producer-interface fixtures here;
        # the assertion covers every real child Deadline created by the helper.
        with mock.patch.object(Path, 'resolve', lambda path, **kwargs: path.absolute()), \
                mock.patch.object(COLLECT.BUILD, 'file_identity', return_value={'size': 1, 'sha256': 'a' * 64}), \
                mock.patch.object(COLLECT.BUILD, 'run_bounded', side_effect=bounded):
            report = COLLECT.record_collection_tools(deadline)
        ownership = [argv for argv in children if argv[:2] == ['/usr/bin/dpkg-query', '--search']]
        versions = [argv for argv in children if argv[-1] == '--version']
        self.assertEqual(len(ownership), len(report['executables']))
        self.assertEqual(len(versions), 4)
        self.assertTrue(any(argv[:2] == ['/usr/bin/dpkg-query', '--show'] for argv in children))
        self.assertTrue(guard.check.called)


class MemoryObservationContracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='apsp-ip-memory-observation-')
        self.root = Path(self.temporary.name).resolve()
        self.meminfo = self.root / 'meminfo'
        self.meminfo.write_text('MemTotal: 1048576 kB\nMemAvailable: 524288 kB\nDirectMap4k: 256 kB\n')
        self.membership = self.root / 'cgroup'
        self.membership.write_text('0::/owned\n')
        self.cgroups = self.root / 'cgroups'
        self.cgroups.mkdir()
        self.owned = self.cgroups / 'owned'
        self.owned.mkdir()
        self.memory_files(self.cgroups, 'max', 100000000, 0, 0)
        self.memory_files(self.owned, 256 * 1024**2, 256 * 1024**2, 200 * 1024**2, 0)

    def tearDown(self):
        self.temporary.cleanup()

    @staticmethod
    def memory_files(folder, limit, current, inactive, slab):
        (folder / 'memory.max').write_text(str(limit) + '\n')
        (folder / 'memory.current').write_text(str(current) + '\n')
        (folder / 'memory.stat').write_text('inactive_file ' + str(inactive) + '\nslab_reclaimable ' + str(slab) + '\n')
        (folder / 'memory.events').write_text('low 0\nhigh 0\nmax 0\noom 0\noom_kill 0\n')

    def observe(self):
        return CAPACITY.available_memory(self.meminfo, self.cgroups, self.membership)

    def test_host_available_is_distinct_from_cache_charged_raw_headroom(self):
        value = self.observe()
        self.assertEqual(value['host_available_bytes'], 512 * 1024**2)
        self.assertEqual(value['available_bytes'], 512 * 1024**2)
        self.assertEqual(value['cgroup_raw_headroom_bytes'], 0)
        self.assertEqual(value['cgroup_observations'][0]['inactive_file_bytes'], 200 * 1024**2)
        self.assertEqual(value['cgroup_observations'][0]['memory_events']['oom_kill'], 0)
        self.assertIs(value['reclaim_estimate_is_guaranteed'], False)

    def test_all_visible_finite_ancestor_limits_are_observed(self):
        self.memory_files(self.cgroups, 128 * 1024**2, 120 * 1024**2, 0, 0)
        self.memory_files(self.owned, 256 * 1024**2, 10 * 1024**2, 0, 0)
        value = self.observe()
        self.assertEqual(value['cgroup_raw_headroom_bytes'], 8 * 1024**2)
        self.assertEqual(len(value['cgroup_observations']), 2)

    def test_oom_and_max_are_distinct_integer_observations_and_missing_events_are_unknown(self):
        (self.owned / 'memory.events').write_text('max 25\noom 2\noom_kill 1\n')
        events = self.observe()['cgroup_observations'][0]['memory_events']
        self.assertEqual(events, {'max': 25, 'oom': 2, 'oom_kill': 1})
        for malformed in ('max 25\noom 2\n', 'max -1\noom 2\noom_kill 1\n',
                          'max 0\noom 0\noom_kill 0\noom 1\n'):
            (self.owned / 'memory.events').write_text(malformed)
            with self.subTest(malformed=malformed), self.assertRaises(ValueError):
                self.observe()

    def test_missing_host_invalid_limit_or_unsafe_membership_cannot_claim_available(self):
        for mutation in ('missing_host', 'bad_limit', 'unsafe_membership', 'duplicate_membership'):
            original = {path: path.read_bytes() for path in (self.meminfo, self.membership, self.owned / 'memory.max')}
            if mutation == 'missing_host':
                self.meminfo.write_text('MemTotal: 123 kB\n')
            elif mutation == 'bad_limit':
                (self.owned / 'memory.max').write_text('-1\n')
            elif mutation == 'unsafe_membership':
                self.membership.write_text('0::/owned/../owned\n')
            else:
                self.membership.write_text('0::/owned\n0::/\n')
            with self.subTest(mutation=mutation), self.assertRaises(ValueError):
                self.observe()
            for path, content in original.items():
                path.write_bytes(content)


if __name__ == '__main__':
    unittest.main()
