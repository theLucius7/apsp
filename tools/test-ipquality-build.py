#!/usr/bin/env python3
"""Inert fixtures for packaging capacity, complete sources and group rollback.

These fixtures never authenticate a Debian build or certify a live diagnostic.
"""
import io
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import ipquality_artifact as artifact
import release

build = artifact.module('apsp_ipquality_builder_tests', artifact.ROOT / 'tools/build-ipquality.py')


class FixtureGuard:
    plan = {'block_size': 4096}

    def __init__(self):
        self.chunks = 0

    def check(self, *args, **kwargs):
        return {}

    def progress(self):
        self.chunks += 1


class BuildTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='apsp-ipquality-build-fixture-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.guard = FixtureGuard()
        self.factory = artifact.factory()
        self.memory = patch.object(build, 'memory_available', return_value=None)
        self.memory.start()
        self.addCleanup(self.memory.stop)

    def args(self, **values):
        return SimpleNamespace(max_output_bytes=4 * 1024 * build.BLOCK,
                               reserve_free_bytes=build.DISK_RESERVE,
                               reserve_free_inodes=build.INODE_RESERVE, **values)

    def lock(self):
        return {'sources': [{'files': [{'blob': 'source/sample.tar.xz', 'size': 4096,
                                        'sha256': 'a' * 64}]}]}

    def observation(self, **changes):
        values = {'f_frsize': 4096, 'f_bavail': 4 * 1024 * 1024, 'f_favail': 100000}
        values.update(changes)
        return SimpleNamespace(**values)

    @unittest.skipUnless(hasattr(os, "mkfifo") and hasattr(os, "O_NONBLOCK"),
                         "requires ordinary-file/FIFO replacement semantics")
    def test_source_offer_asset_replaced_with_fifo_is_refused_without_blocking(self):
        source = self.root / "TEST_ONLY-source-asset"
        source.write_bytes(b"TEST_ONLY ordinary source bytes")
        original_open = os.open
        def replaced(path, flags, *args, **kwargs):
            if Path(path) == source:
                source.unlink()
                os.mkfifo(source, 0o600)
            return original_open(path, flags, *args, **kwargs)
        with patch.object(release.os, "open", side_effect=replaced):
            with self.assertRaisesRegex(ValueError, "asset changed before reading"):
                release.regular_file_proof(source)

    def test_capacity_admission_accounts_for_all_copies_and_does_not_approve_inputs(self):
        with patch.object(build.os, 'statvfs', return_value=self.observation()):
            plan = build.capacity_plan(self.factory, self.root, self.lock(), 8 * build.BLOCK, self.args())
        self.assertTrue(plan['admitted'])
        self.assertEqual(plan['copies']['augmented_rootfs'], artifact.MAX_ARCHIVE)
        self.assertEqual(plan['copies']['validation_intake_rootfs'], artifact.MAX_ARCHIVE)
        self.assertEqual(plan['copies']['outer_archive'], artifact.MAX_ARCHIVE)
        self.assertGreater(plan['copies']['paired_source_archive'], artifact.MAX_SOURCE)
        self.assertGreaterEqual(plan['reserve_free_bytes'], build.DISK_RESERVE)
        self.assertGreaterEqual(plan['reserve_free_inodes'], build.INODE_RESERVE)
        self.assertFalse(plan['source_authenticated'])
        self.assertFalse(plan['builder_approved'])
        self.assertFalse(plan['full_ready'])

    def test_low_disk_and_inodes_are_independent_admission_failures(self):
        for changes, reason in (({'f_bavail': 100}, 'free_disk'), ({'f_favail': 100}, 'free_inodes')):
            with self.subTest(reason=reason), patch.object(build.os, 'statvfs', return_value=self.observation(**changes)):
                plan = build.capacity_plan(self.factory, self.root, self.lock(), 8 * build.BLOCK, self.args())
            self.assertFalse(plan['admitted'])
            self.assertIn(reason, plan['reasons'])

    def test_output_budget_and_linux_memory_reserve_reject_before_building(self):
        args = self.args()
        args.max_output_bytes = artifact.MAX_ARCHIVE
        with patch.object(build.os, 'statvfs', return_value=self.observation()), \
             patch.object(build, 'memory_available', return_value=build.MEMORY_RESERVE + build.BLOCK):
            plan = build.capacity_plan(self.factory, self.root, self.lock(), 8 * build.BLOCK, args)
        self.assertEqual(set(plan['reasons']), {'output_byte_budget', 'available_memory'})

    def test_dynamic_memory_floor_and_archive_write_budget_stop_progress(self):
        with patch.object(build, 'memory_available', return_value=build.MEMORY_RESERVE - 1):
            with self.assertRaisesRegex(ValueError, 'available memory'):
                build.memory_check()
        output = io.BytesIO()
        writer = build.Writer(output, 4, self.guard)
        writer.write(b'1234')
        with self.assertRaisesRegex(ValueError, 'archive byte budget'):
            writer.write(b'5')
        self.assertEqual(output.getvalue(), b'1234')

    def test_read_handles_archives_larger_than_metadata_reader_limit(self):
        value = b'x' * (9 * build.BLOCK)
        path = self.root / 'rootfs.tar.gz'
        path.write_bytes(value)
        self.assertEqual(build.read(path, artifact.MAX_ARCHIVE, self.guard), value)
        self.assertGreaterEqual(self.guard.chunks, 9)

    def test_corresponding_source_includes_exact_minimal_profile_implementation(self):
        # Only upstream bundle conversion is replaced at its separate boundary;
        # the packaging list and local Apsp source bytes are the real inputs.
        bundle, review = self.root / 'bundle.json', self.root / 'license-review.json'
        bundle.write_bytes(b'TEST_ONLY upstream bundle boundary')
        review.write_bytes(b'{"scope":"TEST_ONLY no license approval"}')
        helper = SimpleNamespace(decode_bundle=lambda content: content,
            bundle_files=lambda bundle: {'LICENSE.ip': b'TEST_ONLY upstream license'},
            transform_files=lambda original: {'scope': b'TEST_ONLY no native build'},
            policy_bytes=lambda: {})
        actual_module = artifact.module
        def source_module(name, path):
            # Keep runtime()._ordinary backed by the real rootfs verifier;
            # only the declared upstream conversion boundary is substituted.
            return helper if name == 'apsp_node_ip_source' else actual_module(name, path)
        with patch.object(artifact, 'module', side_effect=source_module):
            sources, _ = build.source_files(bundle, review, self.guard)
        for name in ('tools/ipquality-profile.py', 'tools/ipquality-inputs.py',
                     'tools/ipquality-inputs-capacity.py', 'tools/ipquality-rootfs.py',
                     'tools/nodequality-rootfs-build.py', 'tools/nodequality-rootfs-collect.py'):
            self.assertEqual(sources[name], (artifact.ROOT / name).read_bytes())
        self.assertFalse(any('ipquality-profile-private.json' in name or 'input-ledger.json' in name
                             for name in sources))

    def paired_fixture(self, body=b'TEST_ONLY corresponding source bytes'):
        cache = self.root / 'input-cache'
        (cache / 'source').mkdir(parents=True, mode=0o700)
        (cache / 'source/sample.tar.xz').write_bytes(body)
        mini = b'TEST_ONLY Apsp source bytes'
        expected = {'apsp-source.tar.gz': {'size': len(mini), 'sha256': artifact.digest(mini)},
                    'debian-sources/source/sample.tar.xz': {'size': len(body), 'sha256': artifact.digest(body)}}
        return cache, mini, expected

    def test_paired_source_stream_preserves_actual_bytes_and_complete_exact_inventory(self):
        body = b'abc' * build.BLOCK
        cache, mini, expected = self.paired_fixture(body)
        output = self.root / 'amd64.sources.tar.gz'
        result = build.pack_sources(output, mini, expected, cache, self.factory, self.guard)
        files = {'THIRD_PARTY_NOTICES.txt': b'Apsp IPQuality node self-query\n' + artifact.canonical({
            'source_offer': {'asset': f'ipquality-{artifact.VERSION}-linux-amd64-sources.tar.gz', **result},
            'license': 'AGPL-3.0-only', 'notice': 'TEST_ONLY inert source fixtures; no build approval'})}
        with patch.object(artifact, 'source_inventory', return_value=expected):
            artifact.validate_source_offer(output, files, artifact.VERSION, 'amd64')
        self.assertGreaterEqual(self.guard.chunks, 3)
        self.assertEqual(artifact.unpack(output.read_bytes(), maximum=artifact.MAX_SOURCE_OFFER),
                         {'apsp-source.tar.gz': mini, 'debian-sources/source/sample.tar.xz': body})

    def test_missing_and_changed_corresponding_source_bytes_abort_packaging(self):
        cache, mini, expected = self.paired_fixture()
        blob = cache / 'source/sample.tar.xz'
        original = blob.read_bytes()
        blob.write_bytes(b'X' * len(original))
        with self.assertRaisesRegex(ValueError, 'source bytes differ'):
            build.pack_sources(self.root / 'changed.tar.gz', mini, expected, cache, self.factory, self.guard)
        blob.unlink()
        with self.assertRaises(FileNotFoundError):
            build.pack_sources(self.root / 'missing.tar.gz', mini, expected, cache, self.factory, self.guard)

    def test_pair_inventory_refuses_ambiguous_blob_or_unbounded_payload(self):
        lock = {'sources': [{'files': [{'blob': 'a', 'size': 1, 'sha256': 'a' * 64},
                                       {'blob': 'a', 'size': 2, 'sha256': 'b' * 64}]}]}
        with self.assertRaisesRegex(ValueError, 'ambiguous'):
            build.corresponding_sources(lock)
        lock = {'sources': [{'files': [{'blob': 'a', 'size': artifact.MAX_SOURCE_OFFER, 'sha256': 'a' * 64}]}]}
        with self.assertRaisesRegex(ValueError, 'complete source payload'):
            build.capacity_plan(self.factory, self.root, lock, 1, self.args())

    def publication_fixture(self, existing=False):
        target, stage = self.root / 'release', self.root / 'staging'
        target.mkdir(mode=0o700)
        stage.mkdir(mode=0o700)
        if existing:
            (target / 'arm64').write_bytes(b'TEST_ONLY previous runtime')
            (target / 'arm64.sources.tar.gz').write_bytes(b'TEST_ONLY previous sources')
            sums = ''.join(artifact.digest((target / name).read_bytes()) + '  ' + name + '\n'
                           for name in ('arm64', 'arm64.sources.tar.gz')).encode()
            (target / 'SHA256SUMS').write_bytes(sums)
        outer, paired = stage / 'amd64', stage / 'amd64.sources.tar.gz'
        outer.write_bytes(b'TEST_ONLY new runtime')
        paired.write_bytes(b'TEST_ONLY new sources')
        hashes = {'outer': artifact.digest(outer.read_bytes()), 'paired': artifact.digest(paired.read_bytes())}
        return target, stage, outer, paired, hashes

    def test_main_publication_marker_is_last_and_existing_architecture_is_preserved(self):
        target, stage, outer, paired, hashes = self.publication_fixture(existing=True)
        before = {name: (target / name).read_bytes() for name in ('arm64', 'arm64.sources.tar.gz')}
        original_link, destinations = build.os.link, []
        def observed_link(source, destination, **kwargs):
            if destination == target / 'amd64':
                self.assertTrue((target / 'amd64.sources.tar.gz').is_file())
                self.assertIn(b'  amd64\n', (target / 'SHA256SUMS').read_bytes())
                self.assertEqual((target / 'amd64.sources.tar.gz').stat().st_nlink, 1)
            destinations.append(Path(destination).name)
            return original_link(source, destination, **kwargs)
        with patch.object(build.os, 'link', side_effect=observed_link):
            result = build.publish(target, 'amd64', outer, paired, hashes, stage, self.factory, self.guard)
        self.assertEqual(result, target / 'amd64')
        self.assertEqual(destinations, ['amd64.sources.tar.gz', 'amd64'])
        self.assertEqual({name: (target / name).read_bytes() for name in before}, before)
        self.assertEqual((target / 'amd64').stat().st_nlink, 1)

    def test_failure_at_each_publication_operation_rolls_back_only_new_material(self):
        for operation in ('paired_link', 'inventory_replace', 'main_link', 'final_fsync'):
            with self.subTest(operation=operation), tempfile.TemporaryDirectory(dir=self.root) as name:
                original_root = self.root
                self.root = Path(name).resolve()
                try:
                    target, stage, outer, paired, hashes = self.publication_fixture(existing=True)
                    before = {path.name: path.read_bytes() for path in target.iterdir()}
                    original_link, original_replace, original_fsync = build.os.link, build.os.replace, build.fsync_directory
                    synced = 0
                    def link(source, destination, **kwargs):
                        failed = operation == 'paired_link' and destination == target / 'amd64.sources.tar.gz'
                        failed |= operation == 'main_link' and destination == target / 'amd64'
                        if failed:
                            raise OSError('TEST_ONLY injected publication failure')
                        return original_link(source, destination, **kwargs)
                    def replace(source, destination):
                        if operation == 'inventory_replace' and source == stage / 'SHA256SUMS':
                            raise OSError('TEST_ONLY injected publication failure')
                        return original_replace(source, destination)
                    def fsync(path):
                        nonlocal synced
                        synced += 1
                        if operation == 'final_fsync' and synced == 2:
                            raise OSError('TEST_ONLY injected publication failure')
                        return original_fsync(path)
                    with patch.object(build.os, 'link', side_effect=link), \
                         patch.object(build.os, 'replace', side_effect=replace), \
                         patch.object(build, 'fsync_directory', side_effect=fsync):
                        with self.assertRaisesRegex(OSError, 'injected publication failure'):
                            build.publish(target, 'amd64', outer, paired, hashes, stage, self.factory, self.guard)
                    self.assertEqual({path.name: path.read_bytes() for path in target.iterdir()}, before)
                finally:
                    self.root = original_root

    def test_signal_exception_before_marker_restores_previous_inventory(self):
        target, stage, outer, paired, hashes = self.publication_fixture(existing=True)
        before = {path.name: path.read_bytes() for path in target.iterdir()}
        original_link = build.os.link
        def link(source, destination, **kwargs):
            if destination == target / 'amd64':
                raise KeyboardInterrupt('TEST_ONLY handled interrupt')
            return original_link(source, destination, **kwargs)
        with patch.object(build.os, 'link', side_effect=link):
            with self.assertRaises(KeyboardInterrupt):
                build.publish(target, 'amd64', outer, paired, hashes, stage, self.factory, self.guard)
        self.assertEqual({path.name: path.read_bytes() for path in target.iterdir()}, before)

    def test_existing_version_and_orphan_material_are_never_replaced_or_deleted(self):
        target, stage, outer, paired, hashes = self.publication_fixture()
        (target / 'amd64.sources.tar.gz').write_bytes(b'TEST_ONLY earlier abandoned source')
        with self.assertRaisesRegex(ValueError, 'no checksum inventory'):
            build.publish(target, 'amd64', outer, paired, hashes, stage, self.factory, self.guard)
        self.assertEqual((target / 'amd64.sources.tar.gz').read_bytes(), b'TEST_ONLY earlier abandoned source')
        (target / 'amd64').write_bytes(b'TEST_ONLY existing runtime')
        data = ''.join(artifact.digest((target / name).read_bytes()) + '  ' + name + '\n'
                       for name in ('amd64', 'amd64.sources.tar.gz')).encode()
        (target / 'SHA256SUMS').write_bytes(data)
        with self.assertRaisesRegex(ValueError, 'replacement is forbidden'):
            build.publish(target, 'amd64', outer, paired, hashes, stage, self.factory, self.guard)
        self.assertEqual((target / 'SHA256SUMS').read_bytes(), data)

    def test_output_parent_symlink_is_refused(self):
        (self.root / 'real').mkdir(mode=0o700)
        (self.root / 'linked').symlink_to(self.root / 'real', target_is_directory=True)
        with self.assertRaises(OSError):
            build.private_directory(self.root / 'linked' / 'nested')

    def test_owned_packaging_failure_cleans_temporary_and_lock_preserving_old_material(self):
        prepared, exported = self.root / 'prepared', self.root / 'exported'
        prepared.mkdir(mode=0o700)
        exported.mkdir(mode=0o700)
        (prepared / 'inputs-lock.json').write_bytes(artifact.canonical(self.lock()))
        (exported / 'rootfs.tar.gz').write_bytes(b'TEST_ONLY inert rootfs bytes')
        output = self.root / 'output'
        output.mkdir(mode=0o700)
        sentinel = output / 'old-material'
        sentinel.write_bytes(b'TEST_ONLY prior evidence')
        args = self.args()
        args.arch, args.deadline_seconds = 'amd64', 60
        args.prepared_directory, args.rootfs_directory = prepared, exported
        args.source_bundle, args.license_review = self.root / 'unused-bundle', self.root / 'unused-review'
        args.approved_builder_image_sha256, args.output = 'a' * 64, output
        real_identity = self.factory.FactoryCapacity.identity
        # This fixture exercises ownership cleanup, not factory admission/auth.
        def fixture_capacity(directory, plan, deadline):
            return SimpleNamespace(output=directory, plan=plan, identity=real_identity,
                                   output_identity=real_identity(directory),
                                   parent_identity=real_identity(directory.parent),
                                   check=lambda *args, **kwargs: {})
        fixture_capacity.identity = real_identity
        plan = {'admitted': True, 'reasons': [], 'block_size': 4096}
        with patch.object(build.artifact, 'factory', return_value=self.factory), \
             patch.object(self.factory, 'validate_lock'), \
             patch.object(self.factory, 'FactoryCapacity', new=fixture_capacity), \
             patch.object(build, 'capacity_plan', return_value=plan), \
             patch.object(self.factory, 'verify_prepared', side_effect=ValueError('TEST_ONLY authentication refusal')):
            with self.assertRaisesRegex(ValueError, 'authentication refusal'):
                build.build(args)
        self.assertEqual(sentinel.read_bytes(), b'TEST_ONLY prior evidence')
        self.assertFalse(any(output.glob('.ipquality-package-*')))
        target = output / 'ipquality' / artifact.VERSION
        self.assertEqual(list(target.iterdir()), [])


if __name__ == '__main__':
    unittest.main()
