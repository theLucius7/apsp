#!/usr/bin/env python3
"""Exact IP profile fixtures, without claiming native builder acceptance.

The parent, cache, binding, sidecar hashes and ordinary-file identities are real
temporary bytes. Only native GPG/APT, OS, tool and capacity observations have
explicit TEST_ONLY substitutes. These tests never approve a builder or a license.
"""
import contextlib
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]


def module(name, path):
    specification = importlib.util.spec_from_file_location(name, path)
    result = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(result)
    return result


class MinimalProfileTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Imports happen only when the frozen final suite is explicitly run.
        cls.fixtures = module('apsp_ipquality_profile_parent_fixtures',
                              ROOT / 'tools/test-ipquality-inputs.py')
        cls.inputs = module('apsp_ipquality_profile_fixture_inputs',
                            ROOT / 'tools/ipquality-inputs.py')
        cls.loaded = cls.inputs.modules()
        cls.profile_module = module('apsp_ipquality_profile_contract',
                                    ROOT / 'tools/ipquality-profile.py')

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='apsp-ip-profile-fixture-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.build = self.loaded['build']
        self.profile = self.profile_module.Profile(self.build, self.loaded['profile'].TOOLS)
        self.fixture = self.fixtures.ParentFixture(self.root / 'parent',
            self.loaded['parent_build'], self.loaded['profile'])
        self.isolation = contextlib.ExitStack()
        self.addCleanup(self.isolation.close)
        self.isolation.enter_context(patch.object(self.inputs, 'modules', return_value=self.loaded))
        self.isolation.enter_context(patch.object(self.profile_module, 'inputs', return_value=self.inputs))
        self.isolation.enter_context(patch.object(self.build, 'INPUT_PROFILE', self.profile))
        self.isolation.enter_context(patch.object(self.loaded['collect'], 'native_arch', return_value='amd64'))
        factories = {id(value): value for value in (self.build, self.loaded['collect'].BUILD,
            self.loaded['parent_build'], self.loaded['capacity'].BUILD)}
        for factory in factories.values():
            self.isolation.enter_context(patch.object(factory, 'run_bounded', side_effect=self.fixture.gpgv))
            self.isolation.enter_context(patch.object(factory, 'ensure_no_mounts'))
        self.memory_observation = self.isolation.enter_context(patch.object(
            self.loaded['capacity'], 'available_memory', return_value=self.memory()))
        self.disk = self.isolation.enter_context(patch.object(self.inputs.os, 'statvfs',
                                                             return_value=self.space()))
        self.tool_path = self.root / 'owned-native-tool'
        self.tool_path.write_bytes(b'TEST_ONLY isolated native collection tool')
        self.tools = {'executables': [dict(requested_path=str(self.tool_path),
            resolved_path=str(self.tool_path), dpkg_ownership='TEST_ONLY local ownership',
            **self.identity(self.tool_path.read_bytes()))],
            'dpkg_report': 'TEST_ONLY no builder approval',
            'actual_version_output': {str(self.tool_path): 'TEST_ONLY inert version'},
            'evidence_scope': 'TEST_ONLY isolated tool boundary'}
        self.isolation.enter_context(patch.object(self.loaded['collect'], 'record_collection_tools',
                                                 return_value=self.tools))
        self.selected_override = None
        self.solver = self.isolation.enter_context(patch.object(self.loaded['collect'], 'solve',
                                                               side_effect=self.solve_fixture))
        self.candidate_path = self.make_candidate()
        for factory in {id(value): value for value in (self.build, self.loaded['collect'].BUILD)}.values():
            original = factory.file_identity
            self.isolation.enter_context(patch.object(factory, 'file_identity',
                side_effect=self.candidate_reader(original)))
        self.derived = self.root / 'derived'
        self.bound = self.root / 'bound'
        self.inputs.derive(self.fixture.directory, self.derived, 64 * 1024**2, 60)
        self.inputs.bind(self.derived, self.candidate_path, self.bound, 60,
                         max_output_bytes=64 * 1024**2)
        self.lock = json.loads((self.bound / 'inputs-lock.json').read_bytes())
        self.context = {'derived_inputs': str(self.derived), 'derived_binding': str(self.bound)}

    def identity(self, content):
        return self.fixtures.identity(content)

    def memory(self, available=8 * 1024**3):
        return self.fixtures.DerivedInputsTests.memory(self, available)

    def space(self, **changes):
        return self.fixtures.DerivedInputsTests.space(self, **changes)

    def solve_fixture(self, collector, repositories, indices, keyring, authenticated_expansion=None):
        return self.fixtures.DerivedInputsTests.solve_fixture(
            self, collector, repositories, indices, keyring, authenticated_expansion)

    def make_candidate(self):
        self.candidate_files = {}
        tools = []
        for name, native_path in sorted(self.build.TOOL_PATHS.items()):
            path = self.root / ('candidate-' + name)
            path.write_bytes(('TEST_ONLY native tool IO: ' + name).encode())
            self.candidate_files[native_path] = path
            tools.append(dict(name=name, path=native_path, version='TEST_ONLY inert native version',
                              **self.identity(path.read_bytes())))
        path = self.root / 'candidate-builder.json'
        path.write_bytes(self.fixtures.encoded({'image_sha256': self.identity(
            b'TEST_ONLY image identity, not approval')['sha256'], 'arch': 'amd64', 'tools': tools}))
        return path

    def candidate_reader(self, original):
        def observed(path, limit, deadline=None):
            if str(path) in self.candidate_files:
                value = self.inputs.snapshot_file(self.candidate_files[str(path)], limit,
                                                   deadline or self.build.Deadline(60))
                return {key: value[key] for key in ('size', 'sha256')}
            return original(path, limit, deadline)
        return observed

    def admission(self, name):
        output = self.root / name
        output.mkdir(mode=0o700)
        deadline = self.build.Deadline(60)
        plan = self.inputs.output_plan(self.build, self.root, 64 * 1024**2, self.inputs.RESERVE)
        deadline.capacity = self.build.FactoryCapacity(output, plan, deadline)
        return output, deadline

    def replay(self, name='replay', context=None, lock=None):
        output, deadline = self.admission(name)
        proof = self.profile.replay(context or self.context, lock or self.lock,
                                    self.fixture.cache, deadline)
        return proof, output, deadline

    def change_derived(self, name, value):
        content = self.fixtures.encoded(value)
        (self.derived / name).write_bytes(content)
        receipt = json.loads((self.derived / 'derivation.json').read_bytes())
        receipt['files_sha256'][name] = self.identity(content)['sha256']
        (self.derived / 'derivation.json').write_bytes(self.fixtures.encoded(receipt))

    def prepare(self, name='prepared'):
        self.profile.prepare_context = self.context
        output = self.root / name
        receipt = self.build.prepare(self.bound / 'inputs-lock.json', self.fixture.cache, output,
                                     self.lock['builder']['image_sha256'])
        return output, receipt

    def verify_prepared(self, prepared, name='verify'):
        output, deadline = self.admission(name)
        result = self.build.verify_prepared(prepared, self.lock['builder']['image_sha256'],
                                            _deadline=deadline)
        return result, output, deadline

    def assert_no_solver_scratch(self, output):
        self.assertFalse(any(path.is_dir() for path in output.glob('ipquality-profile-replay-*')))

    def test_replay_authenticates_real_parent_and_exact_binding_without_approval(self):
        before = self.fixture.snapshot()
        count = self.solver.call_count
        proof, output, deadline = self.replay()
        self.assertEqual(self.solver.call_count, count + 1)
        self.assertEqual(proof['materials'], self.fixture.child())
        self.assertLess(len(proof['materials']['packages']), len(self.fixture.materials['packages']))
        for key in self.profile_module.FALSE_FLAGS:
            self.assertIs(proof[key], False)
        self.assertEqual(proof['binding_inputs_lock_sha256'],
                         self.identity((self.bound / 'inputs-lock.json').read_bytes())['sha256'])
        self.assertEqual(self.fixture.snapshot(), before)
        self.assertIsNotNone(deadline.capacity.last_memory)
        self.assert_no_solver_scratch(output)

    def test_public_proof_redacts_private_paths_tool_identities_and_namespace_inodes(self):
        proof, _, _ = self.replay()
        content = self.build.canonical(proof).decode('ascii')
        for private in (str(self.root), 'cache_directory', 'derived_inputs', 'derived_binding',
                        'host_namespace_inode', 'solver_namespace_inode', 'memory_observation',
                        'requested_path', 'resolved_path', 'dpkg_report', 'commands.json'):
            self.assertNotIn(private, content)
        self.assertEqual(set(proof['selection']), {'schema', 'seeds', 'main_essential_names',
            'package_selection', 'installation', 'binary_downloads_by_solver', 'network_namespaces'})
        for namespace in proof['selection']['network_namespaces']:
            self.assertEqual(set(namespace), {'schema', 'operation', 'different_namespace',
                                             'interfaces', 'installation'})

    def test_public_inventory_rejects_extra_missing_and_changed_package_rows(self):
        proof, _, _ = self.replay()
        extra = next(copy.deepcopy(row) for row in self.fixture.materials['packages']
                     if row['name'] == 'fixture-extra')
        changed_rows = [proof['materials']['packages'] + [extra],
                        proof['materials']['packages'][1:],
                        [dict(proof['materials']['packages'][0], version='2.0'),
                         *proof['materials']['packages'][1:]]]
        for packages in changed_rows:
            with self.subTest(packages=[row['name'] for row in packages]):
                changed = copy.deepcopy(proof)
                changed['materials']['packages'] = packages
                changed['materials_sha256'] = self.identity(self.fixtures.encoded(changed['materials']))['sha256']
                with self.assertRaises(ValueError):
                    self.profile.validate_public(changed, self.lock)

    def test_public_corresponding_sources_are_exact_not_just_matching_package_names(self):
        proof, _, _ = self.replay()
        extra = next(copy.deepcopy(row) for row in self.fixture.materials['sources']
                     if row['name'] == 'fixture-hardware')
        changed_sources = [proof['materials']['sources'] + [extra], [],
                           [dict(proof['materials']['sources'][0], version='2.0')]]
        for sources in changed_sources:
            with self.subTest(sources=sources):
                changed = copy.deepcopy(proof)
                changed['materials']['sources'] = sources
                changed['materials_sha256'] = self.identity(self.fixtures.encoded(changed['materials']))['sha256']
                with self.assertRaises(ValueError):
                    self.profile.validate_public(changed, self.lock)

    def test_public_selection_rejects_private_fields_even_after_rehash(self):
        proof, _, _ = self.replay()
        for place, key, value in (('selection', 'commands', [['/private/fixture/tool']]),
                                  ('namespace', 'host_namespace_inode', 999)):
            with self.subTest(place=place):
                changed = copy.deepcopy(proof)
                selected = changed['selection'] if place == 'selection' else changed['selection']['network_namespaces'][0]
                selected[key] = value
                changed['selection_sha256'] = self.identity(self.fixtures.encoded(changed['selection']))['sha256']
                with self.assertRaises(ValueError):
                    self.profile.validate_public(changed, self.lock)

    def test_public_profile_code_and_nonapproval_flags_cannot_be_self_redeclared(self):
        proof, _, _ = self.replay()
        for key in self.profile_module.FALSE_FLAGS:
            changed = copy.deepcopy(proof)
            changed[key] = True
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.profile.validate_public(changed, self.lock)
        changed = copy.deepcopy(proof)
        changed['implementations']['tools/ipquality-profile.py'] = '0' * 64
        with self.assertRaisesRegex(ValueError, 'implementation'):
            self.profile.validate_public(changed, self.lock)
        changed = copy.deepcopy(proof)
        changed['commands']['curl'] = 'fixture-extra'
        with self.assertRaises(ValueError):
            self.profile.validate_public(changed, self.lock)

    def test_fresh_replay_rejects_extra_or_missing_selection_even_inside_parent_inventory(self):
        for label in ('extra', 'missing'):
            self.selected_override = copy.deepcopy(self.fixture.child()['packages'])
            if label == 'extra':
                self.selected_override.append(next(copy.deepcopy(row) for row in self.fixture.materials['packages']
                                                   if row['name'] == 'fixture-extra'))
            else:
                self.selected_override = [row for row in self.selected_override if row['name'] != 'bash']
            output, deadline = self.admission('replay-' + label)
            with self.subTest(label=label), self.assertRaises(ValueError):
                self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
            self.assert_no_solver_scratch(output)
        self.selected_override = None

    def test_actual_parent_payload_change_is_rejected_before_fresh_selection(self):
        path = self.fixture.cache / self.fixture.materials['packages'][0]['blob']
        path.write_bytes(path.read_bytes() + b'TEST_ONLY changed authenticated body')
        after_change = self.fixture.snapshot()
        count = self.solver.call_count
        output, deadline = self.admission('changed-parent')
        with self.assertRaises(ValueError):
            self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        self.assertEqual(self.solver.call_count, count)
        self.assertEqual(self.fixture.snapshot(), after_change)
        self.assert_no_solver_scratch(output)

    def test_binding_receipt_false_flags_and_parent_identity_are_reauthenticated(self):
        path = self.bound / 'binding.json'
        original = path.read_bytes()
        for changes in ({'builder_approved': True}, {'parent_collection_sha256': '0' * 64},
                        {'inputs_lock_sha256': '0' * 64}, {'payload_downloads': True}):
            changed = json.loads(original)
            changed.update(changes)
            path.write_bytes(self.fixtures.encoded(changed))
            output, deadline = self.admission('binding-' + next(iter(changes)))
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
            self.assert_no_solver_scratch(output)
        path.write_bytes(original)

    def test_binding_tools_and_signed_index_bounds_require_actual_matching_bytes(self):
        path = self.bound / 'expanded-indices.json'
        original = path.read_bytes()
        changed = json.loads(original)
        next(iter(changed['main'].values()))['size'] += 1
        path.write_bytes(self.fixtures.encoded(changed))
        output, deadline = self.admission('changed-bound-index')
        with self.assertRaises(ValueError):
            self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        self.assert_no_solver_scratch(output)
        path.write_bytes(original)
        candidate_tool = next(iter(self.candidate_files.values()))
        candidate_tool.write_bytes(b'X' * candidate_tool.stat().st_size)
        output, deadline = self.admission('changed-candidate')
        with self.assertRaisesRegex(ValueError, 'candidate tool bytes'):
            self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        self.assert_no_solver_scratch(output)

    def test_derived_profile_or_code_drift_is_not_repaired_by_rehashing_sidecars(self):
        for label in ('profile.json', 'code-ledger.json'):
            path = self.derived / label
            original = path.read_bytes()
            receipt_original = (self.derived / 'derivation.json').read_bytes()
            changed = json.loads(original)
            if label == 'profile.json':
                changed['commands']['curl'] = 'fixture-extra'
            else:
                changed[0]['identity']['sha256'] = '0' * 64
            self.change_derived(label, changed)
            output, deadline = self.admission('changed-' + label)
            with self.subTest(label=label), self.assertRaises(ValueError):
                self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
            self.assert_no_solver_scratch(output)
            path.write_bytes(original)
            (self.derived / 'derivation.json').write_bytes(receipt_original)

    def test_binding_and_parent_changes_during_solver_are_caught_at_final_admission(self):
        before = self.fixture.snapshot()
        binding_path = self.bound / 'binding.json'
        original = binding_path.read_bytes()
        def changed_binding(*args, **kwargs):
            result = self.solve_fixture(*args, **kwargs)
            binding_path.write_bytes(original + b' ')
            return result
        self.solver.side_effect = changed_binding
        output, deadline = self.admission('binding-window')
        with self.assertRaisesRegex(ValueError, 'binding evidence'):
            self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        self.assert_no_solver_scratch(output)
        self.assertEqual(self.fixture.snapshot(), before)
        binding_path.write_bytes(original)
        payload = self.fixture.cache / self.fixture.materials['packages'][0]['blob']
        def changed_parent(*args, **kwargs):
            result = self.solve_fixture(*args, **kwargs)
            payload.write_bytes(payload.read_bytes() + b'TEST_ONLY window mutation')
            return result
        self.solver.side_effect = changed_parent
        output, deadline = self.admission('parent-window')
        with self.assertRaises(ValueError):
            self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        self.assert_no_solver_scratch(output)

    def test_replay_requires_an_owned_guard_and_rejects_input_overlap(self):
        deadline = self.build.Deadline(60)
        with self.assertRaisesRegex(ValueError, 'owned output guard'):
            self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        before = self.fixture.snapshot()
        plan = self.inputs.output_plan(self.build, self.root, 64 * 1024**2, self.inputs.RESERVE)
        deadline.capacity = self.build.FactoryCapacity(self.fixture.directory, plan, deadline)
        # Admission overlap must reject before touching or deleting original material.
        with self.assertRaisesRegex(ValueError, 'overlaps original inputs'):
            self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_plain_ip_prepare_cannot_bypass_explicit_derived_binding(self):
        self.profile.prepare_context = None
        count = self.solver.call_count
        output = self.root / 'plain-prepare'
        with self.assertRaisesRegex(ValueError, 'derived inputs and binding'):
            self.build.prepare(self.bound / 'inputs-lock.json', self.fixture.cache, output,
                               self.lock['builder']['image_sha256'])
        self.assertFalse(output.exists())
        self.assertEqual(self.solver.call_count, count)

    def test_prepare_rejects_original_parent_or_cache_overlap_before_creating_output(self):
        self.profile.prepare_context = self.context
        before = self.fixture.snapshot()
        original = self.build.reserve_output
        with patch.object(self.build, 'reserve_output', wraps=original) as reserve:
            for parent in (self.fixture.directory, self.fixture.cache):
                output = parent / 'TEST_ONLY-forbidden-prepare'
                with self.subTest(parent=parent), self.assertRaisesRegex(ValueError, 'overlaps original inputs'):
                    self.build.prepare(self.bound / 'inputs-lock.json', self.fixture.cache, output,
                                       self.lock['builder']['image_sha256'])
                self.assertFalse(output.exists())
            reserve.assert_not_called()
        self.assertEqual(self.fixture.snapshot(), before)

    def test_prepare_replays_exact_profile_and_verify_uses_private_saved_context(self):
        before = self.fixture.snapshot()
        count = self.solver.call_count
        prepared, receipt = self.prepare()
        public = (prepared / self.profile_module.PUBLIC_NAME).read_bytes()
        private = json.loads((prepared / self.profile_module.PRIVATE_NAME).read_bytes())
        self.assertEqual(receipt['profile_proof_sha256'], self.identity(public)['sha256'])
        self.assertEqual(private['profile_proof_sha256'], self.identity(public)['sha256'])
        self.assertEqual(private['cache_directory'], str(self.fixture.cache))
        self.assertEqual(self.solver.call_count, count + 1)
        self.profile.prepare_context = {'derived_inputs': '/TEST_ONLY unused context',
                                        'derived_binding': '/TEST_ONLY unused binding'}
        verified, replay_output, _ = self.verify_prepared(prepared)
        self.assertEqual(verified['profile_proof_bytes'], public)
        self.assertEqual(verified['profile_proof_sha256'], receipt['profile_proof_sha256'])
        self.assertEqual(self.solver.call_count, count + 2)
        self.assertEqual(self.fixture.snapshot(), before)
        self.assert_no_solver_scratch(replay_output)
        for row in self.fixture.child()['packages']:
            self.assertEqual((prepared / 'input-cache' / row['blob']).read_bytes(),
                             (self.fixture.cache / row['blob']).read_bytes())

    def test_verify_replays_again_and_missing_public_or_private_proof_is_refused(self):
        prepared, _ = self.prepare()
        for name in (self.profile_module.PUBLIC_NAME, self.profile_module.PRIVATE_NAME):
            path = prepared / name
            original = path.read_bytes()
            path.unlink()
            _, deadline = self.admission('missing-' + name)
            with self.subTest(name=name), self.assertRaises((ValueError, OSError)):
                self.build.verify_prepared(prepared, self.lock['builder']['image_sha256'], _deadline=deadline)
            path.write_bytes(original)

    def test_standalone_verify_owns_separate_scratch_and_keeps_prepared_inputs_unchanged(self):
        prepared, receipt = self.prepare()
        before_parent = self.fixture.snapshot()
        before_prepared = {path.relative_to(prepared).as_posix():
            self.inputs.snapshot_file(path, self.build.MAX_ARCHIVE, self.build.Deadline(60))
            for path in prepared.rglob('*') if path.is_file()}
        before_siblings = {path.name for path in self.root.iterdir()}
        count = self.solver.call_count
        verified = self.build.verify_prepared(prepared, self.lock['builder']['image_sha256'])
        self.assertEqual(verified['profile_proof_sha256'], receipt['profile_proof_sha256'])
        self.assertEqual(self.solver.call_count, count + 1)
        self.assertEqual({path.name for path in self.root.iterdir()}, before_siblings)
        self.assertEqual(self.fixture.snapshot(), before_parent)
        after_prepared = {path.relative_to(prepared).as_posix():
            self.inputs.snapshot_file(path, self.build.MAX_ARCHIVE, self.build.Deadline(60))
            for path in prepared.rglob('*') if path.is_file()}
        self.assertEqual(after_prepared, before_prepared)

    def test_standalone_verify_failure_keeps_bounded_failure_and_actual_cleanup_confirmation(self):
        prepared, _ = self.prepare()
        before = self.fixture.snapshot()
        public = (prepared / self.profile_module.PUBLIC_NAME).read_bytes()
        self.solver.side_effect = ValueError('TEST_ONLY native selection refusal')
        with self.assertRaisesRegex(ValueError, 'native selection refusal'):
            self.build.verify_prepared(prepared, self.lock['builder']['image_sha256'])
        failures = list(self.root.glob('ipquality-profile-verification-*-failure-*/failure.json'))
        self.assertEqual(len(failures), 1)
        failure = json.loads(failures[0].read_bytes())
        self.assertIn('native selection refusal', failure['error'])
        self.assertLessEqual(failures[0].stat().st_size, self.build.MAX_METADATA)
        self.assertFalse(Path(failure['output_directory']).exists())
        cleanup = json.loads((failures[0].parent / 'cleanup.json').read_bytes())
        self.assertIs(cleanup['removed'], True)
        self.assertIs(cleanup['retained'], False)
        self.assertEqual((prepared / self.profile_module.PUBLIC_NAME).read_bytes(), public)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_private_context_swapping_bindings_cannot_reuse_an_old_public_proof(self):
        prepared, _ = self.prepare()
        another = self.root / 'another-derived'
        another_bound = self.root / 'another-bound'
        self.inputs.derive(self.fixture.directory, another, 64 * 1024**2, 60)
        self.inputs.bind(another, self.candidate_path, another_bound, 60,
                         max_output_bytes=64 * 1024**2)
        path = prepared / self.profile_module.PRIVATE_NAME
        private = json.loads(path.read_bytes())
        private['context']['derived_binding'] = str(another_bound)
        path.write_bytes(self.fixtures.encoded(private))
        _, deadline = self.admission('wrong-private-context')
        with self.assertRaisesRegex(ValueError, 'binding receipt'):
            self.profile.verify(prepared, self.lock, deadline)

    def test_rehashed_public_inventory_tamper_cannot_pass_prepared_verification(self):
        prepared, _ = self.prepare()
        public_path = prepared / self.profile_module.PUBLIC_NAME
        proof = json.loads(public_path.read_bytes())
        proof['materials']['packages'][0]['version'] = '2.0'
        proof['materials_sha256'] = self.identity(self.fixtures.encoded(proof['materials']))['sha256']
        public = self.fixtures.encoded(proof)
        public_path.write_bytes(public)
        private_path = prepared / self.profile_module.PRIVATE_NAME
        private = json.loads(private_path.read_bytes())
        private['profile_proof_sha256'] = self.identity(public)['sha256']
        private_path.write_bytes(self.fixtures.encoded(private))
        _, deadline = self.admission('rehashed-proof')
        with self.assertRaisesRegex(ValueError, 'exact package/source'):
            self.profile.verify(prepared, self.lock, deadline)

    def test_private_evidence_mutation_during_verify_is_detected_after_replay(self):
        prepared, _ = self.prepare()
        private_path = prepared / self.profile_module.PRIVATE_NAME
        original = private_path.read_bytes()
        def changed_private(*args, **kwargs):
            result = self.solve_fixture(*args, **kwargs)
            private_path.write_bytes(original + b' ')
            return result
        self.solver.side_effect = changed_private
        _, deadline = self.admission('private-window')
        with self.assertRaisesRegex(ValueError, 'evidence changed'):
            self.profile.verify(prepared, self.lock, deadline)

    def test_solver_interrupt_cleans_replay_scratch_and_preserves_parent_and_binding(self):
        before = self.fixture.snapshot()
        binding = (self.bound / 'binding.json').read_bytes()
        self.solver.side_effect = KeyboardInterrupt('TEST_ONLY interrupted native APT boundary')
        output, deadline = self.admission('interrupted-replay')
        with self.assertRaises(KeyboardInterrupt):
            self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        self.assert_no_solver_scratch(output)
        self.assertEqual(self.fixture.snapshot(), before)
        self.assertEqual((self.bound / 'binding.json').read_bytes(), binding)

    def test_deferred_interrupt_after_scratch_creation_still_cleans_registered_ownership(self):
        original = self.build.deferred_signals
        calls = {'count': 0}
        @contextlib.contextmanager
        def deliver_once():
            calls['count'] += 1
            with original():
                yield
            if calls['count'] == 1:
                raise KeyboardInterrupt('TEST_ONLY deferred signal after registration')
        before = self.fixture.snapshot()
        with patch.object(self.build, 'deferred_signals', deliver_once):
            with self.assertRaisesRegex(KeyboardInterrupt, 'after registration'):
                with self.profile.owned_scratch(self.root, 'deferred-owned-'):
                    self.fail('deferred interruption must arrive before the scratch body')
        self.assertFalse(list(self.root.glob('deferred-owned-*')))
        self.assertEqual(self.profile.scratch_ownership, {})
        self.assertEqual(self.fixture.snapshot(), before)

    def test_replaced_solver_scratch_is_retained_and_never_deleted_as_owned(self):
        before = self.fixture.snapshot()
        moved = self.root / 'moved-solver-scratch'
        replacement = {}
        def replace_scratch(collector, *args, **kwargs):
            result = self.solve_fixture(collector, *args, **kwargs)
            original = collector.output
            original.rename(moved)
            original.mkdir(mode=0o700)
            marker = original / 'foreign-material'
            marker.write_bytes(b'TEST_ONLY unrelated replacement; retain it')
            replacement['marker'] = marker
            return result
        self.solver.side_effect = replace_scratch
        _, deadline = self.admission('scratch-attack')
        with self.assertRaisesRegex(ValueError, 'identity|changed|cleanup'):
            self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        self.assertEqual(replacement['marker'].read_bytes(), b'TEST_ONLY unrelated replacement; retain it')
        self.assertTrue((moved / 'solver/selection.json').is_file())
        self.assertEqual(self.fixture.snapshot(), before)

    def test_observed_mount_under_replay_scratch_blocks_deletion_and_success(self):
        before = self.fixture.snapshot()
        mounted = {}
        def observed_mount(path):
            if Path(path) == mounted.get('scratch'):
                raise ValueError('TEST_ONLY remaining mount; retain owned scratch')
        def mark_mount(collector, *args, **kwargs):
            result = self.solve_fixture(collector, *args, **kwargs)
            mounted['scratch'] = collector.output
            return result
        self.solver.side_effect = mark_mount
        _, deadline = self.admission('mount-attack')
        with patch.object(self.build, 'ensure_no_mounts', side_effect=observed_mount):
            with self.assertRaisesRegex(ValueError, 'remaining mount'):
                self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        self.assertTrue((mounted['scratch'] / 'solver/selection.json').is_file())
        self.assertIn(mounted['scratch'], self.profile.scratch_ownership)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_oom_counter_increment_stops_replay_even_when_host_memory_is_sufficient(self):
        before = self.fixture.snapshot()
        pressure = self.memory()
        pressure['cgroup_observations'][0]['memory_events']['oom_kill'] += 1
        def observed_oom(collector, *args, **kwargs):
            result = self.solve_fixture(collector, *args, **kwargs)
            self.memory_observation.return_value = pressure
            collector.guard.check(force=True)
            return result
        self.solver.side_effect = observed_oom
        output, deadline = self.admission('oom-refusal')
        with self.assertRaisesRegex(ValueError, 'OOM counter'):
            self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
        self.assert_no_solver_scratch(output)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_dynamic_disk_or_inode_shortfall_stops_replay_without_weakening_reserve(self):
        before = self.fixture.snapshot()
        for label, space in (('disk', self.space(f_bavail=(self.inputs.RESERVE - 4096) // 4096)),
                             ('inodes', self.space(f_favail=self.inputs.INODE_RESERVE - 1))):
            def change_capacity(collector, *args, **kwargs):
                result = self.solve_fixture(collector, *args, **kwargs)
                self.disk.return_value = space
                collector.guard.check(force=True)
                return result
            self.disk.return_value = self.space()
            self.solver.side_effect = change_capacity
            output, deadline = self.admission('capacity-' + label)
            with self.subTest(label=label), self.assertRaisesRegex(ValueError, 'reserve crossed'):
                self.profile.replay(self.context, self.lock, self.fixture.cache, deadline)
            self.assert_no_solver_scratch(output)
            self.assertEqual(self.fixture.snapshot(), before)
        self.disk.return_value = self.space()

    def test_memory_failure_during_prepare_retains_observation_and_cleans_only_new_output(self):
        before = self.fixture.snapshot()
        low = self.memory(128 * 1024**2)
        def memory_drop(collector, *args, **kwargs):
            result = self.solve_fixture(collector, *args, **kwargs)
            self.memory_observation.return_value = low
            collector.guard.check(force=True)
            return result
        self.solver.side_effect = memory_drop
        output = self.root / 'memory-refusal'
        self.profile.prepare_context = self.context
        with self.assertRaisesRegex(ValueError, '256 MiB'):
            self.build.prepare(self.bound / 'inputs-lock.json', self.fixture.cache, output,
                               self.lock['builder']['image_sha256'])
        self.assertFalse(output.exists())
        failure_paths = list(self.root.glob('memory-refusal-failure-*/failure.json'))
        self.assertEqual(len(failure_paths), 1)
        failure = json.loads(failure_paths[0].read_bytes())
        self.assertEqual(failure['capacity']['memory_observation']['available_bytes'], low['available_bytes'])
        cleanup = json.loads((failure_paths[0].parent / 'cleanup.json').read_bytes())
        self.assertIs(cleanup['removed'], True)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_replaced_factory_output_is_not_deleted_when_profile_admission_fails(self):
        before = self.fixture.snapshot()
        output, moved = self.root / 'prepare-attack', self.root / 'moved-preparation'
        def replace_output(collector, *args, **kwargs):
            output.rename(moved)
            output.mkdir(mode=0o700)
            (output / 'foreign-material').write_bytes(b'TEST_ONLY unrelated factory replacement')
            collector.guard.check(force=True)
            raise AssertionError('changed owned output was not rejected')
        self.solver.side_effect = replace_output
        self.profile.prepare_context = self.context
        with self.assertRaisesRegex(ValueError, 'identity'):
            self.build.prepare(self.bound / 'inputs-lock.json', self.fixture.cache, output,
                               self.lock['builder']['image_sha256'])
        self.assertEqual((output / 'foreign-material').read_bytes(), b'TEST_ONLY unrelated factory replacement')
        self.assertTrue(moved.is_dir())
        self.assertEqual(self.fixture.snapshot(), before)
        cleanup_paths = list(self.root.glob('prepare-attack-failure-*/cleanup.json'))
        self.assertEqual(len(cleanup_paths), 1)
        cleanup = json.loads(cleanup_paths[0].read_bytes())
        self.assertIs(cleanup['removed'], False)
        self.assertIs(cleanup['retained'], True)


if __name__ == '__main__':
    unittest.main()
