#!/usr/bin/env python3
"""Real signed native TCP provenance and fixed-source build contract tests."""
import importlib.util
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import unittest
import tomllib
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
import release
import tcp_probe_artifact as tcp
import test_release
import publish
import tcp_probe_notices as collector

spec = importlib.util.spec_from_file_location("tcp_builder", ROOT / "tools/build-tcp-probe.py")
builder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(builder)


def git(root, *args):
    return subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True).stdout


def repository(root):
    root.mkdir()
    for name in ["Cargo.toml", "Cargo.lock", "LICENSE", "crates/tcp-probe/Cargo.toml",
                 "crates/tcp-probe/src/lib.rs", "crates/tcp-probe/src/main.rs",
                 "crates/tcp-probe/src/cli.rs", "crates/tcp-probe/src/engine.rs",
                 "crates/tcp-probe/src/journal.rs", "crates/tcp-probe/src/model.rs",
                 "tools/build-tcp-probe.py", "tools/tcp_probe_artifact.py", "tools/artifact_manifest.py", "tools/tcp_probe_notices.py", *tcp.BUNDLED_MUSL_FILES]:
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, target)
    git(root, "init", "-q")
    git(root, "config", "user.email", "test@example.invalid")
    git(root, "config", "user.name", "TEST_ONLY fixture")
    git(root, "add", ".")
    git(root, "commit", "-qm", "TEST_ONLY fixed source")
    return git(root, "rev-parse", "HEAD").decode().strip()


def elf(arch):
    data = bytearray(120)
    data[:6] = b"\x7fELF\x02\x01"
    struct.pack_into("<H", data, 18, 62 if arch == "amd64" else 183)
    struct.pack_into("<Q", data, 32, 64)
    struct.pack_into("<HH", data, 54, 56, 1)
    struct.pack_into("<IIQQQQQQ", data, 64, 1, 5, 0, 0, 0, len(data), len(data), 4096)
    return bytes(data)


def payload(repo, commit, arch):
    version, archive = builder.pinned_source(repo, commit)
    binary = elf(arch)
    files = {tcp.BINARY: binary, "source.tar.gz": archive,
             "Cargo.lock": (repo / "Cargo.lock").read_bytes(), "LICENSE": (repo / "LICENSE").read_bytes()}
    locked = next(p for p in tomllib.loads(files["Cargo.lock"].decode())["package"] if p.get("checksum"))
    original = dict(path="LICENSE", text="TEST_ONLY original dependency notice")
    rustc_info = "rustc 1.98.1 TEST_ONLY fixture\ncommit-hash: " + tcp.BUNDLED_MUSL_IDENTITY["rustc_commit"] + "\n"
    source = tcp.source_files(archive, commit)
    notices = dict(schema=1, target=tcp.TARGETS[arch], lock_sha256=tcp.digest(files["Cargo.lock"]),
                   dependencies=[dict(name=locked["name"], version=locked["version"], source=locked["source"],
                                      checksum=locked["checksum"], license="MIT", notices=[original])],
                   toolchain=[
                       dict(name="Rust standard library and bundled native libraries", version=rustc_info, notices=[original]),
                       dict(name="system musl build tooling", version="TEST_ONLY build tooling", notices=[original]),
                       tcp.bundled_musl(source, rustc_info),
                   ])
    files["THIRD_PARTY_NOTICES.txt"] = json.dumps(notices).encode()
    info = dict(schema=1, tool=tcp.BINARY, tool_version=tcp.TOOL_VERSION,
                artifact_version=version, source_repo=release.REPOSITORY, source_commit=commit,
                target=tcp.TARGETS[arch], rustc=rustc_info, cargo_locked=True,
                source_sha256=tcp.digest(archive), lock_sha256=tcp.digest(files["Cargo.lock"]),
                license_sha256=tcp.digest(files["LICENSE"]), binary_sha256=tcp.digest(binary),
                notices_sha256=tcp.digest(files["THIRD_PARTY_NOTICES.txt"]))
    files["build-info.json"] = json.dumps(info, sort_keys=True).encode()
    return version, files


class FixedSourceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="apsp-tcp-source-contract-")
        self.root = Path(self.temporary.name)
        self.repo = self.root / "repo"
        self.commit = repository(self.repo)

    def tearDown(self):
        self.temporary.cleanup()

    def test_fixed_source_can_be_an_ancestor_and_dirty_or_missing_sources_fail(self):
        version, original = builder.pinned_source(self.repo, self.commit)
        self.assertEqual(version, "0.3.0-" + self.commit + "-r1")
        (self.repo / "README.md").write_text("TEST_ONLY newer registration")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "newer registration fixture")
        self.assertEqual(builder.pinned_source(self.repo, self.commit)[1], original)
        for value in [None, "", "main", "unknown", "0.3.0", "g" * 40]:
            with self.subTest(value=value), self.assertRaises(ValueError):
                builder.pinned_source(self.repo, value)
        with self.assertRaises(subprocess.CalledProcessError):
            builder.pinned_source(self.repo, "0" * 40)
        (self.repo / "Cargo.lock").write_text("dirty unpinned dependencies")
        with self.assertRaisesRegex(ValueError, "clean"):
            builder.pinned_source(self.repo, self.commit)

    def test_orchestrator_executes_the_recipe_from_the_fixed_archive(self):
        pinned_recipe = (self.repo / "tools/build-tcp-probe.py").read_bytes()
        with (self.repo / "tools/build-tcp-probe.py").open("a") as file:
            file.write("\n# TEST_ONLY newer invocation must not replace fixed recipe\n")
        git(self.repo, "add", ".")
        git(self.repo, "commit", "-qm", "newer recipe fixture")
        original_run = subprocess.run
        dispatched = []
        def intercept(command, **kwargs):
            if command[0] == sys.executable and "--snapshot-archive" in command:
                recipe = Path(command[1])
                self.assertEqual(recipe.read_bytes(), pinned_recipe)
                self.assertNotEqual(recipe.parent.parent, self.repo)
                archive = Path(command[command.index("--snapshot-archive") + 1])
                self.assertEqual(tcp.source_files(archive.read_bytes(), self.commit)
                                 ["tools/build-tcp-probe.py"], pinned_recipe)
                dispatched.append(command)
                return subprocess.CompletedProcess(command, 0)
            return original_run(command, **kwargs)
        with patch.object(builder, "ROOT", self.repo), patch.object(sys, "argv", [
            "builder", "amd64", str(self.root / "output"), "--source-commit", self.commit
        ]), patch.object(builder.subprocess, "run", side_effect=intercept):
            builder.main()
        self.assertEqual(len(dispatched), 1)
        self.assertEqual(dispatched[0][dispatched[0].index("--source-commit") + 1], self.commit)

    def test_elf_rejects_wrong_architecture_interpreter_and_dynamic_dependencies(self):
        tcp.verify_elf(elf("amd64"), "amd64")
        tcp.verify_elf(elf("arm64"), "arm64")
        with self.assertRaises(ValueError):
            tcp.verify_elf(elf("amd64"), "arm64")
        for kind in [3, 2]:
            data = bytearray(elf("amd64"))
            struct.pack_into("<I", data, 64, kind)
            if kind == 2:
                data += struct.pack("<qQ", 1, 0)
                struct.pack_into("<Q", data, 72, 120)
                struct.pack_into("<Q", data, 96, 16)
            with self.assertRaises(ValueError):
                tcp.verify_elf(data, "amd64")

    def test_binary_provenance_without_a_pin_is_refused(self):
        with patch.object(builder, "capture", side_effect=[
            b"apsp-tcp-probe 0.3.0\n", b"--workspace --no-rank-upload",
            json.dumps(dict(version="0.3.0", source_repo=release.REPOSITORY, source_commit=None)).encode()
        ]):
            with self.assertRaisesRegex(ValueError, "provenance"):
                builder.verify_binary(self.root / "binary", self.commit)

    def test_build_uses_the_locked_musl_target_and_embeds_the_exact_pin(self):
        target_dir = self.root / "target"
        binary = target_dir / tcp.TARGETS["amd64"] / "release" / tcp.BINARY
        binary.parent.mkdir(parents=True)
        binary.write_bytes(elf("amd64"))
        with patch.object(builder, "capture", side_effect=[
            json.dumps({"target_directory": str(target_dir)}).encode(), b"rustc fixture\n"
        ]), patch.object(builder, "verify_binary") as verify, patch.object(builder.subprocess, "run") as run:
            builder.build(self.repo, "amd64", self.commit)
        command = run.call_args.args[0]
        self.assertEqual(command[:6], ["cargo", "build", "--locked", "--release", "--package", tcp.BINARY])
        self.assertEqual(command[command.index("--target") + 1], tcp.TARGETS["amd64"])
        self.assertEqual(run.call_args.kwargs["env"]["APSP_NATIVE_TCP_SOURCE_COMMIT"], self.commit)
        self.assertIn("+crt-static", run.call_args.kwargs["env"]["RUSTFLAGS"])
        self.assertIn("-Clink-self-contained=yes", run.call_args.kwargs["env"]["RUSTFLAGS"])
        self.assertEqual(run.call_args.kwargs["env"]["CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER"], "cc")
        verify.assert_called_once_with(binary, self.commit)


