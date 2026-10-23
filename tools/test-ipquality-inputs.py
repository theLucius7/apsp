#!/usr/bin/env python3
"""Cache-derivation fixtures with real bytes and identities, no node certification.

Native GPG/APT/tool ownership boundaries are explicitly isolated when necessary;
these fixtures do not claim official signatures, host approval or live isolation.
"""
import contextlib
import copy
import gzip
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]


def module(name, path):
    specification = importlib.util.spec_from_file_location(name, path)
    loaded = importlib.util.module_from_spec(specification)
    specification.loader.exec_module(loaded)
    return loaded


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=True,
                      allow_nan=False).encode() + b'\n'


def identity(content):
    return {'size': len(content), 'sha256': hashlib.sha256(content).hexdigest()}


class ParentFixture:
    """Ordinary single-link bytes, valid material closure and marked inert records."""
    def __init__(self, directory, parent_factory, profile):
        self.directory, self.cache = directory, directory / 'input-cache'
        self.directory.mkdir(mode=0o700)
        self.cache.mkdir(mode=0o700)
        self.parent_factory, self.profile = parent_factory, profile
        self.releases = {}
        self.materials = self.make_materials()
        self.write_receipt()

    def blob(self, name, content):
        path = self.cache / name
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        path.write_bytes(content)
        os.chmod(path, 0o644)
        return dict(identity(content), blob=name)

    def source(self, name):
        rows = []
        for suffix in ('.dsc', '.orig.tar.xz'):
            filename = name + '_1.0' + suffix
            content = ('TEST_ONLY actual corresponding-source fixture: ' + filename).encode()
            rows.append(dict(name=filename, **self.blob(identity(content)['sha256'] + '.source', content)))
        return {'repository': 'main', 'name': name, 'version': '1.0',
                'directory': 'pool/main/f/' + name, 'files': rows}

    def make_materials(self):
        packages = []
        selected = set(self.profile.TOOLS.values()) | {'apt', 'base-files'}
        names = sorted(set(self.parent_factory.TOOL_PACKAGES.values()) | selected | {'fixture-extra'})
        for name in names:
            body = ('TEST_ONLY actual deb fixture: ' + name).encode()
            arch = 'all' if name == 'ca-certificates' else 'amd64'
            source_name = 'fixture-minimal' if name in selected else 'fixture-hardware'
            row = {'repository': 'main', 'name': name, 'version': '1.0', 'architecture': arch,
                   'filename': 'pool/main/f/' + source_name + '/' + name + '_1.0_' + arch + '.deb',
                   'source_name': source_name, 'source_version': '1.0',
                   **self.blob(identity(body)['sha256'] + '.deb', body)}
            packages.append(row)
        sources = [self.source(name) for name in ('fixture-minimal', 'fixture-hardware')]
        package_text = ''.join('Package: ' + row['name'] + '\nVersion: ' + row['version']
                              + '\nArchitecture: ' + row['architecture']
                              + ('\nEssential: yes' if row['name'] == 'base-files' else '')
                              + '\nSource: ' + row['source_name'] + ' (' + row['source_version'] + ')'
                              + '\nFilename: ' + row['filename'] + '\nSize: ' + str(row['size'])
                              + '\nSHA256: ' + row['sha256'] + '\n\n' for row in packages)
        source_text = ''.join('Package: ' + row['name'] + '\nVersion: ' + row['version']
                             + '\nDirectory: ' + row['directory'] + '\nChecksums-Sha256:\n'
                             + ''.join(' ' + item['sha256'] + ' ' + str(item['size']) + ' '
                                       + item['name'] + '\n' for item in row['files']) + '\n'
                             for row in sources)
        repositories = []
        for name, archive, timestamp, suite in (
            ('main', 'debian', '20261001T142720Z', 'bookworm'),
            ('security', 'debian-security', '20261001T142623Z', 'bookworm-security'),
        ):
            indices = []
            checksums = []
            for kind, path, text in (('Packages', 'main/binary-amd64/Packages.gz', package_text),
                                     ('Sources', 'main/source/Sources.gz', source_text)):
                raw = text if name == 'main' else ('Package: unrelated-security\nVersion: 1.0\n'
                      'Architecture: amd64\nFilename: pool/updates/main/u/unrelated/unrelated_1.0_amd64.deb\n'
                      'Size: 1\nSHA256: ' + identity(b'x')['sha256'] + '\n\n' if kind == 'Packages' else '')
                body = gzip.compress(raw.encode(), mtime=0)
                indices.append(dict(kind=kind, path=path, **self.blob(name + '-' + kind + '.gz', body)))
                checksums.append(dict(path=path, **identity(body)))
                checksums.append(dict(path=path[:-3], **identity(raw.encode())))
            inrelease = self.blob(name + '.InRelease', ('TEST_ONLY isolated GPG boundary: ' + name).encode())
            self.releases[str(self.cache / inrelease['blob'])] = (
                'Origin: Debian\nCodename: ' + suite + '\nSHA256:\n'
                + ''.join(' ' + row['sha256'] + ' ' + str(row['size']) + ' ' + row['path'] + '\n'
                          for row in checksums)).encode()
            repositories.append({'id': name, 'archive': archive, 'timestamp': timestamp, 'suite': suite,
                                 'indices': indices,
                                 'inrelease': inrelease})
        return {'schema': 1, 'arch': 'amd64', 'source_epoch': 1790864840,
                'keyring': self.blob('bookworm-keyring.gpg', b'TEST_ONLY isolated reviewed keyring'),
                'repositories': repositories, 'packages': packages, 'sources': sources}

    def child(self):
        materials = copy.deepcopy(self.materials)
        selected = set(self.profile.TOOLS.values()) | {'apt', 'base-files'}
        materials['packages'] = [row for row in materials['packages'] if row['name'] in selected]
        materials['sources'] = [row for row in materials['sources'] if row['name'] == 'fixture-minimal']
        return materials

    def write_receipt(self):
        raw = encoded(self.materials)
        (self.directory / 'materials.json').write_bytes(raw)
        requested = {'schema': 1, 'arch': self.materials['arch'], 'source_epoch': self.materials['source_epoch'],
                     'repositories': [{key: row[key] for key in ('id', 'archive', 'timestamp', 'suite')}
                                      for row in self.materials['repositories']]}
        (self.directory / 'request.json').write_bytes(encoded(requested))
        (self.directory / 'keyring-provenance.json').write_bytes(encoded({
            'schema': 1, 'source': 'TEST_ONLY owned keyring fixture', 'reviewer': 'TEST_ONLY declaration',
            'obtained_at': '2026-10-01T00:00:00+00:00',
            **{key: self.materials['keyring'][key] for key in ('size', 'sha256')}}))
        solver = self.directory / 'solver'
        solver.mkdir(mode=0o700)
        solver_body = encoded({'schema': 1,
                               'package_selection': [{key: value for key, value in row.items() if key != 'blob'}
                                                     for row in self.materials['packages']],
                               'installation': False, 'binary_downloads_by_solver': False,
                               'evidence_scope': 'TEST_ONLY inert parent solver record'})
        (solver / 'selection.json').write_bytes(solver_body)
        capacity_body = encoded({'schema': 1,
                                 'kind': 'apsp-nodequality-debian-input-collection-capacity-plan',
                                 'arch': self.materials['arch'], 'source_epoch': self.materials['source_epoch'],
                                 'keyring': self.materials['keyring'], 'repositories': self.materials['repositories'],
                                 'packages': self.materials['packages'], 'sources': self.materials['sources'],
                                 'complete': False, 'payload_authenticated': False, 'builder_approved': False,
                                 'full_ready': False, 'reproducibility_verified': False,
                                 'stage': 'before_binary_and_source_payload_download',
                                 'evidence_scope': 'TEST_ONLY parent inventory binding; no live capacity admission'})
        (self.directory / 'capacity-plan.json').write_bytes(capacity_body)
        (self.directory / 'unbound-inputs.json').write_bytes(encoded({
            'schema': 1, 'kind': 'apsp-nodequality-debian-input-collection',
            'materials_sha256': identity(raw)['sha256'], 'builder': None, 'lock_ready': False,
            'builder_approved': False, 'full_ready': False, 'reproducibility_verified': False}))
        imports = []
        for repository in self.materials['repositories']:
            archive, selected = repository['archive'], repository['timestamp']
            response_body = encoded({'result': {archive: [selected]}})
            descriptor = self.blob(archive + '-imports.json', response_body)
            imports.append({'archive': archive, 'selected_timestamp': selected, 'response': descriptor,
                            'imports_in_response': [selected],
                            'authentication_scope': 'official HTTPS discovery, not package signature'})
        self.receipt = {'schema': 1, 'kind': 'apsp-nodequality-debian-input-collection',
                        'complete': True, 'arch': 'amd64', 'collected_at': '2026-10-02T00:00:00+00:00',
                        'materials_sha256': identity(raw)['sha256'], 'inputs_lock_sha256': None,
                        'lock_ready': False, 'builder': None, 'imports': imports,
                        'builder_approved': False, 'runtime_image_identity_verified': False,
                        'keyring_acquisition_review_supplied': True,
                        'keyring_trust_independently_verified_by_collector': False,
                        'source_authenticated': True, 'full_ready': False, 'reproducibility_verified': False,
                        'solver_selection_sha256': identity(solver_body)['sha256'],
                        'capacity_plan': dict(path='capacity-plan.json', **identity(capacity_body)),
                        'collector_sha256': identity((ROOT / 'tools/nodequality-rootfs-collect.py').read_bytes())['sha256'],
                        'verification_helper_sha256': identity((ROOT / 'tools/nodequality-rootfs-build.py').read_bytes())['sha256'],
                        'candidate_builder_image_sha256': None,
                        'signatures': [{'repository': row['id'],
                                        'primary_fingerprints': [sorted(self.parent_factory.SIGNERS[row['archive']])[0]],
                                        'inrelease_sha256': row['inrelease']['sha256']}
                                       for row in self.materials['repositories']],
                        'downloads': [],
                        'collection_tools': {'executables': [], 'dpkg_report': 'TEST_ONLY no host approval',
                                             'actual_version_output': {},
                                             'evidence_scope': 'TEST_ONLY inert parent tool record'},
                        'authentication_scope': 'Debian inputs under supplied independently reviewed keyring',
                        'max_total_bytes': 4 * 1024**3, 'overall_timeout_seconds': 60,
                        'reserve_free_bytes': 512 * 1024**2}
        self.save_receipt()

    def save_receipt(self):
        (self.directory / 'collection.json').write_bytes(encoded(self.receipt))

    def snapshot(self):
        return {path.relative_to(self.directory).as_posix():
                (identity(path.read_bytes()), path.stat().st_dev, path.stat().st_ino,
                 path.stat().st_nlink, path.stat().st_mode, path.stat().st_mtime_ns,
                 path.stat().st_ctime_ns)
                for path in self.directory.rglob('*') if path.is_file() and not path.is_symlink()}

    def gpgv(self, arguments, deadline, output_limit, stderr=None, **kwargs):
        if arguments[0] != '/usr/bin/gpgv':
            raise AssertionError('TEST_ONLY gpgv isolation must not run another command')
        source = Path(arguments[-1])
        repository = next(row for row in self.materials['repositories']
                          if Path(row['inrelease']['blob']).name == source.name)
        Path(arguments[arguments.index('--output') + 1]).write_bytes(self.releases[str(self.cache / source.name)])
        fingerprint = sorted(self.parent_factory.SIGNERS[repository['archive']])[0]
        return ('[GNUPG:] VALIDSIG ' + fingerprint
                + ' 2023-11-14 1700000000 0 4 0 1 8 01 ' + fingerprint + '\n').encode()


class DerivedInputsTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # Loaded only when the frozen test suite is executed by the final owner.
        cls.inputs = module('apsp_ipquality_inputs_fixture', ROOT / 'tools/ipquality-inputs.py')
        cls.loaded = cls.inputs.modules()
        cls.parent_factory = cls.loaded['parent_build']
        cls.profile = cls.loaded['profile']

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='apsp-derived-inputs-fixture-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.fixture = ParentFixture(self.root / 'parent', self.parent_factory, self.profile)
        self.isolation = contextlib.ExitStack()
        self.addCleanup(self.isolation.close)
        self.isolation.enter_context(patch.object(self.inputs, 'modules', return_value=self.loaded))
        self.isolation.enter_context(patch.object(self.loaded['collect'], 'native_arch', return_value='amd64'))
        for factory in (self.loaded['parent_build'], self.loaded['build'], self.loaded['capacity'].BUILD):
            self.isolation.enter_context(patch.object(factory, 'run_bounded', side_effect=self.fixture.gpgv))
        self.isolation.enter_context(patch.object(self.loaded['build'], 'ensure_no_mounts'))
        self.memory_observation = self.isolation.enter_context(patch.object(
            self.loaded['capacity'], 'available_memory', return_value=self.memory()))
        self.disk = self.isolation.enter_context(patch.object(self.inputs.os, 'statvfs', return_value=self.space()))
        self.tool_path = self.root / 'owned-native-tool'
        self.tool_path.write_bytes(b'TEST_ONLY isolated native tool bytes')
        self.tools = {'executables': [dict(requested_path=str(self.tool_path), resolved_path=str(self.tool_path),
                                          dpkg_ownership='TEST_ONLY local ownership declaration',
                                          **identity(self.tool_path.read_bytes()))],
                      'dpkg_report': 'TEST_ONLY no native tool or builder approval',
                      'actual_version_output': {str(self.tool_path): 'TEST_ONLY inert version'},
                      'evidence_scope': 'TEST_ONLY isolated tool boundary'}
        self.isolation.enter_context(patch.object(self.loaded['collect'], 'record_collection_tools',
                                                 return_value=self.tools))
        self.solver = self.isolation.enter_context(patch.object(self.loaded['collect'], 'solve',
                                                               side_effect=self.solve_fixture))
        self.selected_override = None

    def memory(self, available=8 * 1024**3):
        return {'available_bytes': available, 'host_available_bytes': available,
                'cgroup_scope': 'TEST_ONLY isolated v2 observation boundary',
                'cgroup_observations': [{'path': '/TEST_ONLY-cgroup', 'limit_bytes': 16 * 1024**3,
                                         'current_bytes': 4 * 1024**3, 'raw_headroom_bytes': 12 * 1024**3,
                                         'inactive_file_bytes': 1024**3, 'slab_reclaimable_bytes': 0,
                                         'reclaim_estimate_headroom_bytes': 13 * 1024**3,
                                         'memory_events': {'low': 0, 'high': 0, 'max': 0,
                                                           'oom': 0, 'oom_kill': 0, 'oom_group_kill': 0}}],
                'cgroup_v2_observed': True, 'cgroup_raw_headroom_bytes': 12 * 1024**3,
                'reclaim_estimate_available_bytes': available, 'reclaim_estimate_is_guaranteed': False}

    def space(self, **changes):
        observed = {'f_frsize': 4096, 'f_bavail': 4 * 1024 * 1024, 'f_favail': 100000}
        observed.update(changes)
        return SimpleNamespace(**observed)

    def solve_fixture(self, collector, repositories, indices, keyring, authenticated_expansion=None):
        # Isolate expensive native APT/namespace commands, keeping actual signed
        # index authentication, expansion and source-closure parsing outside this mock.
        self.assertEqual(collector.cache, self.fixture.cache)
        self.assertEqual(set(authenticated_expansion), {'main', 'security'})
        packages = self.selected_override or self.fixture.child()['packages']
        packages = [{key: value for key, value in row.items() if key != 'blob'} for row in packages]
        seeds, essentials = self.loaded['collect'].essential_seeds(repositories, indices, collector.deadline)
        self.assertEqual(essentials, ['base-files'])
        self.assertIn('base-files', seeds)
        report = {'schema': 1, 'package_selection': packages, 'seeds': seeds,
                  'main_essential_names': essentials, 'installation': False,
                  'network_namespaces': [{'schema': 1, 'operation': operation,
                                          'host_namespace_inode': 101, 'solver_namespace_inode': 202 + index,
                                          'different_namespace': True, 'interfaces': ['lo'],
                                          'installation': False,
                                          'evidence_scope': 'TEST_ONLY isolated native namespace boundary'}
                                         for index, operation in enumerate(('update', 'plan'))],
                  'binary_downloads_by_solver': False, 'evidence_scope': 'TEST_ONLY isolated native APT'}
        body = encoded(report)
        collector.budget(len(body) + 4096)
        collector.guard.check(additional_inodes=1)
        (collector.output / 'solver').mkdir(mode=0o700)
        (collector.output / 'solver/selection.json').write_bytes(body)
        collector.budget()
        return packages, report

    def derive(self, output=None, maximum=32 * 1024**2, seconds=60):
        output = output or self.root / 'derived'
        self.inputs.derive(self.fixture.directory, output, maximum, seconds)
        return output

    def verify(self, output):
        return self.inputs.verify_derivation(output, self.deadline(), loaded=self.loaded)

    def change_sidecar(self, output, name, changed):
        body = encoded(changed)
        (output / name).write_bytes(body)
        receipt = json.loads((output / 'derivation.json').read_bytes())
        receipt['files_sha256'][name] = identity(body)['sha256']
        (output / 'derivation.json').write_bytes(encoded(receipt))

    def deadline(self):
        return self.parent_factory.Deadline(60)

    def parent(self):
        return self.inputs.verify_parent(self.fixture.directory, self.deadline(), loaded=self.loaded)

    def test_snapshot_file_binds_actual_bytes_and_complete_ordinary_identity(self):
        path = self.fixture.cache / self.fixture.materials['keyring']['blob']
        value = self.inputs.snapshot_file(path, 1024, self.deadline())
        self.assertEqual(set(value), {'dev', 'ino', 'size', 'sha256', 'mode', 'uid', 'gid',
                                     'mtime_ns', 'ctime_ns', 'nlink'})
        self.assertEqual({key: value[key] for key in ('size', 'sha256')}, identity(path.read_bytes()))
        self.assertEqual(value['dev'], path.stat().st_dev)
        self.assertEqual(value['ino'], path.stat().st_ino)
        self.assertEqual(value['nlink'], 1)

    def test_snapshot_rejects_symlink_fifo_directory_hardlink_and_over_limit(self):
        original = self.root / 'ordinary'
        original.write_bytes(b'TEST_ONLY ordinary bytes')
        symlink = self.root / 'symlink'
        symlink.symlink_to(original)
        directory = self.root / 'directory'
        directory.mkdir(mode=0o700)
        fifo = self.root / 'fifo'
        os.mkfifo(fifo, 0o600)
        for path in (symlink, directory, fifo):
            with self.subTest(path=path.name), self.assertRaises((ValueError, OSError)):
                self.inputs.snapshot_file(path, 1024, self.deadline())
        link = self.root / 'hardlink'
        os.link(original, link)
        with self.assertRaises(ValueError):
            self.inputs.snapshot_file(original, 1024, self.deadline())
        link.unlink()
        with self.assertRaises(ValueError):
            self.inputs.snapshot_file(original, 1, self.deadline())

    def test_parent_reauthentication_reads_real_bodies_and_leaves_all_old_identity_unchanged(self):
        before = self.fixture.snapshot()
        verified = self.parent()
        self.assertEqual(verified['materials'], self.fixture.materials)
        self.assertEqual(verified['directory'], self.fixture.directory)
        self.assertEqual(verified['cache'], self.fixture.cache)
        self.assertEqual(verified['materials_sha256'], identity((self.fixture.directory / 'materials.json').read_bytes())['sha256'])
        self.assertEqual(verified['receipt_sha256'], identity((self.fixture.directory / 'collection.json').read_bytes())['sha256'])
        self.assertTrue(verified['ledger'])
        self.assertEqual({row['path'] for row in verified['ledger']['files'] if row['scope'] == 'parent'},
                         {'materials.json', 'collection.json', 'request.json', 'keyring-provenance.json',
                          'unbound-inputs.json', 'solver/selection.json', 'capacity-plan.json'})
        self.assertEqual(self.fixture.snapshot(), before)

    def test_parent_unbound_solver_and_capacity_claims_require_actual_matching_bodies(self):
        names = ('unbound-inputs.json', 'solver/selection.json', 'capacity-plan.json')
        originals = {name: (self.fixture.directory / name).read_bytes() for name in names}
        original_receipt = copy.deepcopy(self.fixture.receipt)
        for name in names:
            with self.subTest(name=name):
                changed = json.loads(originals[name])
                if name == 'unbound-inputs.json':
                    changed['full_ready'] = True
                elif name == 'solver/selection.json':
                    changed['package_selection'][0]['blob'] = self.fixture.materials['packages'][0]['blob']
                else:
                    changed['sources'][0]['files'][0]['size'] += 1
                body = encoded(changed)
                (self.fixture.directory / name).write_bytes(body)
                self.fixture.receipt = copy.deepcopy(original_receipt)
                if name == 'solver/selection.json':
                    self.fixture.receipt['solver_selection_sha256'] = identity(body)['sha256']
                elif name == 'capacity-plan.json':
                    self.fixture.receipt['capacity_plan'] = dict(path=name, **identity(body))
                self.fixture.save_receipt()
                with self.assertRaises(ValueError):
                    self.parent()
                (self.fixture.directory / name).write_bytes(originals[name])
        self.fixture.receipt = original_receipt
        self.fixture.save_receipt()

    def test_parent_receipt_kind_arch_complete_material_digest_and_authentication_are_required(self):
        initial = copy.deepcopy(self.fixture.receipt)
        for key, replacement in (('kind', 'apsp-ipquality-debian-input-collection'),
                                 ('arch', 'arm64'), ('complete', False),
                                 ('source_authenticated', False), ('builder_approved', True),
                                 ('materials_sha256', '0' * 64)):
            with self.subTest(key=key):
                self.fixture.receipt = dict(initial, **{key: replacement})
                self.fixture.save_receipt()
                with self.assertRaises(ValueError):
                    self.parent()
        self.fixture.receipt = initial
        self.fixture.save_receipt()
        with patch.object(self.loaded['collect'], 'native_arch', return_value='arm64'):
            with self.assertRaises(ValueError):
                self.parent()
        (self.fixture.directory / 'failure.json').write_bytes(encoded({'complete': False}))
        with self.assertRaises(ValueError):
            self.parent()

    def test_recorded_signature_claim_is_compared_with_actual_reauthentication(self):
        self.fixture.receipt['signatures'] = []
        self.fixture.save_receipt()
        with self.assertRaisesRegex(ValueError, 'signatures'):
            self.parent()

    def test_parent_import_pair_empty_duplicate_extra_or_timestamp_drift_is_rejected(self):
        original = copy.deepcopy(self.fixture.receipt)
        imports = original['imports']
        extra = copy.deepcopy(imports[0])
        extra['archive'] = 'foreign'
        drift = copy.deepcopy(imports)
        drift[0]['selected_timestamp'] = '20261001T000000Z'
        for observations in ([], imports[:1], [imports[0], imports[0]], [*imports, extra], drift):
            with self.subTest(observations=observations):
                self.fixture.receipt = dict(original, imports=copy.deepcopy(observations))
                self.fixture.save_receipt()
                with self.assertRaises(ValueError):
                    self.parent()

    def test_successful_parent_receipt_does_not_hide_changed_or_missing_import_body(self):
        row = self.fixture.receipt['imports'][0]
        path = self.fixture.cache / row['response']['blob']
        original = path.read_bytes()
        path.write_bytes(b'X' * len(original))
        with self.assertRaises(ValueError):
            self.parent()
        path.unlink()
        with self.assertRaises((ValueError, FileNotFoundError)):
            self.parent()

    def test_reauthenticated_import_content_still_requires_selected_timestamp(self):
        row = self.fixture.receipt['imports'][0]
        path = self.fixture.cache / row['response']['blob']
        body = encoded({'result': {row['archive']: ['20261001T000000Z']}})
        path.write_bytes(body)
        row['response'].update(identity(body))
        row['imports_in_response'] = ['20261001T000000Z']
        self.fixture.save_receipt()
        with self.assertRaises(ValueError):
            self.parent()

    def test_unchanged_declared_parent_authentication_does_not_hide_mutated_deb_source_or_index(self):
        descriptors = [self.fixture.materials['packages'][0],
                       self.fixture.materials['sources'][0]['files'][0],
                       self.fixture.materials['repositories'][0]['indices'][0]]
        for descriptor in descriptors:
            path = self.fixture.cache / descriptor['blob']
            original = path.read_bytes()
            with self.subTest(blob=descriptor['blob']):
                path.write_bytes(b'X' * len(original))
                with self.assertRaises(ValueError):
                    self.parent()
                path.write_bytes(original)

    def test_cache_links_and_descriptor_escape_cannot_read_foreign_material(self):
        row = self.fixture.materials['packages'][0]
        path = self.fixture.cache / row['blob']
        original = path.read_bytes()
        outside = self.root / 'outside.deb'
        outside.write_bytes(original)
        path.unlink()
        path.symlink_to(outside)
        with self.assertRaises((ValueError, OSError)):
            self.parent()
        path.unlink()
        path.write_bytes(original)
        os.link(path, self.root / 'foreign-hardlink')
        with self.assertRaises(ValueError):
            self.parent()
        (self.root / 'foreign-hardlink').unlink()
        row['blob'] = '../outside.deb'
        raw = encoded(self.fixture.materials)
        (self.fixture.directory / 'materials.json').write_bytes(raw)
        self.fixture.receipt['materials_sha256'] = identity(raw)['sha256']
        self.fixture.save_receipt()
        with self.assertRaises(ValueError):
            self.parent()

    def test_strict_subset_uses_exact_deb_and_source_bytes_without_changing_parent(self):
        before = self.fixture.snapshot()
        child = self.fixture.child()
        self.inputs.strict_subset(self.fixture.materials, child)
        self.assertLess(len(child['packages']), len(self.fixture.materials['packages']))
        self.assertEqual({row['name'] for row in child['sources']}, {'fixture-minimal'})
        self.assertEqual(self.fixture.snapshot(), before)

    def test_deb_identity_version_architecture_repository_and_blob_cannot_drift(self):
        for key, replacement in (('name', 'new-package'), ('version', '2.0'), ('architecture', 'arm64'),
                                 ('repository', 'security'), ('filename', 'pool/main/f/foreign.deb'),
                                 ('blob', '../outside.deb'), ('size', 1), ('sha256', '0' * 64)):
            with self.subTest(key=key):
                child = self.fixture.child()
                child['packages'][0][key] = replacement
                with self.assertRaises(ValueError):
                    self.inputs.strict_subset(self.fixture.materials, child)

    def test_corresponding_source_identity_files_and_repository_cannot_drift(self):
        for key, replacement in (('name', 'foreign'), ('version', '2.0'),
                                 ('repository', 'security'), ('directory', 'pool/main/f/foreign')):
            with self.subTest(key=key):
                child = self.fixture.child()
                child['sources'][0][key] = replacement
                with self.assertRaises(ValueError):
                    self.inputs.strict_subset(self.fixture.materials, child)
        for key, replacement in (('name', 'foreign.dsc'), ('blob', '../outside.source'),
                                 ('sha256', '0' * 64), ('size', 1)):
            with self.subTest(key=key):
                child = self.fixture.child()
                child['sources'][0]['files'][0][key] = replacement
                with self.assertRaises(ValueError):
                    self.inputs.strict_subset(self.fixture.materials, child)

    def test_source_closure_rejects_missing_extra_and_duplicated_sources(self):
        initial = self.fixture.child()
        for sources in ([], initial['sources'] * 2, self.fixture.materials['sources']):
            with self.subTest(sources=sources):
                child = copy.deepcopy(initial)
                child['sources'] = copy.deepcopy(sources)
                with self.assertRaises(ValueError):
                    self.inputs.strict_subset(self.fixture.materials, child)

    def test_new_materials_cannot_change_parent_snapshot_keyring_epoch_or_family(self):
        for key, replacement in (('arch', 'arm64'), ('source_epoch', 1)):
            child = self.fixture.child()
            child[key] = replacement
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.inputs.strict_subset(self.fixture.materials, child)
        for key in ('keyring', 'repositories'):
            child = self.fixture.child()
            if key == 'keyring':
                child[key]['sha256'] = '0' * 64
            else:
                child[key][0]['timestamp'] = '20261001T000000Z'
            with self.subTest(key=key), self.assertRaises(ValueError):
                self.inputs.strict_subset(self.fixture.materials, child)

    def test_derivation_is_new_local_evidence_with_exact_minimal_materials_and_no_cache_links(self):
        before = self.fixture.snapshot()
        output = self.derive()
        verified = self.verify(output)
        receipt = verified['receipt']
        self.assertEqual(receipt['kind'], 'apsp-ipquality-debian-input-derivation')
        self.assertEqual(receipt['parent']['kind'], 'apsp-nodequality-debian-input-collection')
        self.assertEqual(verified['child'], self.fixture.child())
        self.assertEqual(receipt['cache_directory'], str(self.fixture.cache))
        self.assertEqual(receipt['borrowed_new_allocated_bytes'], 0)
        self.assertGreater(receipt['borrowed_logical_bytes'], 0)
        for key in ('installation', 'network_requests', 'payload_downloads', 'builder_approved',
                    'runtime_image_identity_verified', 'reproducibility_verified', 'full_ready', 'lock_ready'):
            self.assertIs(receipt[key], False, key)
        self.assertIsNone(receipt['builder'])
        self.assertFalse((output / 'input-cache').exists())
        self.assertFalse((output / 'collection.json').exists())
        self.assertFalse((output / 'inputs-lock.json').exists())
        self.assertEqual(self.fixture.snapshot(), before)
        self.assertTrue((output / 'factory-capacity.json').is_file())
        memory = json.loads((output / 'memory-observation.json').read_bytes())
        self.assertEqual(memory['memory_floor_bytes'], 256 * 1024**2)
        self.assertEqual(memory['oom_counter_baseline'], {'/TEST_ONLY-cgroup': {'oom': 0, 'oom_kill': 0}})
        self.assertIs(memory['reclaim_is_guaranteed'], False)
        for name, digest in receipt['files_sha256'].items():
            self.assertEqual(identity((output / name).read_bytes())['sha256'], digest)

    def test_missing_parent_payload_stays_unknown_without_new_download_and_cleans_only_output(self):
        path = self.fixture.cache / self.fixture.materials['packages'][0]['blob']
        path.unlink()
        before = self.fixture.snapshot()
        with patch.object(self.loaded['collect'].Collector, 'obtain', side_effect=AssertionError('unexpected download')) as fetch:
            with self.assertRaises((ValueError, FileNotFoundError)):
                self.derive()
        fetch.assert_not_called()
        self.assertEqual(self.fixture.snapshot(), before)
        self.assertFalse((self.root / 'derived').exists())

    def test_unregistered_solver_selection_is_rejected_without_source_or_deb_download(self):
        self.selected_override = copy.deepcopy(self.fixture.child()['packages'])
        self.selected_override[0]['version'] = '2.0'
        before = self.fixture.snapshot()
        with patch.object(self.loaded['collect'].Collector, 'obtain', side_effect=AssertionError('unexpected download')) as fetch:
            with self.assertRaisesRegex(ValueError, 'exact parent subset'):
                self.derive()
        fetch.assert_not_called()
        self.assertEqual(self.fixture.snapshot(), before)
        self.assertFalse((self.root / 'derived').exists())

    def test_derived_receipt_cannot_claim_collection_download_approval_installation_or_full_execution(self):
        output = self.derive()
        initial = json.loads((output / 'derivation.json').read_bytes())
        changes = [('kind', 'apsp-ipquality-debian-input-collection'), ('source_authenticated', False)]
        changes += [(key, True) for key in ('builder_approved', 'runtime_image_identity_verified',
                                          'keyring_trust_independently_verified', 'reproducibility_verified',
                                          'full_ready', 'installation', 'network_requests', 'payload_downloads')]
        for key, value in changes:
            with self.subTest(key=key):
                changed = copy.deepcopy(initial)
                changed[key] = value
                (output / 'derivation.json').write_bytes(encoded(changed))
                with self.assertRaises(ValueError):
                    self.verify(output)
        (output / 'derivation.json').write_bytes(encoded(initial))
        (output / 'collection.json').write_bytes(encoded({'TEST_ONLY': 'forged HTTP record'}))
        with self.assertRaisesRegex(ValueError, 'impersonate'):
            self.verify(output)

    def test_old_parent_receipt_mutation_invalidates_new_derivation(self):
        output = self.derive()
        receipt = self.fixture.directory / 'collection.json'
        receipt.write_bytes(receipt.read_bytes() + b' ')
        with self.assertRaisesRegex(ValueError, 'parent identity changed'):
            self.verify(output)

    def test_same_digest_with_replaced_parent_inode_is_still_stale(self):
        output = self.derive()
        path = self.fixture.cache / self.fixture.materials['keyring']['blob']
        content = path.read_bytes()
        replacement = self.fixture.cache / 'new-keyring-inode'
        replacement.write_bytes(content)
        os.replace(replacement, path)
        with self.assertRaisesRegex(ValueError, 'stale|identity'):
            self.verify(output)

    def test_input_ledger_cross_scope_traversal_duplicate_and_identity_forgery_are_refused(self):
        output = self.derive()
        initial = json.loads((output / 'input-ledger.json').read_bytes())
        for kind in ('traversal', 'scope', 'duplicate', 'digest', 'directory'):
            with self.subTest(kind=kind):
                ledger = copy.deepcopy(initial)
                if kind == 'traversal':
                    ledger['files'][0]['path'] = '../outside'
                elif kind == 'scope':
                    ledger['files'][0]['scope'] = 'external'
                elif kind == 'duplicate':
                    ledger['files'].append(copy.deepcopy(ledger['files'][0]))
                elif kind == 'digest':
                    ledger['files'][0]['identity']['sha256'] = '0' * 64
                else:
                    ledger['cache_directory'] = str(self.root)
                with self.assertRaises((ValueError, OSError)):
                    self.inputs.verify_ledger(ledger, self.deadline(), self.loaded['build'])

    def test_profile_code_and_input_ledger_are_not_accepted_just_by_rehashing_sidecars(self):
        output = self.derive()
        for name in ('profile.json', 'code-ledger.json', 'input-ledger.json'):
            original = (output / name).read_bytes()
            changed = json.loads(original)
            if name == 'profile.json':
                changed['commands']['fio'] = 'fio'
            elif name == 'code-ledger.json':
                changed[0]['identity']['sha256'] = '0' * 64
            else:
                changed['files'].pop()
            self.change_sidecar(output, name, changed)
            with self.subTest(name=name), self.assertRaises(ValueError):
                self.verify(output)
            self.change_sidecar(output, name, json.loads(original))

    def test_real_tool_bytes_and_inode_are_bound_without_claiming_native_approval(self):
        output = self.derive()
        evidence = json.loads((output / 'tool-evidence.json').read_bytes())
        self.assertFalse(evidence['builder_approved'])
        self.assertFalse(evidence['runtime_image_identity_verified'])
        self.assertEqual(evidence['snapshots'][0]['identity']['sha256'], identity(self.tool_path.read_bytes())['sha256'])
        body = self.tool_path.read_bytes()
        self.tool_path.write_bytes(b'X' * len(body))
        with self.assertRaisesRegex(ValueError, 'tool bytes|identity'):
            self.verify(output)

    def test_new_parent_path_with_equal_bytes_cannot_replace_original_ledger_identity(self):
        output = self.derive()
        clone = ParentFixture(self.root / 'cloned-parent', self.parent_factory, self.profile)
        receipt = json.loads((output / 'derivation.json').read_bytes())
        receipt['parent']['directory'] = str(clone.directory)
        receipt['cache_directory'] = str(clone.cache)
        (output / 'derivation.json').write_bytes(encoded(receipt))
        with self.assertRaisesRegex(ValueError, 'stale|identity'):
            self.verify(output)

    def test_new_output_plan_refuses_disk_inode_and_management_reserve_shortfalls(self):
        for changes, reason in (({'f_bavail': 1}, 'free_disk'), ({'f_favail': 1024}, 'free_inodes')):
            self.disk.return_value = self.space(**changes)
            with self.subTest(reason=reason):
                plan = self.inputs.output_plan(self.loaded['build'], self.root, 32 * 1024**2, 512 * 1024**2)
                self.assertFalse(plan['admitted'])
                self.assertIn(reason, plan['reasons'])
        self.disk.return_value = self.space()
        with self.assertRaises(ValueError):
            self.inputs.output_plan(self.loaded['build'], self.root, 32 * 1024**2, 1)

    def test_small_output_budget_and_admission_failure_leave_parent_and_existing_output_untouched(self):
        before = self.fixture.snapshot()
        with self.assertRaises(ValueError):
            self.derive(maximum=1)
        self.assertFalse((self.root / 'derived').exists())
        self.disk.return_value = self.space(f_favail=1024)
        with self.assertRaises(ValueError):
            self.derive(output=self.root / 'inode-refused')
        self.assertFalse((self.root / 'inode-refused').exists())
        self.disk.return_value = self.space()
        existing = self.root / 'existing'
        existing.mkdir(mode=0o700)
        sentinel = existing / 'old-evidence'
        sentinel.write_bytes(b'TEST_ONLY preserve previous material')
        with self.assertRaises(FileExistsError):
            self.derive(output=existing)
        self.assertEqual(sentinel.read_bytes(), b'TEST_ONLY preserve previous material')
        self.assertEqual(self.fixture.snapshot(), before)

    def test_memory_floor_and_cgroup_cache_do_not_silently_weaken_management_protection(self):
        self.assertGreaterEqual(self.inputs.MEMORY_RESERVE, 256 * 1024**2)
        low = self.memory(256 * 1024**2 - 1)
        with patch.object(self.loaded['capacity'], 'available_memory', return_value=low):
            with self.assertRaisesRegex(ValueError, 'MemAvailable|memory'):
                self.derive()
        self.assertFalse((self.root / 'derived').exists())
        cache_pressure = self.memory()
        cache_pressure['cgroup_raw_headroom_bytes'] = 0
        cache_pressure['reclaim_estimate_available_bytes'] = 0
        cache_pressure['cgroup_observations'][0].update(
            current_bytes=16 * 1024**3, raw_headroom_bytes=0, reclaim_estimate_headroom_bytes=0)
        with patch.object(self.loaded['capacity'], 'available_memory', return_value=cache_pressure):
            output = self.derive()
        self.assertTrue((output / 'derivation.json').is_file())

    def test_solver_abort_cleans_only_owned_new_output_preserving_parent(self):
        before = self.fixture.snapshot()
        for exception in (KeyboardInterrupt('TEST_ONLY interrupt'), ValueError('TEST_ONLY solver refusal')):
            with self.subTest(exception=type(exception).__name__):
                self.solver.side_effect = exception
                with self.assertRaises(type(exception)):
                    self.derive()
                self.assertFalse((self.root / 'derived').exists())
                self.assertEqual(self.fixture.snapshot(), before)

    def test_replaced_output_is_not_deleted_and_failed_cleanup_remains_explicit(self):
        before = self.fixture.snapshot()
        original = self.root / 'derived'
        retained = self.root / 'moved-owned-output'
        def replace_output(collector, *args, **kwargs):
            collector.output.rename(retained)
            original.mkdir(mode=0o700)
            (original / 'foreign-material').write_bytes(b'TEST_ONLY unrelated replacement; retain it')
            return self.solve_fixture(collector, *args, **kwargs)
        self.solver.side_effect = replace_output
        with self.assertRaisesRegex(ValueError, 'identity'):
            self.derive()
        self.assertEqual((original / 'foreign-material').read_bytes(),
                         b'TEST_ONLY unrelated replacement; retain it')
        self.assertTrue((retained / 'capacity-plan.json').is_file())
        evidence = list(self.root.glob('derived-failure-*/cleanup.json'))
        self.assertEqual(len(evidence), 1)
        cleanup = json.loads(evidence[0].read_bytes())
        self.assertIs(cleanup['removed'], False)
        self.assertIs(cleanup['retained'], True)
        self.assertEqual(self.fixture.snapshot(), before)

    def test_memory_drop_during_solver_stops_and_cleans_only_owned_output(self):
        before = self.fixture.snapshot()
        def reduced_memory(*args, **kwargs):
            self.memory_observation.return_value = self.memory(256 * 1024**2 - 1)
            return self.solve_fixture(*args, **kwargs)
        self.solver.side_effect = reduced_memory
        with self.assertRaisesRegex(ValueError, 'MemAvailable|memory'):
            self.derive()
        self.assertFalse((self.root / 'derived').exists())
        self.assertEqual(self.fixture.snapshot(), before)

    def test_parent_mutation_during_derivation_refuses_the_final_receipt(self):
        parent_receipt = self.fixture.directory / 'collection.json'
        before = parent_receipt.read_bytes()
        stale = b'X' + before[1:]
        def stale_parent(*args, **kwargs):
            parent_receipt.write_bytes(stale)
            return self.solve_fixture(*args, **kwargs)
        self.solver.side_effect = stale_parent
        with self.assertRaisesRegex(ValueError, 'identity'):
            self.derive()
        self.assertFalse((self.root / 'derived').exists())
        self.assertEqual(parent_receipt.read_bytes(), stale)

    def test_output_overlapping_parent_is_refused_without_removing_old_input(self):
        before = self.fixture.snapshot()
        with self.assertRaisesRegex(ValueError, 'disjoint'):
            self.derive(output=self.fixture.directory / 'derived')
        self.assertFalse((self.fixture.directory / 'derived').exists())
        self.assertEqual(self.fixture.snapshot(), before)

    def test_deadline_and_growth_during_snapshot_are_rejected(self):
        path = self.root / 'changing'
        path.write_bytes(b'x' * (2 * 65536))
        class MutatingDeadline:
            def __init__(self):
                self.count = 0
            def check(self):
                self.count += 1
                if self.count == 1:
                    with path.open('ab') as stream:
                        stream.write(b'additional bytes')
        with self.assertRaisesRegex(ValueError, 'grew|changed'):
            self.inputs.snapshot_file(path, 1024**2, MutatingDeadline())
        deadline = self.deadline()
        deadline.end = 0
        with self.assertRaisesRegex(ValueError, 'deadline'):
            self.inputs.snapshot_file(path, 1024**2, deadline)

    def test_dynamic_disk_and_inode_reserve_stop_writes_before_crossing_management_floor(self):
        for kind in ('disk', 'inode'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory(dir=self.root) as name:
                output = Path(name).resolve()
                plan = self.inputs.output_plan(self.loaded['build'], output.parent, 32 * 1024**2, 512 * 1024**2)
                capacity = self.loaded['build'].FactoryCapacity(output, plan, self.deadline())
                guard = self.inputs.Guard(capacity, self.loaded['capacity'])
                guard.check(force=True)
                self.disk.return_value = self.space(f_bavail=1) if kind == 'disk' else self.space(f_favail=1023)
                with self.assertRaisesRegex(ValueError, 'reserve'):
                    guard.write(output / 'refused.json', b'{}\n')
                self.assertFalse((output / 'refused.json').exists())
                self.disk.return_value = self.space()

    def test_cgroup_max_pressure_is_observed_but_oom_or_membership_changes_stop_guard(self):
        base = self.memory()
        base['cgroup_observations'][0]['memory_events']['max'] = 2
        for change in ('oom', 'oom_kill', 'membership'):
            with self.subTest(change=change), tempfile.TemporaryDirectory(dir=self.root) as name:
                output = Path(name).resolve()
                plan = self.inputs.output_plan(self.loaded['build'], output.parent, 32 * 1024**2, 512 * 1024**2)
                capacity = self.loaded['build'].FactoryCapacity(output, plan, self.deadline())
                guard = self.inputs.Guard(capacity, self.loaded['capacity'])
                with patch.object(self.loaded['capacity'], 'available_memory', return_value=base):
                    guard.check(force=True)
                pressure = copy.deepcopy(base)
                pressure['cgroup_observations'][0]['memory_events']['max'] += 1
                with patch.object(self.loaded['capacity'], 'available_memory', return_value=pressure):
                    guard.check(force=True)
                bad = copy.deepcopy(pressure)
                if change == 'membership':
                    bad['cgroup_observations'][0]['path'] += '-changed'
                else:
                    bad['cgroup_observations'][0]['memory_events'][change] += 1
                with patch.object(self.loaded['capacity'], 'available_memory', return_value=bad):
                    with self.assertRaisesRegex(ValueError, 'OOM|membership'):
                        guard.check(force=True)

    def test_unknown_cgroup_oom_observation_cannot_start_derivation(self):
        before = self.fixture.snapshot()
        for change in ('v2', 'empty', 'missing_oom', 'invalid_oom'):
            unknown = self.memory()
            if change == 'v2':
                unknown['cgroup_v2_observed'] = False
            elif change == 'empty':
                unknown['cgroup_observations'] = []
            elif change == 'missing_oom':
                del unknown['cgroup_observations'][0]['memory_events']['oom']
            else:
                unknown['cgroup_observations'][0]['memory_events']['oom_kill'] = True
            with self.subTest(change=change), patch.object(
                    self.loaded['capacity'], 'available_memory', return_value=unknown):
                with self.assertRaisesRegex(ValueError, 'unknown|OOM'):
                    self.derive()
            self.assertFalse((self.root / 'derived').exists())
            self.assertEqual(self.fixture.snapshot(), before)

    def test_solver_profile_and_namespace_record_are_rechecked_even_with_fresh_digest(self):
        output = self.derive()
        initial = json.loads((output / 'solver/selection.json').read_bytes())
        original_receipt = json.loads((output / 'derivation.json').read_bytes())
        for change in ('seeds', 'selection', 'namespace', 'interfaces', 'download'):
            with self.subTest(change=change):
                changed = copy.deepcopy(initial)
                if change == 'seeds':
                    changed['seeds'] = []
                elif change == 'selection':
                    changed['package_selection'][0]['version'] = '2.0'
                elif change == 'namespace':
                    changed['network_namespaces'][0]['different_namespace'] = False
                elif change == 'interfaces':
                    changed['network_namespaces'][0]['interfaces'] = ['lo', 'eth0']
                else:
                    changed['binary_downloads_by_solver'] = True
                body = encoded(changed)
                (output / 'solver/selection.json').write_bytes(body)
                receipt = copy.deepcopy(original_receipt)
                receipt['solver_selection_sha256'] = identity(body)['sha256']
                (output / 'derivation.json').write_bytes(encoded(receipt))
                with self.assertRaises(ValueError):
                    self.verify(output)

    def candidate(self):
        tools = []
        self.candidate_files = {}
        for name, native_path in sorted(self.loaded['build'].TOOL_PATHS.items()):
            path = self.root / ('candidate-' + name)
            path.write_bytes(('TEST_ONLY isolated native binary IO: ' + name).encode())
            self.candidate_files[native_path] = path
            tools.append(dict(name=name, path=native_path, version='TEST_ONLY owned binary fixture',
                              **identity(path.read_bytes())))
        candidate = {'image_sha256': identity(b'TEST_ONLY candidate image, not approval')['sha256'],
                     'arch': 'amd64', 'tools': tools}
        path = self.root / 'candidate-builder.json'
        path.write_bytes(encoded(candidate))
        return path

    @contextlib.contextmanager
    def candidate_io(self):
        build = self.loaded['collect'].BUILD
        original = build.file_identity
        def observed(path, limit, deadline=None):
            if str(path) in self.candidate_files:
                value = self.inputs.snapshot_file(self.candidate_files[str(path)], limit, deadline or self.deadline())
                return {key: value[key] for key in ('size', 'sha256')}
            return original(path, limit, deadline)
        with patch.object(build, 'file_identity', side_effect=observed):
            yield

    def test_binding_reauthenticates_replays_subset_and_binds_candidate_without_approval(self):
        output = self.derive()
        candidate = self.candidate()
        before = self.fixture.snapshot()
        with self.candidate_io(), \
             patch.object(self.loaded['build'], 'verify_tools') as approve, \
             patch.object(self.loaded['build'], 'prepare') as prepare:
            receipt = self.inputs.bind(output, candidate, self.root / 'bound', 60)
        approve.assert_not_called()
        prepare.assert_not_called()
        self.assertEqual(self.solver.call_count, 2)
        self.assertEqual(receipt['kind'], 'apsp-ipquality-debian-input-derivation-binding')
        self.assertTrue(receipt['lock_ready'])
        for key in ('builder_approved', 'runtime_image_identity_verified', 'reproducibility_verified',
                    'full_ready', 'installation', 'network_requests', 'payload_downloads'):
            self.assertIs(receipt[key], False, key)
        lock = json.loads((self.root / 'bound/inputs-lock.json').read_bytes())
        self.loaded['build'].validate_lock(lock)
        self.assertEqual({key: value for key, value in lock.items() if key != 'builder'}, self.fixture.child())
        self.assertEqual(receipt['inputs_lock_sha256'], identity((self.root / 'bound/inputs-lock.json').read_bytes())['sha256'])
        self.assertEqual(self.fixture.snapshot(), before)
        self.assertFalse((self.root / 'bound/input-cache').exists())

    def test_binding_refuses_changed_parent_before_candidate_approval_or_new_lock(self):
        output = self.derive()
        candidate = self.candidate()
        body = self.fixture.directory / 'collection.json'
        body.write_bytes(body.read_bytes() + b' ')
        before = self.fixture.snapshot()
        with self.candidate_io(), patch.object(self.loaded['build'], 'verify_tools') as approve:
            with self.assertRaises(ValueError):
                self.inputs.bind(output, candidate, self.root / 'bound', 60)
        approve.assert_not_called()
        self.assertFalse((self.root / 'bound').exists())
        self.assertEqual(self.fixture.snapshot(), before)

    def test_binding_refuses_solver_replay_drift_and_changed_native_candidate_bytes(self):
        output = self.derive()
        candidate = self.candidate()
        self.selected_override = copy.deepcopy(self.fixture.child()['packages'])
        self.selected_override.append(next(copy.deepcopy(row) for row in self.fixture.materials['packages']
                                           if row['name'] == 'fixture-extra'))
        with self.candidate_io(), self.assertRaisesRegex(ValueError, 'binding replay'):
            self.inputs.bind(output, candidate, self.root / 'drifted', 60)
        self.assertFalse((self.root / 'drifted').exists())
        self.selected_override = None
        path = next(iter(self.candidate_files.values()))
        original = path.read_bytes()
        path.write_bytes(b'X' * len(original))
        with self.candidate_io(), self.assertRaisesRegex(ValueError, 'candidate tool bytes'):
            self.inputs.bind(output, candidate, self.root / 'changed-tool', 60)
        self.assertFalse((self.root / 'changed-tool').exists())

    def test_binding_overlap_and_wrong_architecture_preserve_both_input_directories(self):
        output = self.derive()
        candidate = self.candidate()
        before = self.fixture.snapshot()
        with self.candidate_io():
            for target in (output / 'bound', self.fixture.directory / 'bound'):
                with self.subTest(target=target), self.assertRaisesRegex(ValueError, 'disjoint|overlap'):
                    self.inputs.bind(output, candidate, target, 60)
                self.assertFalse(target.exists())
            changed = json.loads(candidate.read_bytes())
            changed['arch'] = 'arm64'
            candidate.write_bytes(encoded(changed))
            with self.assertRaises(ValueError):
                self.inputs.bind(output, candidate, self.root / 'wrong-arch', 60)
        self.assertEqual(self.fixture.snapshot(), before)


if __name__ == '__main__':
    unittest.main()
