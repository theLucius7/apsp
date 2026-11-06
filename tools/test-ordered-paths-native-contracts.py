#!/usr/bin/env python3
"""Portable manifest contracts; the native controller main never executes."""
import copy
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "ordered_native_controller", ROOT / "tools/test-ordered-paths-native.py")
controller = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(controller)

COMMON = {
    "lib.rs", "relays.rs", "external.rs", "protocols.rs", "certificates.rs",
    "paths/mod.rs", "paths/model.rs", "paths/network.rs", "paths/context.rs",
    "paths/validate.rs", "paths/render.rs", "paths/transport_validation.rs",
    "paths/outbound_validation.rs",
}
LEGACY = COMMON | {"settings.rs"}
SPLIT = COMMON | {
    "settings/mod.rs", "settings/apply.rs", "settings/transport.rs", "settings/validate.rs",
}
CLIENT = SPLIT | {"client.rs"}


def source_map(names):
    return {name: hashlib.sha256(name.encode()).hexdigest() for name in names}


class CompilerSourceIdentityContracts(unittest.TestCase):
    def test_exact_historical_split_and_client_source_sets_are_accepted(self):
        for names in (LEGACY, SPLIT, CLIENT):
            with self.subTest(source_count=len(names)):
                controller.compiler_source_identity(source_map(names))

    def test_same_size_renaming_cannot_replace_a_source_identity(self):
        for names in (LEGACY, SPLIT, CLIENT):
            for original in sorted(names):
                with self.subTest(source_count=len(names), original=original):
                    value = source_map(names)
                    value["TEST_ONLY_unknown.rs"] = value.pop(original)
                    with self.assertRaisesRegex(controller.Rejected, "compiler_source_identity_missing"):
                        controller.compiler_source_identity(value)

    def test_missing_extra_and_mixed_layouts_are_rejected(self):
        for names in (COMMON, LEGACY | SPLIT, SPLIT | {"TEST_ONLY_extra.rs"},
                      SPLIT - {"settings/validate.rs"}, LEGACY | {"settings/mod.rs"},
                      CLIENT - {"settings/validate.rs"}, LEGACY | {"client.rs"}):
            with self.subTest(names=sorted(names)), \
                    self.assertRaisesRegex(controller.Rejected, "compiler_source_identity_missing"):
                controller.compiler_source_identity(source_map(names))

    def test_malformed_containers_and_digest_values_are_rejected(self):
        for value in (None, [], "TEST_ONLY", tuple(sorted(LEGACY))):
            with self.subTest(container=type(value).__name__), \
                    self.assertRaisesRegex(controller.Rejected, "compiler_source_identity_missing"):
                controller.compiler_source_identity(value)
        for digest in (None, 64, b"a" * 64, "a" * 63, "a" * 65,
                       "A" * 64, "g" * 64, "é" * 64, "a" * 63 + "\n"):
            with self.subTest(digest_type=type(digest).__name__, digest=repr(digest)), \
                    self.assertRaisesRegex(controller.Rejected, "compiler_source_identity_missing"):
                value = source_map(LEGACY)
                value["lib.rs"] = digest
                controller.compiler_source_identity(value)

    def test_current_producer_binds_every_client_source_file(self):
        example = ROOT / "crates/compiler/examples/ordered_native_fixture.rs"
        section = example.read_text().split("let sources = [", 1)[1].split("];", 1)[0]
        rows = re.findall(r'\(\s*"([^"]+)",\s*include_bytes!\("../src/([^"]+)"\)\.as_slice\(\),?\s*\)', section)
        self.assertEqual(len(rows), len(CLIENT))
        self.assertEqual({name for name, _ in rows}, CLIENT)
        self.assertTrue(all(name == relative for name, relative in rows))
        identities = {name: hashlib.sha256(
            (ROOT / "crates/compiler/src" / relative).read_bytes()).hexdigest()
            for name, relative in rows}
        controller.compiler_source_identity(identities)


class NativeIdentityContracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="s155identity-contract-")
        self.addCleanup(self.temporary.cleanup)
        self.binary = Path(self.temporary.name) / "TEST_ONLY_inert_binary"
        self.binary.write_bytes(b"TEST_ONLY never executed")
        self.binary.chmod(0o600)
        self.old_deadline = controller.DEADLINE
        controller.DEADLINE = time.monotonic() + 5
        self.addCleanup(setattr, controller, "DEADLINE", self.old_deadline)
        self.digest = controller.sha_file(self.binary)

    def test_pinned_binary_version_and_required_flags_are_accepted(self):
        tags = sorted(controller.REQUIRED_TAGS | {"TEST_ONLY_extra_tag"})
        version = ("sing-box version 1.14.0-apsp.1\nTags: " + ",".join(tags) + "\n").encode()
        with mock.patch.object(controller, "capture", return_value=version) as capture:
            identity = controller.binary_identity(self.binary, self.digest)
        capture.assert_called_once_with([str(self.binary), "version"])
        self.assertEqual(identity["sha256"], self.digest)
        self.assertEqual(identity["version"], "1.14.0-apsp.1")
        self.assertEqual(identity["observed_build_tags"], tags)

    def test_wrong_runtime_or_each_missing_build_flag_is_rejected(self):
        for missing in (None, *sorted(controller.REQUIRED_TAGS)):
            with self.subTest(missing=missing):
                tags = controller.REQUIRED_TAGS - {missing}
                version = "1.14.3" if missing is None else "1.14.0-apsp.1"
                output = ("sing-box version " + version + "\nTags: " + ",".join(sorted(tags))).encode()
                code = "exact_runtime_version_required" if missing is None else "required_runtime_tags_missing"
                with mock.patch.object(controller, "capture", return_value=output), \
                        self.assertRaisesRegex(controller.Rejected, code):
                    controller.binary_identity(self.binary, self.digest)

    def test_wrong_binary_hash_refuses_before_any_command(self):
        with mock.patch.object(controller, "capture") as capture, \
                self.assertRaisesRegex(controller.Rejected, "native_binary_hash_mismatch"):
            controller.binary_identity(self.binary, "c" * 64)
        capture.assert_not_called()