class SignedTcpTests(unittest.TestCase):
    def setUp(self):
        self.fixture = test_release.ReleaseTests(methodName="runTest")
        self.fixture.setUp()
        self.repo = self.fixture.directory / "source-repo"
        self.commit = repository(self.repo)
        self.version = tcp.artifact_version(self.commit)
        for arch in tcp.TARGETS:
            _, files = payload(self.repo, self.commit, arch)
            output = self.fixture.source / "tcpquality" / self.version
            output.mkdir(parents=True, exist_ok=True)
            (output / arch).write_bytes(tcp.pack(files))
        shutil.rmtree(self.fixture.bundle)
        self.fixture.arguments.tcp_probe_version = self.version
        release.assemble(self.fixture.arguments)
        self.fixture.sign()

    def tearDown(self):
        self.fixture.tearDown()

    def rewrite_asset_and_resign(self, mutate):
        bundle = self.fixture.bundle
        metadata = json.loads((bundle / "release.json").read_text())
        entry = next(item for item in metadata["artifacts"] if item["name"] == "tcpquality" and item["arch"] == "amd64")
        files = tcp.archive_files((bundle / entry["asset_name"]).read_bytes())
        mutate(files)
        packed = tcp.pack(files)
        (bundle / entry["asset_name"]).write_bytes(packed)
        entry.update(archive_size=len(packed), binary_size=len(files[tcp.BINARY]),
                     binary_sha256=tcp.digest(files[tcp.BINARY]),
                     auxiliary_files={name: dict(size=len(data), sha256=tcp.digest(data))
                                      for name, data in files.items() if name != tcp.BINARY})
        encoded = (json.dumps(metadata, sort_keys=True, separators=(",", ":")) + "\n").encode()
        (bundle / "release.json").write_bytes(encoded)
        sums = dict((path, value) for value, path in
                    (line.split("  ", 1) for line in (bundle / "SHA256SUMS").read_text().splitlines()))
        sums["release.json"] = tcp.digest(encoded)
        sums[release.canonical_path(entry)] = tcp.digest(packed)
        (bundle / "SHA256SUMS").write_text("".join(f"{sums[path]}  {path}\n" for path in sorted(sums)))
        self.fixture.sign()

    def test_real_signature_covers_both_architectures_and_all_provenance_files(self):
        metadata = self.fixture.verify()
        entries = [entry for entry in metadata["artifacts"] if entry["name"] == "tcpquality"]
        self.assertEqual({entry["arch"] for entry in entries}, {"amd64", "arm64"})
        for entry in entries:
            self.assertEqual(entry["binary_name"], tcp.BINARY)
            self.assertEqual(set(entry["auxiliary_files"]), tcp.FILES - {tcp.BINARY})
        self.assertEqual(len(metadata["artifacts"]), 8)
        publish.require_components(metadata)

    def test_opt_in_publication_requires_both_tcp_architectures_and_all_old_modules(self):
        metadata = self.fixture.verify()
        for name, arch in [("tcpquality", "arm64"), ("agent", "amd64"), ("nodequality", "arm64")]:
            partial = dict(metadata, artifacts=[entry for entry in metadata["artifacts"]
                                               if (entry["name"], entry["arch"]) != (name, arch)])
            with self.subTest(name=name, arch=arch), self.assertRaises(ValueError):
                publish.require_components(partial)

    def test_modified_binary_and_every_auxiliary_file_fail_the_existing_signature(self):
        bundle = self.fixture.bundle
        entry = next(item for item in json.loads((bundle / "release.json").read_text())["artifacts"]
                     if item["name"] == "tcpquality" and item["arch"] == "amd64")
        asset = bundle / entry["asset_name"]
        original = asset.read_bytes()
        for name in tcp.FILES:
            with self.subTest(name=name):
                files = tcp.archive_files(original)
                files[name] += b"TEST_ONLY tampering"
                asset.write_bytes(tcp.pack(files))
                with self.assertRaises(ValueError):
                    self.fixture.verify()
        asset.write_bytes(original)

    def test_even_resigned_unknown_pin_missing_source_and_unlocked_builds_are_rejected(self):
        for operation in ["unknown_pin", "missing_source", "unlocked"]:
            with self.subTest(operation=operation):
                self.setUp_payload_from_fixture()
                def mutate(files):
                    info = json.loads(files["build-info.json"])
                    if operation == "unknown_pin":
                        info["source_commit"] = None
                    elif operation == "missing_source":
                        files.pop("source.tar.gz")
                    else:
                        info["cargo_locked"] = False
                    files["build-info.json"] = json.dumps(info).encode()
                self.rewrite_asset_and_resign(mutate)
                with self.assertRaises(ValueError):
                    self.fixture.verify()

    def setUp_payload_from_fixture(self):
        # Restore all unsigned inputs and reassemble, retaining only public test signatures.
        shutil.rmtree(self.fixture.bundle)
        release.assemble(self.fixture.arguments)
        self.fixture.sign()

    def test_signed_auxiliary_digest_does_not_replace_inner_provenance_validation(self):
        self.rewrite_asset_and_resign(lambda files: files.__setitem__("Cargo.lock", b"TEST_ONLY other lockfile"))
        with self.assertRaisesRegex(ValueError, "provenance"):
            self.fixture.verify()

    def test_even_resigned_missing_or_changed_bundled_libc_notices_are_rejected(self):
        for action in ["missing", "changed"]:
            with self.subTest(action=action):
                self.setUp_payload_from_fixture()
                def mutate(files):
                    notices = json.loads(files["THIRD_PARTY_NOTICES.txt"])
                    library = next(item for item in notices["toolchain"] if item["name"] == "Rust bundled musl libc")
                    if action == "missing":
                        notices["toolchain"].remove(library)
                    else:
                        library["notices"][0]["text"] += "TEST_ONLY forged original"
                    files["THIRD_PARTY_NOTICES.txt"] = json.dumps(notices).encode()
                    info = json.loads(files["build-info.json"])
                    info["notices_sha256"] = tcp.digest(files["THIRD_PARTY_NOTICES.txt"])
                    files["build-info.json"] = json.dumps(info).encode()
                self.rewrite_asset_and_resign(mutate)
                with self.assertRaises(ValueError):
                    self.fixture.verify()

    def test_default_release_keeps_three_modules_and_does_not_require_tcp(self):
        shutil.rmtree(self.fixture.bundle)
        self.fixture.arguments.tcp_probe_version = None
        release.assemble(self.fixture.arguments)
        self.fixture.sign()
        self.assertEqual({entry["name"] for entry in self.fixture.verify()["artifacts"]},
                         {"agent", "sing-box", "nodequality"})

    def test_even_resigned_unknown_or_ambiguous_rustc_commits_are_rejected(self):
        known = tcp.BUNDLED_MUSL_IDENTITY["rustc_commit"]
        for commits in [["unknown"], [known, "unknown"], ["unknown", known], [known, known]]:
            with self.subTest(commits=commits):
                self.setUp_payload_from_fixture()
                self.fixture.verify()
                def mutate(files):
                    rustc_info = "rustc 1.98.1 TEST_ONLY fixture\n" + "".join(
                        f"commit-hash: {commit}\n" for commit in commits)
                    notices = json.loads(files["THIRD_PARTY_NOTICES.txt"])
                    standard = next(item for item in notices["toolchain"]
                                    if item["name"] == "Rust standard library and bundled native libraries")
                    standard["version"] = rustc_info
                    files["THIRD_PARTY_NOTICES.txt"] = json.dumps(notices).encode()
                    info = json.loads(files["build-info.json"])
                    info["rustc"] = rustc_info
                    info["notices_sha256"] = tcp.digest(files["THIRD_PARTY_NOTICES.txt"])
                    files["build-info.json"] = json.dumps(info).encode()
                self.rewrite_asset_and_resign(mutate)
                with self.assertRaisesRegex(ValueError, "rustc commit"):
                    self.fixture.verify()


