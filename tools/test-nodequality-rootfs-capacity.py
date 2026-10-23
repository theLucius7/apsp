#!/usr/bin/env python3
"""Factory capacity contracts using owned metadata and tiny local producers.

Synthetic descriptor identities and mocked filesystem observations certify only
capacity admission/cleanup behavior, never Debian authenticity, builder approval,
licenses, reproducibility or readiness of the complete diagnostic toolchain.
"""

import copy
import gzip
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import signal
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import types
import unittest
from unittest import mock


REPO = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location('nodequality_rootfs_build_capacity',
                                           REPO / 'tools/nodequality-rootfs-build.py')
BUILD = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(BUILD)


def filesystem(free_bytes=16 * 1024**3, free_inodes=1000000, block=4096):
    # Reserved filesystem blocks/inodes are deliberately unavailable to this
    # operation. Admission must not use the larger f_bfree/f_ffree values.
    return types.SimpleNamespace(f_frsize=block, f_bsize=block,
                                 f_bavail=free_bytes // block,
                                 f_bfree=free_bytes // block + 1000000,
                                 f_favail=free_inodes,
                                 f_ffree=free_inodes + 1000000,
                                 f_files=2000000)


def descriptor(name, size=1):
    return {'blob': name, 'sha256': BUILD.digest(('owned:' + name).encode()), 'size': size}


def materials():
    """Complete but inert source schema, with deliberately shared cache blobs."""
    result = {'schema': 1, 'arch': 'amd64', 'source_epoch': 1700000000,
              'keyring': descriptor('keyring.gpg'), 'repositories': [],
              'packages': [], 'sources': []}
    for identity, archive, suite in (('main', 'debian', 'bookworm'),
                                      ('security', 'debian-security', 'bookworm-security')):
        indices = [dict(descriptor('shared-' + kind + '.gz'), kind=kind, path=path)
                   for kind, path in (('Packages', 'main/binary-amd64/Packages.gz'),
                                      ('Sources', 'main/source/Sources.gz'))]
        result['repositories'].append({'id': identity, 'archive': archive,
            'timestamp': '20231115T000000Z', 'suite': suite,
            'inrelease': descriptor(identity + '.InRelease'), 'indices': indices})
    for name in sorted(set(BUILD.TOOL_PACKAGES.values())):
        result['packages'].append(dict(descriptor('shared-binary.deb'), repository='main',
            name=name, version='1', architecture='amd64',
            filename='pool/main/f/fixture/' + name + '_1_amd64.deb',
            source_name='fixture', source_version='1'))
    result['sources'] = [{'repository': 'main', 'name': 'fixture', 'version': '1',
        'directory': 'pool/main/f/fixture',
        'files': [dict(descriptor('shared-source'), name='fixture.dsc'),
                  dict(descriptor('shared-source'), name='fixture.orig.tar.xz')]}]
    return result


class FactoryCapacityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='apsp-factory-capacity-test-')
        self.root = Path(self.temporary.name).resolve()
        self.inputs = materials()

    def tearDown(self):
        self.temporary.cleanup()

    def plan(self, operation='prepare', inputs=None, **kwargs):
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()):
            return BUILD.capacity_plan(operation, self.inputs if inputs is None else inputs,
                                       self.root, **kwargs)

    def guard(self, max_output_bytes=16 * 1024**2):
        # Scale only bounded metadata constants, not production reserve policy,
        # to obtain a genuinely admitted plan for tiny runtime-output fixtures.
        with mock.patch.object(BUILD, 'MAX_LOCK', 4096), \
                mock.patch.object(BUILD, 'MAX_METADATA', 4096):
            plan = self.plan(max_output_bytes=max_output_bytes)
        self.assertTrue(plan['admitted'])
        output = self.root / ('output-' + str(len(list(self.root.iterdir()))))
        output.mkdir(mode=0o700)
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()):
            guard = BUILD.FactoryCapacity(output, plan, BUILD.Deadline(10))
        return output, guard

    def lock_file(self):
        value = copy.deepcopy(self.inputs)
        value['builder'] = {'arch': value['arch'], 'image_sha256': 'a' * 64,
            'tools': [{'name': name, 'path': path, 'version': 'owned-fixture-only',
                       'sha256': 'b' * 64, 'size': 1}
                      for name, path in sorted(BUILD.TOOL_PATHS.items())]}
        lock = self.root / 'owned-lock.json'
        lock.write_bytes(BUILD.canonical(value))
        cache = self.root / 'owned-cache'
        cache.mkdir(mode=0o700)
        for row, _ in BUILD.all_descriptors(value):
            (cache / row['blob']).write_bytes(b'x')
        return lock, cache

    def test_planning_is_read_only_and_does_not_approve_a_builder(self):
        before = set(self.root.iterdir())
        plan = self.plan()
        self.assertEqual(set(self.root.iterdir()), before)
        self.assertEqual(plan['operation'], 'prepare')
        self.assertEqual(plan['output_parent'], str(self.root))
        self.assertEqual(plan['device'], self.root.stat().st_dev)
        self.assertEqual(plan['block_size'], 4096)
        self.assertTrue(plan['admitted'])
        self.assertEqual(plan['reasons'], [])
        self.assertEqual(len(plan['input_descriptors_sha256']), 64)
        self.assertIs(plan['source_authenticated'], False)
        for field in ('builder_approved', 'full_ready', 'reproducibility_verified'):
            self.assertFalse(plan.get(field, False))

    def test_unknown_operation_and_non_integer_budgets_are_rejected(self):
        with self.assertRaises(ValueError):
            self.plan(operation='download')
        for key in ('max_output_bytes', 'reserve_free_bytes', 'reserve_free_inodes'):
            for value in (True, -1, 1.5, '4096'):
                with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                    self.plan(**{key: value})

    def test_conflicting_shared_blob_identity_is_rejected(self):
        for key, value in (('size', 2), ('sha256', 'f' * 64)):
            altered = copy.deepcopy(self.inputs)
            altered['sources'][0]['files'][1][key] = value
            with self.subTest(key=key), self.assertRaisesRegex(ValueError, 'conflict|identit'):
                self.plan(inputs=altered)

    def test_descriptor_digest_binds_bytes_and_materials_digest_binds_layout(self):
        original = self.plan()
        for mutation in ('size', 'hash', 'destination'):
            altered = copy.deepcopy(self.inputs)
            if mutation == 'size':
                altered['keyring']['size'] = 2
            elif mutation == 'hash':
                altered['keyring']['sha256'] = 'f' * 64
            else:
                altered['packages'][0]['filename'] = 'pool/main/f/fixture/other_1_amd64.deb'
            with self.subTest(mutation=mutation):
                changed = self.plan(inputs=altered)
                self.assertNotEqual(changed['materials_sha256'], original['materials_sha256'])
                if mutation == 'destination':
                    self.assertEqual(changed['input_descriptors_sha256'], original['input_descriptors_sha256'])
                    self.assertNotEqual(changed['destinations_sha256'], original['destinations_sha256'])
                else:
                    self.assertNotEqual(changed['input_descriptors_sha256'], original['input_descriptors_sha256'])

    def test_available_blocks_and_inodes_not_reserved_totals_control_admission(self):
        for observation in (filesystem(free_bytes=0), filesystem(free_inodes=0)):
            with self.subTest(observation=observation), \
                    mock.patch.object(BUILD.os, 'statvfs', return_value=observation):
                plan = BUILD.capacity_plan('prepare', self.inputs, self.root)
            self.assertFalse(plan['admitted'])
            self.assertTrue(plan['reasons'])

    def test_exact_aggregate_admission_boundary_preserves_reserve(self):
        reference = self.plan()
        required = reference['required_bytes']
        inodes = reference['required_inodes']
        reserve = reference['reserve_free_bytes']
        inode_reserve = reference['reserve_free_inodes']
        cases = [(required + reserve, inodes + inode_reserve, True),
                 (required + reserve - reference['block_size'], inodes + inode_reserve, False),
                 (required + reserve, inodes + inode_reserve - 1, False)]
        for free_bytes, free_inodes, admitted in cases:
            with self.subTest(free_bytes=free_bytes, free_inodes=free_inodes), \
                    mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem(
                        free_bytes=free_bytes, free_inodes=free_inodes)):
                plan = BUILD.capacity_plan('prepare', self.inputs, self.root, max_output_bytes=required)
            self.assertEqual(plan['admitted'], admitted)
            self.assertEqual(plan['required_bytes'], required)
            self.assertEqual(plan['required_inodes'], inodes)

    def test_reserve_cannot_be_disabled_and_invalid_block_observations_reject(self):
        for kwargs in ({'reserve_free_bytes': BUILD.DEFAULT_RESERVE_FREE - 1},
                       {'reserve_free_inodes': BUILD.DEFAULT_RESERVE_INODES - 1}):
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                self.plan(**kwargs)
        for block in (0, -1, True, BUILD.MAX_METADATA + 1):
            disk = filesystem()
            disk.f_frsize = block
            with self.subTest(block=block), mock.patch.object(BUILD.os, 'statvfs', return_value=disk), \
                    self.assertRaises(ValueError):
                BUILD.capacity_plan('prepare', self.inputs, self.root)

    def test_prepare_requires_aggregate_cache_mirror_and_directory_capacity(self):
        plan = self.plan()
        # Shared binaries consume one cache inode but a separate mirror inode
        # for each package filename. Each repo also has its own keyring/indexes.
        package_count = len(self.inputs['packages'])
        unique_cache = len({row['blob'] for row, _ in BUILD.all_descriptors(self.inputs)})
        mirror_files = package_count + 2 * (1 + 1 + 2)
        self.assertGreaterEqual(plan['required_inodes'], unique_cache + mirror_files)
        self.assertGreaterEqual(plan['required_bytes'], (unique_cache + mirror_files) * 4096)
        self.assertGreater(plan['required_inodes'], unique_cache + mirror_files)
        self.assertGreater(plan['required_bytes'], (unique_cache + mirror_files) * 4096)

    def test_prepare_block_tail_is_charged_for_every_materialized_copy(self):
        one = self.plan()
        altered = copy.deepcopy(self.inputs)
        altered['keyring']['size'] = 4097
        two = self.plan(inputs=altered)
        # One cache copy and two repository mirror copies cross a block boundary.
        self.assertGreaterEqual(two['required_bytes'] - one['required_bytes'], 3 * 4096)

    def test_larger_target_blocks_raise_prepare_budget(self):
        small = self.plan()
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem(block=65536)):
            large = BUILD.capacity_plan('prepare', self.inputs, self.root)
        self.assertEqual(large['block_size'], 65536)
        self.assertGreater(large['required_bytes'], small['required_bytes'])
        self.assertEqual(large['required_inodes'], small['required_inodes'])

    def test_build_budget_covers_tree_tails_package_cache_and_index_expansion(self):
        plan = self.plan(operation='build')
        packages = sum(row['size'] for row in self.inputs['packages'])
        packages_indices = sum(1 for repo in self.inputs['repositories']
                               for row in repo['indices'] if row['kind'] == 'Packages')
        self.assertGreaterEqual(plan['required_bytes'], BUILD.MAX_EXPANDED
                                + BUILD.MAX_MEMBERS * 4096
                                + packages + packages_indices * BUILD.MAX_INDEX)
        self.assertGreaterEqual(plan['required_inodes'], BUILD.MAX_MEMBERS)

    def test_export_budget_covers_archive_manifest_and_all_sidecars(self):
        plan = self.plan(operation='export')
        self.assertGreaterEqual(plan['required_bytes'], BUILD.MAX_ARCHIVE + BUILD.MAX_LOCK + 4 * BUILD.MAX_METADATA)
        self.assertGreaterEqual(plan['required_inodes'], 7)
        limited = self.plan(operation='export', max_output_bytes=BUILD.MAX_ARCHIVE)
        self.assertFalse(limited['admitted'])
        self.assertTrue(limited['reasons'])

    def test_aggregate_limit_cannot_be_bypassed_by_small_individual_inputs(self):
        plan = self.plan(max_output_bytes=BUILD.MAX_LOCK)
        self.assertFalse(plan['admitted'])
        self.assertTrue(plan['reasons'])
        self.assertTrue(all(row['size'] == 1 for row, _ in BUILD.all_descriptors(self.inputs)))

    def test_prepare_admission_rejects_before_output_or_signature_child(self):
        lock, cache = self.lock_file()
        output = self.root / 'not-admitted'
        before = set(self.root.iterdir())
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()), \
                mock.patch.object(BUILD, 'verify_tools'), \
                mock.patch.object(BUILD, 'verify_inputs') as authenticate, \
                mock.patch.object(BUILD.subprocess, 'Popen') as spawn:
            with self.assertRaisesRegex(ValueError, 'capacity plan rejected'):
                BUILD.prepare(lock, cache, output, 'a' * 64, max_output_bytes=BUILD.MAX_LOCK)
        authenticate.assert_not_called()
        spawn.assert_not_called()
        self.assertFalse(output.exists())
        self.assertEqual(set(self.root.iterdir()), before)

    def test_prepare_copy_failure_cleans_output_and_keeps_independent_failure_receipt(self):
        lock, cache = self.lock_file()
        output = self.root / 'failed-preparation'
        # Byte identities are intentionally wrong: bypass only synthetic
        # authentication and retain the real pre-copy identity rejection.
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()), \
                mock.patch.object(BUILD, 'verify_tools'), \
                mock.patch.object(BUILD, 'verify_inputs', return_value=({'fixture': True}, [])):
            with self.assertRaisesRegex(ValueError, '^locked input checksum or size mismatch$'):
                BUILD.prepare(lock, cache, output, 'a' * 64)
        self.assertFalse(output.exists())
        evidence = list(self.root.glob(output.name + '-failure-*'))
        self.assertEqual(len(evidence), 1)
        receipt = json.loads((evidence[0] / 'failure.json').read_bytes())
        cleanup = json.loads((evidence[0] / 'cleanup.json').read_bytes())
        self.assertEqual(receipt['operation'], 'prepare')
        self.assertEqual(receipt['error_type'], 'ValueError')
        self.assertFalse(receipt['full_ready'])
        self.assertTrue(cleanup['removed'])
        self.assertFalse(cleanup['retained'])
        self.assertTrue(lock.exists())
        self.assertEqual((cache / 'keyring.gpg').read_bytes(), b'x')

    def test_unsafe_output_parent_is_rejected_without_creation(self):
        linked = self.root / 'linked'
        linked.symlink_to(self.root, target_is_directory=True)
        with self.assertRaises((OSError, ValueError)):
            BUILD.capacity_plan('prepare', self.inputs, linked)
        writable = self.root / 'writable'
        writable.mkdir(mode=0o777)
        writable.chmod(0o777)
        with self.assertRaises(ValueError):
            BUILD.capacity_plan('prepare', self.inputs, writable)
        self.assertEqual(set(path.name for path in self.root.iterdir()), {'linked', 'writable'})

    def test_runtime_guard_preserves_bytes_and_inodes_reserve(self):
        output, guard = self.guard()
        for observation in (filesystem(free_bytes=0), filesystem(free_inodes=0)):
            with self.subTest(observation=observation), \
                    mock.patch.object(BUILD.os, 'statvfs', return_value=observation), \
                    self.assertRaises(ValueError):
                guard.check(force=True)
        self.assertTrue(output.is_dir())

    def test_rejected_plan_cannot_start_guard(self):
        plan = self.plan(max_output_bytes=BUILD.MAX_LOCK)
        output = self.root / 'rejected-output'
        output.mkdir(mode=0o700)
        with self.assertRaises(ValueError):
            BUILD.FactoryCapacity(output, plan, BUILD.Deadline(10))
        self.assertEqual(list(output.iterdir()), [])

    def test_runtime_guard_checks_deadline_and_rejects_invalid_prospective_counts(self):
        _, guard = self.guard()
        for kwargs in ({'additional_bytes': True}, {'additional_bytes': -1},
                       {'additional_inodes': True}, {'additional_inodes': -1}):
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                guard.check(force=True, **kwargs)
        guard.deadline.end = time.monotonic() - 1
        with self.assertRaisesRegex(ValueError, 'deadline'):
            guard.check(force=True)

    def test_runtime_guard_accounts_for_prospective_write_and_inode(self):
        _, guard = self.guard()
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem(
                free_bytes=BUILD.DEFAULT_RESERVE_FREE, free_inodes=BUILD.DEFAULT_RESERVE_INODES)):
            guard.check(force=True)
            for kwargs in ({'additional_bytes': 1}, {'additional_inodes': 1}):
                with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                    guard.check(force=True, **kwargs)

    def test_runtime_guard_cannot_follow_replaced_output(self):
        output, guard = self.guard()
        moved = output.with_name(output.name + '-original')
        output.rename(moved)
        output.mkdir(mode=0o700)
        with self.assertRaisesRegex(ValueError, 'identity'):
            guard.check(force=True)
        self.assertTrue(output.is_dir())
        self.assertTrue(moved.is_dir())

    def test_runtime_guard_cannot_use_replaced_parent_or_foreign_device(self):
        parent = self.root / 'parent'
        parent.mkdir(mode=0o700)
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()):
            plan = BUILD.capacity_plan('prepare', self.inputs, parent)
            output = parent / 'output'
            output.mkdir(mode=0o700)
            guard = BUILD.FactoryCapacity(output, plan, BUILD.Deadline(10))
        parent.rename(self.root / 'original-parent')
        parent.mkdir(mode=0o700)
        output.mkdir(mode=0o700)
        with self.assertRaisesRegex(ValueError, 'identity'):
            guard.check(force=True)
        ordinary, ordinary_guard = self.guard()
        real_identity = BUILD.FactoryCapacity.identity

        def identity(path):
            device, inode = real_identity(path)
            return (device + 1, inode) if Path(path) == ordinary else (device, inode)

        with mock.patch.object(BUILD.FactoryCapacity, 'identity', side_effect=identity), \
                self.assertRaises(ValueError):
            ordinary_guard.check(force=True)

    def test_guard_tolerates_owned_subdirectory_removed_during_scan(self):
        output, guard = self.guard()
        disappearing = output / 'owned-scratch'
        disappearing.mkdir(mode=0o700)
        (disappearing / 'owned').write_bytes(b'owned temporary content')
        real_open = os.open
        removed = []

        def opening(path, *args, **kwargs):
            if Path(path) == disappearing and not removed:
                removed.append(True)
                shutil.rmtree(disappearing)
            return real_open(path, *args, **kwargs)

        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()), \
                mock.patch.object(BUILD.os, 'open', side_effect=opening):
            observation = guard.check(force=True)
        self.assertEqual(removed, [True])
        self.assertFalse(disappearing.exists())
        self.assertTrue(output.is_dir())
        self.assertGreaterEqual(observation['output_inodes'], 1)

    def test_runtime_guard_rejects_output_link_without_reading_its_target(self):
        output, guard = self.guard()
        moved = output.with_name(output.name + '-original')
        output.rename(moved)
        output.symlink_to(moved, target_is_directory=True)
        with self.assertRaises((OSError, ValueError)):
            guard.check(force=True)
        self.assertTrue(output.is_symlink())
        self.assertTrue(moved.is_dir())

    def test_runtime_guard_enforces_logical_budget_even_for_sparse_file(self):
        output, guard = self.guard(max_output_bytes=1024**2)
        sparse = output / 'owned-sparse'
        with sparse.open('xb') as stream:
            stream.truncate(2 * 1024**2)
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()), \
                self.assertRaisesRegex(ValueError, 'output budget'):
            guard.check(force=True)
        self.assertEqual(sparse.stat().st_size, 2 * 1024**2)

    def test_runtime_guard_enforces_actual_block_allocation_independently_of_logical_size(self):
        output, guard = self.guard(max_output_bytes=1024**2)
        logical = allocated = 0
        for index in range(341):
            path = output / ('tiny-' + str(index))
            # Avoid compressible or inline one-byte files on filesystems such
            # as APFS. Measure actual allocation instead of assuming 4 KiB.
            path.write_bytes(os.urandom(3073))
            metadata = path.stat()
            logical += metadata.st_size
            allocated += max(metadata.st_size, metadata.st_blocks * 512)
            if allocated > guard.plan['max_output_bytes']:
                break
        self.assertLess(logical, guard.plan['max_output_bytes'])
        if allocated <= guard.plan['max_output_bytes']:
            self.skipTest('filesystem did not expose an independent block-tail allocation within bounded fixture')
        self.assertGreater(allocated, guard.plan['max_output_bytes'])
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()), \
                self.assertRaisesRegex(ValueError, 'output budget'):
            guard.check(force=True)
        self.assertEqual(sum(path.stat().st_size for path in output.iterdir()), logical)

    def test_guarded_control_write_refuses_before_creating_file(self):
        output, guard = self.guard()
        target = output / 'owned-control.json'
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem(free_bytes=BUILD.DEFAULT_RESERVE_FREE)):
            with self.assertRaisesRegex(ValueError, 'reserve'):
                guard.write(target, b'{"owned":true}\n')
        self.assertFalse(target.exists())

    def test_guarded_control_write_cannot_escape_or_replace_previous_output(self):
        output, guard = self.guard()
        existing = output / 'existing'
        existing.write_bytes(b'preserve earlier control file')
        outside = self.root / 'outside'
        for target in (outside, output / 'missing-parent' / 'nested'):
            with self.subTest(target=target), self.assertRaises(ValueError):
                guard.write(target, b'owned new content')
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()), \
                self.assertRaises(FileExistsError):
            guard.write(existing, b'wrong replacement')
        self.assertEqual(existing.read_bytes(), b'preserve earlier control file')
        self.assertFalse(outside.exists())

    def test_cleanup_never_deletes_same_path_replacement(self):
        output, guard = self.guard()
        original = output.with_name(output.name + '-original')
        output.rename(original)
        output.mkdir(mode=0o700)
        sentinel = output / 'replacement-sentinel'
        sentinel.write_bytes(b'not the owned output')
        with self.assertRaisesRegex(ValueError, 'replaced'):
            BUILD.cleanup_output(output, capacity=guard)
        self.assertEqual(sentinel.read_bytes(), b'not the owned output')
        self.assertTrue(original.is_dir())

    def test_failure_evidence_is_new_private_bounded_and_tracks_cleanup(self):
        output, guard = self.guard()
        original = ValueError('owned capacity failure')
        original.factory_command = {'argv': ['owned-synthetic-producer'],
            'output': b'owned command log\n', 'output_truncated': False,
            'returncode': None, 'cleanup_returncode': -signal.SIGKILL}
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()):
            first = BUILD.preserve_factory_failure(output, 'build', original, guard)
            second = BUILD.preserve_factory_failure(output, 'build', original, guard)
        self.assertIsNotNone(first)
        self.assertIsNotNone(second)
        directory, receipt, _ = first
        self.assertNotEqual(directory, second[0])
        self.assertEqual(directory.parent, self.root)
        self.assertFalse(stat.S_IMODE(directory.stat().st_mode) & 0o077)
        for path in (directory / 'failure.json', directory / 'command.log'):
            self.assertFalse(stat.S_IMODE(path.stat().st_mode) & 0o077)
        self.assertEqual((directory / 'command.log').read_bytes(), b'owned command log\n')
        self.assertEqual(receipt['command']['output'], {'path': 'command.log',
            'size': len(b'owned command log\n'), 'sha256': BUILD.digest(b'owned command log\n')})
        self.assertEqual(receipt['command']['cleanup_returncode'], -signal.SIGKILL)
        self.assertFalse(receipt['full_ready'])
        preserved = (directory / 'failure.json').read_bytes()
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()):
            BUILD.record_factory_cleanup(first, False, original)
        cleanup = json.loads((directory / 'cleanup.json').read_bytes())
        self.assertFalse(cleanup['removed'])
        self.assertTrue(cleanup['retained'])
        self.assertEqual((directory / 'failure.json').read_bytes(), preserved)
        self.assertTrue(output.exists())

    def test_failure_evidence_cannot_consume_management_reserve_or_replace_original(self):
        output, guard = self.guard()
        original = ValueError('owned original capacity failure')
        original.factory_command = {'argv': ['owned-synthetic-producer'],
            'output': b'owned retained memory log', 'output_truncated': False,
            'returncode': None, 'cleanup_returncode': -signal.SIGKILL}
        before = set(self.root.iterdir())
        for observation in (filesystem(free_bytes=BUILD.DEFAULT_RESERVE_FREE),
                            filesystem(free_inodes=BUILD.DEFAULT_RESERVE_INODES)):
            with self.subTest(observation=observation), \
                    mock.patch.object(BUILD.os, 'statvfs', return_value=observation), \
                    mock.patch.object(BUILD.sys, 'stderr', io.StringIO()):
                result = BUILD.preserve_factory_failure(output, 'build', original, guard)
            self.assertIsNone(result)
            self.assertEqual(str(original), 'owned original capacity failure')
            self.assertEqual(original.factory_command['output'], b'owned retained memory log')
            self.assertEqual(set(self.root.iterdir()), before)

    def test_locked_copy_uses_held_regular_fd_after_path_becomes_fifo(self):
        cache = self.root / 'copy-cache'
        cache.mkdir(mode=0o700)
        source = cache / 'owned-source'
        content = b'owned regular source bytes'
        source.write_bytes(content)
        value = {'blob': source.name, 'sha256': BUILD.digest(content), 'size': len(content)}
        target = self.root / 'copied'
        real_open, opened = BUILD.open_regular, []

        def opening(path, limit):
            stream, metadata = real_open(path, limit)
            if Path(path) == source:
                opened.append(stream)
                # The first descriptor belongs to checked_blob prevalidation.
                # Replace the path only after the actual copy holds its FD.
                if len(opened) == 2:
                    self.assertTrue(opened[0].closed)
                    source.unlink()
                    os.mkfifo(source)
            return stream, metadata

        with mock.patch.object(BUILD, 'open_regular', side_effect=opening):
            BUILD.copy_locked(cache, value, target, BUILD.MAX_ARCHIVE, BUILD.Deadline(5))
        self.assertEqual(target.read_bytes(), content)
        self.assertTrue(stat.S_ISFIFO(source.lstat().st_mode))
        self.assertEqual(len(opened), 2)
        self.assertTrue(all(stream.closed for stream in opened))

    def test_locked_copy_rejects_growth_before_writing_excess_and_closes_fd(self):
        cache = self.root / 'copy-cache'
        cache.mkdir(mode=0o700)
        source = cache / 'owned-source'
        content = b'abcd'
        source.write_bytes(content)
        value = {'blob': source.name, 'sha256': BUILD.digest(content), 'size': len(content)}
        target = self.root / 'not-copied'
        real_open, opened = BUILD.open_regular, []

        def opening(path, limit):
            stream, metadata = real_open(path, limit)
            if Path(path) == source:
                opened.append(stream)
                # Exercise the copying bound after the initial input checksum
                # succeeds, before the held copy descriptor reads any bytes.
                if len(opened) == 2:
                    self.assertTrue(opened[0].closed)
                    with source.open('ab') as append:
                        append.write(b'owned growth')
            return stream, metadata

        with mock.patch.object(BUILD, 'open_regular', side_effect=opening), \
                self.assertRaisesRegex(ValueError, '^copied locked input exceeds byte limit$'):
            BUILD.copy_locked(cache, value, target, BUILD.MAX_ARCHIVE, BUILD.Deadline(5))
        self.assertEqual(len(opened), 2)
        self.assertTrue(all(stream.closed for stream in opened))
        self.assertEqual(target.stat().st_size, 0)

    def test_compressed_archive_footer_cannot_cross_capacity_writer_limit(self):
        stream = io.BytesIO()
        capacity = mock.Mock()
        capacity.plan = {'block_size': 4096}
        writer = BUILD.CapacityWriter(stream, capacity, 18)
        compressed = gzip.GzipFile(filename='', mode='wb', mtime=0, fileobj=writer)
        before = len(stream.getvalue())
        self.assertLess(before, 18)
        with self.assertRaisesRegex(ValueError, 'archive budget'):
            compressed.close()
        self.assertGreater(len(stream.getvalue()), before)
        self.assertLessEqual(len(stream.getvalue()), 18)
        self.assertTrue(capacity.check.called)

    def test_capacity_receipt_is_private_bounded_and_not_authentication(self):
        output, guard = self.guard()
        (output / 'owned').write_bytes(b'owned input')
        with mock.patch.object(BUILD.os, 'statvfs', return_value=filesystem()):
            guard.finish()
        path = output / 'factory-capacity.json'
        raw = path.read_bytes()
        self.assertLessEqual(len(raw), BUILD.MAX_LOCK)
        receipt = json.loads(raw)
        self.assertEqual(receipt['plan_sha256'], BUILD.digest(BUILD.canonical(guard.plan) + b'\n'))
        self.assertGreaterEqual(receipt['observation']['output_inodes'], 2)
        self.assertGreaterEqual(receipt['observation']['output_bytes'], len(b'owned input'))
        for key in ('hard_quota', 'source_authenticated', 'builder_approved', 'full_ready'):
            self.assertIs(receipt[key], False)
        self.assertFalse(stat.S_IMODE(output.stat().st_mode) & 0o077)

    def test_guard_failure_does_not_delete_remaining_mount_output(self):
        output, _ = self.guard()
        sentinel = output / 'owned-sentinel'
        sentinel.write_bytes(b'keep this owned output')
        error = ValueError('remaining mount; owned output retained')
        with mock.patch.object(BUILD, 'ensure_no_mounts', side_effect=error):
            with self.assertRaises(ValueError) as caught:
                BUILD.cleanup_output(output, guard_mounts=True)
        self.assertIs(caught.exception, error)
        self.assertEqual(sentinel.read_bytes(), b'keep this owned output')

    def test_capacity_failure_before_spawn_never_runs_command(self):
        original = ValueError('owned capacity admission failure')
        guard = mock.Mock()
        guard.check.side_effect = original
        # Admission fails before any primitive is invoked or process starts;
        # keep this mocked boundary portable even without Linux waitid.
        with mock.patch.object(BUILD, 'hasattr', return_value=True, create=True), \
                mock.patch.object(BUILD.subprocess, 'Popen') as spawn:
            with self.assertRaises(ValueError) as caught:
                BUILD.run_bounded([sys.executable, '-c', 'raise SystemExit(99)'],
                                  BUILD.Deadline(5), 64, capacity=guard)
        self.assertIs(caught.exception, original)
        spawn.assert_not_called()
        self.assertEqual(caught.exception.factory_command['output'], b'')
        self.assertIsNone(caught.exception.factory_command['returncode'])

    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'waitid') and hasattr(os, 'WNOWAIT'),
                         'Linux waitid/WNOWAIT child collector required')
    def test_live_disk_guard_stops_tiny_child_and_preserves_bounded_output_status(self):
        output, guard = self.guard()
        ready = output / 'owned-child.pid'
        code = ('import os, pathlib, sys, time; pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); '
                'os.write(1, b"owned bounded child output\\n"); time.sleep(30)')
        real_spawn = subprocess.Popen
        children = []

        def spawn(*args, **kwargs):
            process = real_spawn(*args, **kwargs)
            children.append(process)
            return process

        def space(_path):
            return filesystem(free_bytes=0) if ready.exists() else filesystem()

        with mock.patch.object(BUILD.os, 'statvfs', side_effect=space), \
                mock.patch.object(BUILD.subprocess, 'Popen', side_effect=spawn):
            with self.assertRaisesRegex(ValueError, 'reserve') as caught:
                BUILD.run_bounded([sys.executable, '-c', code, str(ready)],
                                  BUILD.Deadline(5), 64, capacity=guard)
        self.assertTrue(ready.exists())
        self.assertEqual(len(children), 1)
        self.assertEqual(children[0].returncode, -signal.SIGKILL)
        self.assertTrue(children[0].stdout.closed)
        with self.assertRaises(ProcessLookupError):
            os.kill(int(ready.read_text()), 0)
        command = caught.exception.factory_command
        self.assertEqual(command['output'], b'owned bounded child output\n')
        self.assertFalse(command['output_truncated'])
        self.assertIsNone(command['returncode'])
        self.assertEqual(command['cleanup_returncode'], -signal.SIGKILL)

    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'waitid') and hasattr(os, 'WNOWAIT'),
                         'waitid/WNOWAIT child collector required')
    def test_closed_log_pipes_do_not_stop_runtime_capacity_observation(self):
        output, guard = self.guard()
        ready = output / 'closed-logs.pid'
        code = ('import os, pathlib, sys, time; os.close(1); os.close(2); '
                'pathlib.Path(sys.argv[1]).write_text(str(os.getpid())); time.sleep(30)')

        def space(_path):
            return filesystem(free_bytes=0) if ready.exists() else filesystem()

        with mock.patch.object(BUILD.os, 'statvfs', side_effect=space):
            with self.assertRaisesRegex(ValueError, 'reserve') as caught:
                BUILD.run_bounded([sys.executable, '-c', code, str(ready)],
                                  BUILD.Deadline(5), 64, capacity=guard)
        self.assertTrue(ready.exists())
        self.assertEqual(caught.exception.factory_command['cleanup_returncode'], -signal.SIGKILL)
        with self.assertRaises(ProcessLookupError):
            os.kill(int(ready.read_text()), 0)

    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'waitid') and hasattr(os, 'WNOWAIT'),
                         'Linux waitid/WNOWAIT child collector required')
    def test_nonzero_child_exit_retains_actual_code_without_replacing_error(self):
        with self.assertRaisesRegex(ValueError, 'command failed') as caught:
            BUILD.run_bounded([sys.executable, '-c',
                               'import os; os.write(1, b"owned failure\\n"); raise SystemExit(7)'],
                              BUILD.Deadline(5), 64)
        self.assertEqual(caught.exception.factory_command['output'], b'owned failure\n')
        self.assertEqual(caught.exception.factory_command['returncode'], 7)
        self.assertEqual(caught.exception.factory_command['cleanup_returncode'], 7)

    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'waitid') and hasattr(os, 'WNOWAIT'),
                         'Linux owned child-group capacity acceptance required')
    def test_capacity_guard_reaps_owned_producer_and_descendant_group(self):
        # The separate subreaper collects its owned grandchild. No assertion
        # depends on a surrounding container's PID 1 eventually reaping zombies.
        producer = (
            'import json, os, pathlib, subprocess, sys, time\n'
            'child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])\n'
            'ready = pathlib.Path(sys.argv[1]); temporary = pathlib.Path(sys.argv[1] + ".tmp")\n'
            'temporary.write_text(json.dumps({"leader": os.getpid(), "child": child.pid})); os.replace(temporary, ready)\n'
            'os.write(1, b"owned group output\\n")\n'
            'time.sleep(30)\n'
        )
        harness = (
            'import ctypes, importlib.util, json, os, pathlib, signal, sys, time, types\n'
            'if ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) != 0: raise OSError("owned subreaper setup failed")\n'
            'spec = importlib.util.spec_from_file_location("owned_builder", sys.argv[1])\n'
            'module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)\n'
            'root = pathlib.Path(sys.argv[2]); ready = root / "group.ready"\n'
            'output = root / "group-output"; output.mkdir(mode=0o700)\n'
            'plan = json.loads(pathlib.Path(sys.argv[3]).read_text())\n'
            'def space(path):\n'
            '    return types.SimpleNamespace(f_frsize=4096, f_bavail=0 if ready.exists() else 4000000, f_favail=1000000)\n'
            'module.os.statvfs = space\n'
            'guard = module.FactoryCapacity(output, plan, module.Deadline(10))\n'
            'result = {}\n'
            'try:\n'
            '    with module.cli_signals():\n'
            '        module.run_bounded([sys.executable, "-c", sys.argv[4], str(ready)], module.Deadline(5), 64, capacity=guard)\n'
            'except ValueError as error:\n'
            '    command = error.factory_command\n'
            '    result = {"error": str(error), "output": command["output"].decode("ascii"),\n'
            '              "returncode": command["returncode"], "cleanup_returncode": command["cleanup_returncode"]}\n'
            'finally:\n'
            '    reaped = []; end = time.monotonic() + 5\n'
            '    while time.monotonic() < end:\n'
            '        try: pid, status = os.waitpid(-1, os.WNOHANG)\n'
            '        except ChildProcessError: break\n'
            '        if pid: reaped.append({"pid": pid, "status": status})\n'
            '        else: time.sleep(0.01)\n'
            '    result["reaped"] = reaped\n'
            '    pathlib.Path(sys.argv[5]).write_text(json.dumps(result))\n'
        )
        plan_path = self.root / 'group-plan.json'
        plan_path.write_bytes(BUILD.canonical(self.plan()))
        ready = self.root / 'group.ready'
        receipt = self.root / 'group.receipt'
        process = subprocess.Popen([sys.executable, '-c', harness,
            str(REPO / 'tools/nodequality-rootfs-build.py'), str(self.root), str(plan_path),
            producer, str(receipt)], stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, start_new_session=True)
        group = None
        try:
            stdout, stderr = process.communicate(timeout=12)
            self.assertEqual(process.returncode, 0, stderr.decode(errors='replace'))
            self.assertEqual(stdout, b'')
            self.assertEqual(stderr, b'')
            self.assertTrue(ready.exists())
            group = json.loads(ready.read_text())
            result = json.loads(receipt.read_text())
            self.assertIn('reserve', result['error'])
            self.assertEqual(result['output'], 'owned group output\n')
            self.assertIsNone(result['returncode'])
            self.assertEqual(result['cleanup_returncode'], -signal.SIGKILL)
            self.assertEqual([row['pid'] for row in result['reaped']], [group['child']])
            status = result['reaped'][0]['status']
            self.assertTrue(os.WIFSIGNALED(status))
            self.assertEqual(os.WTERMSIG(status), signal.SIGKILL)
            for pid in group.values():
                with self.assertRaises(ProcessLookupError):
                    os.kill(pid, 0)
        finally:
            if process.poll() is None:
                os.kill(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
            if group is None and ready.exists():
                group = json.loads(ready.read_text())
            if group is not None:
                try:
                    if os.getpgid(group['child']) == group['leader']:
                        os.killpg(group['leader'], signal.SIGKILL)
                except ProcessLookupError:
                    pass
            process.stdout.close()
            process.stderr.close()


class OwnedLoopFilesystemTests(unittest.TestCase):
    """Explicitly enabled real ext4 observations in an existing private namespace.

    Each image, loop association, mount, output and producer belongs to this
    fixture. A cleanup identity ambiguity retains evidence instead of touching
    another association or recursively removing a still-mounted filesystem.
    """

    IMAGE_BYTES = 600 * 1024**2
    MAX_PRODUCER_BYTES = 96 * 1024**2

    def setUp(self):
        self.root = self.image = self.mountpoint = self.loop = None
        self.loop_started = False
        self.evidence = None
        if os.environ.get('APSP_FACTORY_LOOP_FIXTURE') != '1':
            self.skipTest('real owned filesystem fixture requires APSP_FACTORY_LOOP_FIXTURE=1')
        if sys.platform != 'linux' or os.geteuid() != 0:
            self.skipTest('real owned filesystem fixture requires Linux root')
        if not hasattr(os, 'waitid') or not hasattr(os, 'WNOWAIT'):
            self.skipTest('real owned filesystem fixture requires waitid/WNOWAIT')
        self.namespace = Path('/proc/self/ns/mnt').stat().st_ino
        if self.namespace == Path('/proc/1/ns/mnt').stat().st_ino:
            self.skipTest('real filesystem fixture refuses the PID 1 mount namespace')
        self.tools = {name: shutil.which(name) for name in ('mkfs.ext4', 'mount', 'umount', 'losetup')}
        missing = [name for name, path in self.tools.items() if path is None]
        if missing:
            self.skipTest('real filesystem fixture missing tools: ' + ','.join(missing))
        if not Path('/dev/loop-control').exists():
            self.skipTest('real filesystem fixture has no existing loop-control device')
        self.root = Path(tempfile.mkdtemp(prefix='apsp-owned-factory-fs-')).resolve()
        self.root_identity = BUILD.FactoryCapacity.identity(self.root)
        self.addCleanup(self.cleanup_fixture)
        observed = os.statvfs(self.root)
        needed = BUILD.DEFAULT_RESERVE_FREE + self.MAX_PRODUCER_BYTES + 8 * 1024**2
        if observed.f_bavail * observed.f_frsize < needed:
            self.skipTest('backing filesystem lacks 512 MiB reserve plus bounded fixture allocation')
        self.image = self.root / 'owned-capacity.img'
        with self.image.open('xb') as image:
            image.truncate(self.IMAGE_BYTES)
        self.image.chmod(0o600)
        metadata = self.image.lstat()
        self.image_identity = (metadata.st_dev, metadata.st_ino, metadata.st_size)
        self.mountpoint = self.root / 'owned-mount'
        self.mountpoint.mkdir(mode=0o700)
        self.command([self.tools['mkfs.ext4'], '-F', '-N', '2048', '-m0', '-O', '^has_journal', str(self.image)])
        self.loop_started = True
        loop = self.command([self.tools['losetup'], '--find', '--show', str(self.image)]).decode('ascii').strip()
        self.assertRegex(loop, r'^/dev/loop[0-9]+$')
        self.loop = loop
        self.assert_owned_loop()
        self.command([self.tools['mount'], '-o', 'nodev,nosuid,noexec', self.loop, str(self.mountpoint)])
        mounted = self.mounted()
        self.assertEqual(len(mounted), 1)
        self.mount_identity = mounted[0]
        self.assertEqual(self.mount_identity['source'], self.loop)
        self.assertEqual(self.mount_identity['filesystem'], 'ext4')
        self.parent = self.mountpoint / 'owned-parent'
        self.parent.mkdir(mode=0o700)
        self.sentinel = self.parent / 'old-sentinel'
        self.sentinel_content = b'preserve existing owned evidence\n'
        self.sentinel.write_bytes(self.sentinel_content)

    def command(self, arguments):
        # Only the explicitly selected local filesystem utilities are invoked.
        return BUILD.run_bounded(arguments, BUILD.Deadline(30), 65536)

    def mounted(self):
        if self.mountpoint is None:
            return []
        rows = []
        inventory = BUILD.read_regular('/proc/self/mountinfo', BUILD.MAX_LOCK).decode('utf-8')
        for line in inventory.splitlines():
            fields = line.split()
            self.assertGreaterEqual(len(fields), 6)
            path = re.sub(r'\\([0-7]{3})', lambda value: chr(int(value[1], 8)), fields[4])
            if path == str(self.mountpoint):
                marker = fields.index('-')
                rows.append({'mount_id': fields[0], 'device': fields[2],
                             'filesystem': fields[marker + 1], 'source': fields[marker + 2]})
        return rows

    def loop_rows(self):
        raw = self.command([self.tools['losetup'], '--list', '--json', '--output', 'NAME,BACK-FILE', self.loop])
        result = json.loads(raw)
        self.assertEqual(set(result), {'loopdevices'})
        self.assertIsInstance(result['loopdevices'], list)
        return result['loopdevices']

    def assert_owned_loop(self):
        self.assertIsNotNone(self.loop)
        self.assertEqual(Path('/proc/self/ns/mnt').stat().st_ino, self.namespace)
        self.assertEqual(BUILD.FactoryCapacity.identity(self.root), self.root_identity)
        metadata = self.image.lstat()
        self.assertTrue(stat.S_ISREG(metadata.st_mode))
        self.assertEqual((metadata.st_dev, metadata.st_ino, metadata.st_size), self.image_identity)
        rows = self.loop_rows()
        self.assertEqual(len(rows), 1)
        self.assertEqual(rows[0]['name'], self.loop)
        self.assertEqual(rows[0]['back-file'], str(self.image))
        device = Path(self.loop).lstat()
        self.assertTrue(stat.S_ISBLK(device.st_mode))
        return str(os.major(device.st_rdev)) + ':' + str(os.minor(device.st_rdev))

    def real_observation(self):
        disk = os.statvfs(self.parent)
        host = os.statvfs(self.root)
        return {'block_size': disk.f_frsize, 'free_bytes': disk.f_bavail * disk.f_frsize,
                'free_inodes': disk.f_favail, 'total_inodes': disk.f_files,
                'backing_free_bytes': host.f_bavail * host.f_frsize}

    def exercise(self, operation):
        plan = BUILD.capacity_plan('prepare', materials(), self.parent)
        if not plan['admitted']:
            self.skipTest('real target filesystem cannot admit fixture: ' + ','.join(plan['reasons']))
        output = self.parent / 'owned-output'
        output.mkdir(mode=0o700)
        guard = BUILD.FactoryCapacity(output, plan, BUILD.Deadline(35))
        ready = output / 'owned-producer.pid'
        code = (
            'import os, pathlib, sys, time\n'
            'output = pathlib.Path(sys.argv[1]); backing = pathlib.Path(sys.argv[2])\n'
            '(output / "owned-producer.pid").write_text(str(os.getpid()))\n'
            'os.write(1, b"owned real filesystem producer\\n")\n'
            'def backing_space(additional):\n'
            '    disk = os.statvfs(backing)\n'
            '    if disk.f_bavail * disk.f_frsize < 512 * 1024**2 + additional:\n'
            '        os.write(1, b"owned backing capacity precondition lost\\n"); raise SystemExit(86)\n'
            'if sys.argv[3] == "disk":\n'
            '    with (output / "owned-data").open("xb") as destination:\n'
            '        for index in range(96):\n'
            '            backing_space(1024**2 + 4096)\n'
            '            destination.write(b"x" * 1024**2); destination.flush(); os.fsync(destination.fileno())\n'
            '            time.sleep(0.01)\n'
            'else:\n'
            '    for index in range(1400):\n'
            '        backing_space(4096)\n'
            '        with (output / ("owned-inode-" + str(index))).open("xb") as destination:\n'
            '            destination.write(b"x"); destination.flush(); os.fsync(destination.fileno())\n'
            '        time.sleep(0.002)\n'
            'time.sleep(30)\n'
        )
        children, real_spawn = [], subprocess.Popen

        def spawn(*args, **kwargs):
            process = real_spawn(*args, **kwargs)
            children.append(process)
            return process

        self.evidence = {'case': operation, 'baseline': self.real_observation(),
                         'plan_required_bytes': plan['required_bytes'],
                         'plan_required_inodes': plan['required_inodes'],
                         'reserve_free_bytes': plan['reserve_free_bytes'],
                         'reserve_free_inodes': plan['reserve_free_inodes'],
                         'loop': self.loop, 'image_identity': list(self.image_identity),
                         'mount_identity': self.mount_identity, 'cleanup': {'complete': False}}
        with mock.patch.object(BUILD.subprocess, 'Popen', side_effect=spawn):
            with self.assertRaises(ValueError) as caught:
                BUILD.run_bounded([sys.executable, '-c', code, str(output), str(self.root), operation],
                                  BUILD.Deadline(30), 1024, capacity=guard)
        error = caught.exception
        self.evidence.update({'error': str(error), 'trigger': self.real_observation(),
                              'guard_observation': guard.last,
                              'child_returncode': children[0].returncode,
                              'command_cleanup_returncode': error.factory_command['cleanup_returncode']})
        if error.factory_command['returncode'] == 86:
            self.evidence['unavailable'] = 'backing filesystem reserve precondition lost'
            self.skipTest(self.evidence['unavailable'])
        category = 'free-disk reserve' if operation == 'disk' else 'free-inode reserve'
        self.assertIn(category, str(error))
        self.assertEqual(len(children), 1)
        self.assertEqual(children[0].returncode, -signal.SIGKILL)
        self.assertTrue(children[0].stdout.closed)
        self.assertTrue(ready.exists())
        pid = int(ready.read_text())
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        self.assertEqual(error.factory_command['cleanup_returncode'], -signal.SIGKILL)
        self.assertEqual(self.sentinel.read_bytes(), self.sentinel_content)
        key = 'free_bytes' if operation == 'disk' else 'free_inodes'
        reserve_key = 'reserve_free_bytes' if operation == 'disk' else 'reserve_free_inodes'
        self.assertLess(guard.last[key], plan[reserve_key])
        if operation == 'disk':
            self.assertLessEqual((output / 'owned-data').stat().st_size, self.MAX_PRODUCER_BYTES)
        else:
            self.assertLessEqual(len(list(output.glob('owned-inode-*'))), 1400)
        self.assertTrue(BUILD.cleanup_output(output, guard_mounts=True, capacity=guard))
        self.assertEqual(self.sentinel.read_bytes(), self.sentinel_content)
        self.evidence['old_sentinel_preserved'] = True
        self.evidence['producer_pid_absent'] = pid

    def test_real_filesystem_free_disk_guard_stops_owned_producer(self):
        self.exercise('disk')

    def test_real_filesystem_free_inode_guard_stops_owned_producer(self):
        self.exercise('inodes')

    def cleanup_fixture(self):
        if self.root is None:
            return
        cleanup = {'complete': False, 'mount_absent': False, 'loop_detached': False,
                   'image_removed': False, 'private_directory_removed': False}
        try:
            self.assertEqual(Path('/proc/self/ns/mnt').stat().st_ino, self.namespace)
            self.assertEqual(BUILD.FactoryCapacity.identity(self.root), self.root_identity)
            mounts = self.mounted()
            if mounts:
                device = self.assert_owned_loop()
                self.assertEqual(len(mounts), 1)
                self.assertEqual(mounts[0]['device'], device)
                self.assertEqual(mounts[0]['source'], self.loop)
                if hasattr(self, 'mount_identity'):
                    self.assertEqual(mounts[0], self.mount_identity)
                self.command([self.tools['umount'], str(self.mountpoint)])
            self.assertEqual(self.mounted(), [])
            cleanup['mount_absent'] = True
            BUILD.ensure_no_mounts(self.root)
            if self.loop is not None:
                self.assert_owned_loop()
                self.command([self.tools['losetup'], '--detach', self.loop])
                detached = self.loop_rows()
                # util-linux may retain an unused loop device row after detach.
                # Its association, rather than the device node, must be absent.
                if detached:
                    self.assertEqual(len(detached), 1)
                    self.assertEqual(detached[0]['name'], self.loop)
                    self.assertIsNone(detached[0]['back-file'])
                cleanup['loop_detached'] = True
            else:
                self.assertFalse(self.loop_started, 'unidentified loop association; retain owned image')
                cleanup['loop_detached'] = True
            if self.image is not None:
                metadata = self.image.lstat()
                self.assertEqual((metadata.st_dev, metadata.st_ino, metadata.st_size), self.image_identity)
                self.image.unlink()
                self.assertFalse(self.image.exists())
            cleanup['image_removed'] = True
            self.assertEqual(BUILD.FactoryCapacity.identity(self.root), self.root_identity)
            shutil.rmtree(self.root)
            cleanup['private_directory_removed'] = not self.root.exists()
            cleanup['complete'] = cleanup['private_directory_removed']
        except BaseException as error:
            cleanup['error_type'] = type(error).__name__
            cleanup['retained_private_path'] = str(self.root)
            raise
        finally:
            if self.evidence is not None:
                self.evidence['cleanup'] = cleanup
                print('FACTORY_FS_EVIDENCE_JSON=' + json.dumps(self.evidence, sort_keys=True), flush=True)


if __name__ == '__main__':
    unittest.main(verbosity=2)