class CompilationManifestContracts(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="s155ordered-contract-")
        self.addCleanup(self.temporary.cleanup)
        self.base = Path(self.temporary.name)
        self.prepared = self.base / "prepared"
        self.compiled = self.base / "compiled"
        self.prepared.mkdir(mode=0o700)
        self.compiled.mkdir(mode=0o700)
        self.old_deadline = controller.DEADLINE
        controller.DEADLINE = time.monotonic() + 5
        self.addCleanup(setattr, controller, "DEADLINE", self.old_deadline)
        for name in ("OwnedProcess", "capture", "prepare", "run"):
            guard = mock.patch.object(controller, name, side_effect=AssertionError(
                "native execution is forbidden in portable contracts"))
            self.addCleanup(guard.stop)
            guard.start()
        self.inputs = {"native_binary_sha256": "a" * 64,
                       "ports": {"client": 22080, "stats_a": 22081,
                                 "stats_m": 22082, "stats_b": 22083}}
        self.write(self.prepared / "compiler-input.json", self.inputs)
        self.manifest = {
            "schema": 1, "test_only": True, "runtime_version": "1.14.0-apsp.1",
            "native_binary_sha256": self.inputs["native_binary_sha256"],
            "compiler_input_sha256": controller.sha_file(self.prepared / "compiler-input.json"),
            "generator_source_sha256": "b" * 64,
            "compiler_sources": source_map(CLIENT), "cases": [],
        }
        for case_name in ("three", "four"):
            files = {}
            for role in ("A", "M", "B", "client"):
                source = {"inbounds": [{"listen_port": 2080}],
                          "experimental": {"v2ray_api": {"listen": "127.0.0.1:18085"}},
                          "route": {"final": "TEST_ONLY_outbound"},
                          "outbounds": [{"tag": "TEST_ONLY_outbound",
                                         "uuid": "TEST_ONLY_credential"}]}
                pointer = "/inbounds/0/listen_port" if role == "client" else "/experimental/v2ray_api/listen"
                target = self.inputs["ports"]["client"] if role == "client" else \
                    "127.0.0.1:" + str(self.inputs["ports"]["stats_" + role.lower()])
                fixture = copy.deepcopy(source)
                controller.pointer_assign(fixture, pointer, target)
                product_name, fixture_name = f"{case_name}-{role}-product.json", f"{case_name}-{role}-fixture.json"
                self.write(self.compiled / product_name, source)
                self.write(self.compiled / fixture_name, fixture)
                files[role] = {"product_file": product_name, "fixture_file": fixture_name,
                               "product_sha256": controller.sha_file(self.compiled / product_name),
                               "fixture_sha256": controller.sha_file(self.compiled / fixture_name),
                               "transforms": [{"pointer": pointer, "from": 2080 if role == "client" else
                                               "127.0.0.1:18085", "to": target}]}
            self.manifest["cases"].append({"name": case_name, "files": files})
        self.args = SimpleNamespace(compiled_dir=str(self.compiled),
                                    compilation_manifest_sha256=None,
                                    generator_source_sha256="b" * 64)
        self.save_manifest()

    @staticmethod
    def write(path, value):
        path.write_text(json.dumps(value, sort_keys=True))
        path.chmod(0o600)

    def save_manifest(self):
        path = self.compiled / "compilation-manifest.json"
        self.write(path, self.manifest)
        self.args.compilation_manifest_sha256 = controller.sha_file(path)

    def validate(self):
        return controller.compilation(self.args, self.prepared, self.inputs)

    def test_full_manifest_accepts_all_exact_source_layouts_without_native_execution(self):
        for names in (LEGACY, SPLIT, CLIENT):
            with self.subTest(source_count=len(names)):
                self.manifest["compiler_sources"] = source_map(names)
                self.save_manifest()
                parent, manifest = self.validate()
                self.assertEqual(parent, self.compiled)
                self.assertEqual(manifest, self.manifest)

    def test_manifest_edit_requires_the_independent_receipt_hash(self):
        self.manifest["compiler_sources"] = source_map(LEGACY)
        self.write(self.compiled / "compilation-manifest.json", self.manifest)
        with self.assertRaisesRegex(controller.Rejected, "compilation_receipt_hash_mismatch"):
            self.validate()

    def test_input_native_and_generator_bindings_remain_required(self):
        original = copy.deepcopy(self.manifest)
        for field in ("native_binary_sha256", "compiler_input_sha256", "generator_source_sha256"):
            with self.subTest(field=field):
                self.manifest = copy.deepcopy(original)
                self.manifest[field] = "c" * 64
                self.save_manifest()
                code = "generator_source_binding_mismatch" if field == "generator_source_sha256" else \
                    "compiled_input_binding_mismatch"
                with self.assertRaisesRegex(controller.Rejected, code):
                    self.validate()

    def test_test_only_schema_and_pinned_runtime_remain_required(self):
        original = copy.deepcopy(self.manifest)
        for field, value in (("schema", 2), ("test_only", False), ("test_only", 1),
                             ("runtime_version", "TEST_ONLY_wrong_version")):
            with self.subTest(field=field, value=value):
                self.manifest = copy.deepcopy(original)
                self.manifest[field] = value
                self.save_manifest()
                with self.assertRaisesRegex(controller.Rejected, "invalid_compilation_contract"):
                    self.validate()

    def test_both_complete_cases_and_all_product_roles_remain_required(self):
        original = copy.deepcopy(self.manifest)
        for mutate, code in ((lambda value: value["cases"].pop(), "complete_path_cases_required"),
                             (lambda value: value["cases"].reverse(), "complete_path_cases_required"),
                             (lambda value: value["cases"][0]["files"].pop("M"), "product_role_missing")):
            with self.subTest(code=code):
                self.manifest = copy.deepcopy(original)
                mutate(self.manifest)
                self.save_manifest()
                with self.assertRaisesRegex(controller.Rejected, code):
                    self.validate()

    def test_source_identity_rejection_is_not_bypassed_by_a_new_manifest_hash(self):
        identities = self.manifest["compiler_sources"]
        identities["TEST_ONLY_replacement.rs"] = identities.pop("settings/validate.rs")
        self.save_manifest()
        with self.assertRaisesRegex(controller.Rejected, "compiler_source_identity_missing"):
            self.validate()

    def test_product_and_fixture_hashes_remain_bound(self):
        record = self.manifest["cases"][0]["files"]["A"]
        for key in ("product_file", "fixture_file"):
            with self.subTest(file=key):
                path = self.compiled / record[key]
                original = path.read_bytes()
                self.write(path, {"TEST_ONLY_changed": True})
                try:
                    with self.assertRaisesRegex(controller.Rejected, "compiled_file_hash_mismatch"):
                        self.validate()
                finally:
                    path.write_bytes(original)

    def test_only_the_exact_statistics_or_client_port_transform_is_allowed(self):
        original = copy.deepcopy(self.manifest)
        for role, field, value in (("A", "pointer", "/route/final"), ("A", "from", "127.0.0.1:1"),
                                   ("A", "to", "127.0.0.1:1"), ("client", "to", 22081)):
            with self.subTest(role=role, field=field):
                self.manifest = copy.deepcopy(original)
                self.manifest["cases"][0]["files"][role]["transforms"][0][field] = value
                self.save_manifest()
                with self.assertRaisesRegex(controller.Rejected, "unapproved_fixture_transform"):
                    self.validate()

    def test_rehashed_fixture_cannot_change_routing_or_authentication(self):
        record = self.manifest["cases"][0]["files"]["A"]
        path = self.compiled / record["fixture_file"]
        original = json.loads(path.read_text())
        for pointer in ("/route/final", "/outbounds/0/uuid"):
            with self.subTest(pointer=pointer):
                fixture = copy.deepcopy(original)
                controller.pointer_assign(fixture, pointer, "TEST_ONLY_changed")
                self.write(path, fixture)
                record["fixture_sha256"] = controller.sha_file(path)
                self.save_manifest()
                with self.assertRaisesRegex(controller.Rejected, "fixture_changes_product_routing_or_authentication"):
                    self.validate()


if __name__ == "__main__":
    unittest.main()