if __name__ == "__main__":
    unittest.main()

class NoticeInventoryTests(unittest.TestCase):
    setUp = FixedSourceTests.setUp
    tearDown = FixedSourceTests.tearDown
    def test_locked_notice_checksums_and_required_originals_are_enforced(self):
        version, files = payload(self.repo, self.commit, "amd64")
        original = files["THIRD_PARTY_NOTICES.txt"]
        for mutate in [
            lambda d: d["dependencies"][0].update(checksum="0" * 64),
            lambda d: d["dependencies"][0].update(notices=[]),
            lambda d: d.update(toolchain=[]),
            lambda d: d["dependencies"][0].update(license="MIT AND Unicode-3.0"),
        ]:
            with self.subTest(mutate=mutate):
                notices = json.loads(original)
                mutate(notices)
                files["THIRD_PARTY_NOTICES.txt"] = json.dumps(notices).encode()
                info = json.loads(files["build-info.json"])
                info["notices_sha256"] = tcp.digest(files["THIRD_PARTY_NOTICES.txt"])
                files["build-info.json"] = json.dumps(info).encode()
                with self.assertRaises(ValueError):
                    tcp.validate_files(files, version, "amd64")


class BundledMuslTests(unittest.TestCase):
    setUp = FixedSourceTests.setUp
    tearDown = FixedSourceTests.tearDown

    def test_ambiguous_rustc_identity_is_rejected_before_cargo(self):
        known = tcp.BUNDLED_MUSL_IDENTITY["rustc_commit"]
        for commits in [[known, "unknown"], ["unknown", known], [known, known]]:
            rustc_info = "rustc 1.98.1 TEST_ONLY\n" + "".join(
                f"commit-hash: {commit}\n" for commit in commits)
            with self.subTest(commits=commits), patch.object(
                    collector, "capture", return_value=rustc_info.encode()) as calls:
                with self.assertRaisesRegex(ValueError, "rustc commit"):
                    collector.collect(self.repo, "amd64")
                calls.assert_called_once_with(["rustc", "-vV"])

    def test_missing_or_changed_originals_and_unknown_toolchains_fail_before_cargo(self):
        source = {name: (self.repo / name).read_bytes() for name in tcp.BUNDLED_MUSL_FILES}
        rustc_info = "rustc 1.98.1 TEST_ONLY\ncommit-hash: " + tcp.BUNDLED_MUSL_IDENTITY["rustc_commit"] + "\n"
        self.assertEqual(tcp.bundled_musl(source, rustc_info)["version"], "1.2.5")
        for index, name in enumerate(tcp.BUNDLED_MUSL_FILES):
            changed = dict(source)
            changed[name] += b"tampered"
            with self.subTest(index=index), self.assertRaises(ValueError):
                tcp.bundled_musl(changed, rustc_info)
            del changed[name]
            with self.subTest(missing=name), self.assertRaises(ValueError):
                tcp.bundled_musl(changed, rustc_info)
        with patch.object(collector, "capture", return_value=b"rustc TEST_ONLY\ncommit-hash: unknown\n") as calls:
            with self.assertRaisesRegex(ValueError, "rustc commit"):
                collector.collect(self.repo, "amd64")
            calls.assert_called_once_with(["rustc", "-vV"])

    def test_rehashed_missing_or_forged_bundled_notices_are_rejected(self):
        version, files = payload(self.repo, self.commit, "amd64")
        original = files["THIRD_PARTY_NOTICES.txt"]
        for action in ["missing", "copyright", "recipe", "identity", "toolchain"]:
            notices = json.loads(original)
            bundled = next(item for item in notices["toolchain"] if item["name"] == "Rust bundled musl libc")
            if action == "missing":
                notices["toolchain"].remove(bundled)
            elif action == "toolchain":
                next(item for item in notices["toolchain"] if item["name"].startswith("Rust standard"))["version"] = "rustc other"
            else:
                bundled["notices"][{"copyright": 0, "recipe": 1, "identity": 2}[action]]["text"] += "forged"
            files["THIRD_PARTY_NOTICES.txt"] = json.dumps(notices).encode()
            info = json.loads(files["build-info.json"])
            info["notices_sha256"] = tcp.digest(files["THIRD_PARTY_NOTICES.txt"])
            files["build-info.json"] = json.dumps(info).encode()
            with self.subTest(action=action), self.assertRaisesRegex(ValueError, "native library|bundled musl|toolchain"):
                tcp.validate_files(files, version, "amd64")
