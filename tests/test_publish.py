#!/usr/bin/env python3
"""Publication CI gates, self-contained raw Agent contracts, and paired source-offer checks."""

import argparse
import copy
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import urllib.parse
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/protocol/tests/fixtures"
if not FIXTURES.is_dir():
    FIXTURES = ROOT / "fixtures"
sys.path.insert(0, str(ROOT / "tools"))
sys.path.insert(0, str(ROOT / "tests"))
import publish
import release
from test_release import build_embedded_agent_manifest, encode_embedded_agent_manifest_frame

COMMIT = "a" * 40
VERSION = "0.4.0"
TAG = "agent-v" + VERSION
LINUX_TARGETS = ("amd64", "arm64", "linux-gnu-amd64", "linux-gnu-arm64")


def encode_embedded_frame(agent_version, target, commit=COMMIT, **kwargs):
    manifest = build_embedded_agent_manifest(agent_version, target, commit=commit, **kwargs)
    return (
        b"TEST ONLY raw apsp-agent executable "
        + target.encode("ascii")
        + encode_embedded_agent_manifest_frame(manifest)
    )


class FakeGithub:
    def __init__(self, bundle, extra_ci_jobs=()):
        self.bundle = bundle
        self.extra_ci_jobs = set(extra_ci_jobs)
        self.patches = []
        self.downloads = 0
        self.draft = True
        self.tag_name = TAG
        self.reassign_tag_if_omitted = False
        self.reference = {"type": "commit", "sha": COMMIT}
        self.changed_tag = False
        self.changed_assets = False
        self.changed_checks = False
        self.missing_job = None
        self.run_failed = False
        self.build_commit = COMMIT
        self.listed_releases = None
        self.release_detail_updates = {}
        self.changed_release_identity = {}
        self.api_calls = []
        self.assets = [
            {
                "id": index,
                "name": path.name,
                "state": "uploaded",
                "size": len(path.read_bytes()),
                "digest": "sha256:" + release.digest(path.read_bytes()),
            }
            for index, path in enumerate(sorted(bundle.iterdir()), start=1)
        ]

    def release_record(self):
        value = {
            "id": 7,
            "tag_name": self.tag_name,
            "draft": self.draft,
            "target_commitish": self.build_commit,
        }
        if self.downloads:
            value.update(self.changed_release_identity)
        return value

    def api(self, endpoint, method="GET", fields=()):
        self.api_calls.append((endpoint, method))
        if method == "PATCH":
            self.patches.append((endpoint, fields))
            values = dict(field.split("=", 1) for field in fields)
            fallback_tag = "untagged-fixture" if self.reassign_tag_if_omitted else self.tag_name
            self.tag_name = values.get("tag_name", fallback_tag)
            self.build_commit = values.get("target_commitish", self.build_commit)
            self.draft = False
            return self.release_record()
        if "/git/ref/tags/" in endpoint:
            reference = self.reference
            if self.changed_tag and self.downloads:
                reference = {"type": "commit", "sha": "b" * 40}
            return {"ref": "refs/tags/" + TAG, "object": copy.deepcopy(reference)}
        if "/git/tags/" in endpoint:
            return {"object": {"type": "commit", "sha": COMMIT}}
        if "/actions/workflows/ci.yml/runs?" in endpoint:
            failed = self.run_failed or (self.changed_checks and self.downloads)
            return {
                "total_count": 1,
                "workflow_runs": [{
                    "id": 5,
                    "run_attempt": 1,
                    "head_sha": COMMIT,
                    "head_branch": "main",
                    "event": "push",
                    "path": ".github/workflows/ci.yml@refs/heads/main",
                    "status": "completed",
                    "conclusion": "failure" if failed else "success",
                }],
            }
        if "/actions/runs/5/jobs?" in endpoint:
            names = sorted(set(publish.REQUIRED_JOBS) | self.extra_ci_jobs)
            if self.missing_job:
                names.remove(self.missing_job)
            jobs = [
                {
                    "id": index,
                    "name": name,
                    "status": "completed",
                    "conclusion": "success",
                    "head_sha": COMMIT,
                }
                for index, name in enumerate(names, start=1)
            ]
            return {"total_count": len(jobs), "jobs": jobs}
        if "/releases/tags/" in endpoint:
            if self.draft:
                raise ValueError("GitHub API request failed (HTTP 404)")
            return self.release_record()
        if endpoint.startswith(f"repos/{publish.REPOSITORY}/releases?"):
            query = urllib.parse.parse_qs(urllib.parse.urlsplit(endpoint).query)
            page, per_page = int(query["page"][0]), int(query["per_page"][0])
            records = self.listed_releases
            if records is None:
                records = [self.release_record()]
            return copy.deepcopy(records[(page - 1) * per_page:page * per_page])
        if endpoint == f"repos/{publish.REPOSITORY}/releases/{self.release_record()['id']}":
            return dict(self.release_record(), **self.release_detail_updates)
        if f"/releases/{self.release_record()['id']}/assets?" in endpoint:
            assets = copy.deepcopy(self.assets)
            if self.changed_assets and self.downloads:
                assets[0]["id"] += 100
            return assets
        raise AssertionError("unexpected fake API endpoint")

    def download(self, asset, destination):
        destination.write_bytes((self.bundle / asset["name"]).read_bytes())
        self.downloads += 1


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="apsp-publish-test-")
        self.directory = Path(self.temporary.name)
        self.source = self.directory / "artifacts"
        self.installer = self.directory / "installer.sh"
        self.installer.write_text(
            "#!/bin/sh\n# APSP_BOOTSTRAP_AGENT_SOURCE=preloaded-github-v1\nexit 0\n",
            encoding="utf-8",
        )
        self.roots = release.load_roots(FIXTURES / "public-keys.json")
        self.write_source_inputs()
        self.bundle = self.assemble_and_sign("release", targets=LINUX_TARGETS)
        self.github = FakeGithub(self.bundle)

    def tearDown(self):
        self.temporary.cleanup()

    def write_source_inputs(self, targets=LINUX_TARGETS, **frame_kwargs):
        agent_dir = self.source / "agent" / VERSION
        agent_dir.mkdir(parents=True, exist_ok=True)
        for target in targets:
            (agent_dir / target).write_bytes(
                encode_embedded_frame(VERSION, target, **frame_kwargs)
            )
        sb_dir = self.source / "sing-box" / release.SINGBOX_VERSION
        sb_dir.mkdir(parents=True, exist_ok=True)
        (sb_dir / f"sing-box-{release.SINGBOX_VERSION}-sources.tar.gz").write_bytes(
            b"TEST ONLY corresponding sing-box source archive"
        )
        for arch in ("amd64", "arm64"):
            for component, ver in (
                ("nodequality", release.NODEQUALITY_VERSION),
                ("network-workbench", release.WORKBENCH_VERSION),
                ("ipquality", release.IPQUALITY_DEFAULT_VERSION),
            ):
                comp_dir = self.source / component / ver
                comp_dir.mkdir(parents=True, exist_ok=True)
                (comp_dir / f"{arch}.sources.tar.gz").write_bytes(
                    f"TEST ONLY corresponding {component} {arch} source archive".encode("utf-8")
                )

    def sign(self, bundle):
        result = subprocess.run(
            [
                "minisign",
                "-S",
                "-m",
                str(bundle / "SHA256SUMS"),
                "-s",
                str(FIXTURES / "TEST_ONLY.key"),
                "-x",
                str(bundle / "SHA256SUMS.minisig"),
                "-t",
                "Apsp TEST ONLY automated fixture",
            ],
            capture_output=True,
            check=False,
        )
        self.assertEqual(result.returncode, 0, "fixture signing failed")

    def assemble_and_sign(self, output_name, targets=LINUX_TARGETS, require_paired=True):
        output = self.directory / output_name
        arguments = argparse.Namespace(
            source=str(self.source),
            output=str(output),
            agent_version=VERSION,
            tag=TAG,
            installer=str(self.installer),
            target=list(targets),
            include_present_targets=False,
            paired_source=[],
            no_auto_paired_sources=False,
            require_paired_sources=require_paired,
        )
        release.assemble(arguments)
        self.sign(output)
        return output

    def verify(self, publish_release=False):
        return publish.checked_publication(
            self.github, TAG, self.roots, "minisign", publish_release
        )

    def test_self_contained_release_rejects_standalone_optional_components(self):
        base = json.loads((self.bundle / "release.json").read_text(encoding="utf-8"))
        publish.require_components(base)
        for standalone in ("sing-box", "nodequality", "tcpquality", "ipquality"):
            with self.subTest(standalone=standalone):
                mutated = copy.deepcopy(base)
                mutated["artifacts"].append({
                    "name": standalone,
                    "version": "1.0.0",
                    "arch": "amd64",
                    "format": "tar.gz",
                    "binary_name": standalone,
                    "archive_size": 16,
                    "binary_size": 16,
                    "binary_sha256": "0" * 64,
                    "asset_name": f"{standalone}-1.0.0-linux-amd64.tar.gz",
                })
                with self.assertRaisesRegex(ValueError, "standalone components are prohibited"):
                    publish.require_components(mutated)

    def test_release_requires_complete_linux_musl_and_gnu_pairs_and_optional_platform_pairs(self):
        base = json.loads((self.bundle / "release.json").read_text(encoding="utf-8"))
        for omitted in ("amd64", "arm64", "linux-gnu-amd64", "linux-gnu-arm64"):
            with self.subTest(omitted=omitted):
                mutated = copy.deepcopy(base)
                mutated["artifacts"] = [
                    entry for entry in mutated["artifacts"] if entry["arch"] != omitted
                ]
                with self.assertRaises(ValueError):
                    publish.require_components(mutated)

        for prefix in ("freebsd", "windows"):
            with self.subTest(prefix=prefix):
                binary_name = "apsp-agent.exe" if prefix == "windows" else "apsp-agent"
                mutated = copy.deepcopy(base)
                mutated["artifacts"].append({
                    "name": "agent",
                    "version": VERSION,
                    "arch": f"{prefix}-amd64",
                    "format": "raw",
                    "binary_name": binary_name,
                    "archive_size": 64,
                    "binary_size": 64,
                    "binary_sha256": "1" * 64,
                    "asset_name": f"agent-{VERSION}-{prefix}-amd64",
                })
                with self.assertRaisesRegex(ValueError, "both amd64 and arm64"):
                    publish.require_components(mutated)
                mutated["artifacts"].append({
                    "name": "agent",
                    "version": VERSION,
                    "arch": f"{prefix}-arm64",
                    "format": "raw",
                    "binary_name": binary_name,
                    "archive_size": 64,
                    "binary_size": 64,
                    "binary_sha256": "2" * 64,
                    "asset_name": f"agent-{VERSION}-{prefix}-arm64",
                })
                publish.require_components(mutated)

    def test_native_platform_targets_require_matching_tagged_main_ci_jobs(self):
        targets = (*LINUX_TARGETS, "macos-arm64")
        self.write_source_inputs(targets=targets)
        bundle = self.assemble_and_sign("with-macos", targets=targets)
        self.github = FakeGithub(bundle)
        with self.assertRaisesRegex(ValueError, "required CI job is missing"):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

        self.github = FakeGithub(bundle, extra_ci_jobs=("Agent macos (arm64)",))
        evidence = self.verify()
        self.assertIn("Agent macos (arm64)", evidence["checks"]["jobs"])
        self.assertIn("macos-arm64", evidence["embedded_components"])

    def tampered_signed_bundle(self, output_name, target, payload):
        bundle = self.directory / output_name
        bundle.mkdir(parents=True)
        for item in self.bundle.iterdir():
            (bundle / item.name).write_bytes(item.read_bytes())
        metadata = json.loads((bundle / "release.json").read_text(encoding="utf-8"))
        entry = next(item for item in metadata["artifacts"] if item["arch"] == target)
        (bundle / entry["asset_name"]).write_bytes(payload)
        entry.update(
            archive_size=len(payload),
            binary_size=len(payload),
            binary_sha256=release.digest(payload),
        )
        encoded_meta = (
            json.dumps(metadata, sort_keys=True, separators=(",", ":")) + "\n"
        ).encode("utf-8")
        (bundle / "release.json").write_bytes(encoded_meta)
        rows = dict(
            line.split("  ", 1)[::-1]
            for line in (bundle / "SHA256SUMS").read_text(encoding="utf-8").splitlines()
        )
        rows[release.canonical_path(entry)] = release.digest(payload)
        rows["release.json"] = release.digest(encoded_meta)
        (bundle / "SHA256SUMS").write_text(
            "".join(f"{rows[path]}  {path}\n" for path in sorted(rows)),
            encoding="utf-8",
        )
        self.sign(bundle)
        return bundle

    def test_embedded_abi_cronet_or_commit_mismatch_blocks_publication(self):
        agent_dir = self.source / "agent" / VERSION

        # Case 1: linux-gnu-amd64 binary accidentally built with musl target_env.
        bad_abi_payload = encode_embedded_frame(
            VERSION, "linux-gnu-amd64", abi_override=("linux", "x86_64", "musl")
        )
        (agent_dir / "linux-gnu-amd64").write_bytes(bad_abi_payload)
        with self.assertRaisesRegex(ValueError, "embedded Agent ABI"):
            self.assemble_and_sign("bad-abi-rejected-at-assemble")
        bad_abi = self.tampered_signed_bundle("bad-abi", "linux-gnu-amd64", bad_abi_payload)
        self.github = FakeGithub(bad_abi)
        with self.assertRaisesRegex(ValueError, "embedded Agent ABI"):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

        # Case 2: linux-gnu-amd64 and amd64 (musl) share identical sing-box archive digest.
        shared_sha = "c" * 64
        (agent_dir / "amd64").write_bytes(
            encode_embedded_frame(VERSION, "amd64", singbox_sha=shared_sha)
        )
        (agent_dir / "linux-gnu-amd64").write_bytes(
            encode_embedded_frame(VERSION, "linux-gnu-amd64", singbox_sha=shared_sha)
        )
        same_runtime = self.assemble_and_sign("same-runtime")
        self.github = FakeGithub(same_runtime)
        with self.assertRaisesRegex(ValueError, "cannot share the same runtime/Cronet archive"):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

        # Case 3: built-in tcp-probe commit pin differs from tagged build commit.
        self.write_source_inputs(commit="d" * 40)
        wrong_commit = self.assemble_and_sign("wrong-tcp-commit")
        self.github = FakeGithub(wrong_commit)
        with self.assertRaisesRegex(ValueError, "build-commit-pinned built-in tcp-probe"):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

        # Case 4: Linux raw Agent missing one required embedded component.
        self.write_source_inputs()
        missing_ipq_payload = encode_embedded_frame(
            VERSION, "arm64", omit_component="ipquality"
        )
        (agent_dir / "arm64").write_bytes(missing_ipq_payload)
        with self.assertRaisesRegex(ValueError, "lacks verified embedded 'ipquality'"):
            self.assemble_and_sign("missing-ipq-rejected-at-assemble")
        missing_ipq = self.tampered_signed_bundle("missing-ipq", "arm64", missing_ipq_payload)
        self.github = FakeGithub(missing_ipq)
        with self.assertRaisesRegex(ValueError, "lacks verified embedded 'ipquality'"):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

    def test_paired_sources_publish_without_polluting_legacy_agent_checksums(self):
        checksums = (self.bundle / "SHA256SUMS").read_text(encoding="utf-8")
        self.assertNotIn("sources.tar.gz", checksums)
        evidence = self.verify()
        self.assertFalse(evidence["published"])
        self.assertEqual(self.github.patches, [])
        self.assertEqual(len(evidence["source_offers"]), 7)
        expected_names = {
            f"sing-box-{release.SINGBOX_VERSION}-sources.tar.gz",
            *(
                f"{name}-{ver}-linux-{arch}-sources.tar.gz"
                for arch in ("amd64", "arm64")
                for name, ver in (
                    ("nodequality", release.NODEQUALITY_VERSION),
                    ("network-workbench", release.WORKBENCH_VERSION),
                    ("ipquality", release.IPQUALITY_DEFAULT_VERSION),
                )
            ),
        }
        self.assertEqual({item["asset"] for item in evidence["source_offers"]}, expected_names)
        for item in evidence["source_offers"]:
            self.assertEqual(
                item["url"],
                f"https://github.com/{release.REPOSITORY}/releases/download/{TAG}/{item['asset']}",
            )

    def test_github_source_digest_cannot_replace_the_signed_installer_source_declaration(self):
        target_name = f"sing-box-{release.SINGBOX_VERSION}-sources.tar.gz"
        (self.bundle / target_name).write_bytes(b"replaced corresponding source archive")
        # GitHub's current digest legitimately describes the replacement. The signed
        # # APSP_PAIRED_SOURCE header in install.sh still authenticates the original bytes.
        self.github = FakeGithub(self.bundle)
        with self.assertRaisesRegex(ValueError, "signed declaration"):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

    def test_missing_paired_source_blocks_publication(self):
        target_name = f"network-workbench-{release.WORKBENCH_VERSION}-linux-arm64-sources.tar.gz"
        (self.bundle / target_name).unlink()
        self.github = FakeGithub(self.bundle)
        with self.assertRaisesRegex(ValueError, "ordinary file"):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

    def test_only_canonical_source_offer_names_receive_the_larger_download_bound(self):
        offer_name = f"ipquality-{release.IPQUALITY_DEFAULT_VERSION}-linux-amd64-sources.tar.gz"
        item = next(asset for asset in self.github.assets if asset["name"] == offer_name)
        item["size"] = release.MAX_BINARY + 1
        self.assertEqual(publish.release_snapshot(self.github, TAG)["id"], 7)
        item["name"] = "untrusted-sources.tar.gz"
        self.refuse_before_download("oversized asset")
        self.assertGreater(release.source_offer_asset_limit(offer_name), release.MAX_BINARY)
        self.assertEqual(
            release.source_offer_asset_limit(f"sing-box-{release.SINGBOX_VERSION}-sources.tar.gz"),
            release.MAX_PAIRED_SOURCE,
        )
        self.assertEqual(
            release.source_offer_asset_limit("untrusted-sources.tar.gz"),
            release.MAX_BINARY,
        )

    def test_real_download_pipe_limits_selected_size_and_error_output(self):
        github = publish.Github()
        asset = {"id": 1, "name": "fixture", "size": 8}
        for index, program in enumerate(("import sys; sys.stdout.buffer.write(b'x'*64)",
                                         "import sys; sys.stderr.buffer.write(b'e'*70000)")):
            with self.subTest(program=program):
                target = self.directory / f"bounded-download-{index}"
                with patch.object(github, "command", return_value=[sys.executable, "-c", program]):
                    with self.assertRaises(ValueError):
                        github.download(asset, target)
                self.assertLessEqual(target.stat().st_size, asset["size"])

    def test_verified_draft_remains_draft_without_publish_flag(self):
        evidence = self.verify()
        self.assertFalse(evidence["published"])
        # 4 proof/installer files + 4 Linux raw Agents + 7 corresponding source archives = 15 assets.
        self.assertEqual(len(evidence["release"]["assets"]), 15)
        self.assertEqual(self.github.patches, [])

    def test_draft_tag_endpoint_is_unavailable_but_id_lookup_succeeds(self):
        with self.assertRaisesRegex(ValueError, "HTTP 404"):
            self.github.api(f"repos/{publish.REPOSITORY}/releases/tags/{TAG}")
        self.github.api_calls.clear()
        evidence = self.verify()
        self.assertEqual(evidence["release"]["id"], 7)
        self.assertIn((f"repos/{publish.REPOSITORY}/releases/7", "GET"), self.github.api_calls)
        self.assertFalse(any("/releases/tags/" in endpoint
                             for endpoint, _ in self.github.api_calls))

    def other_releases(self, count):
        return [dict(self.github.release_record(), id=100 + index,
                     tag_name=TAG + f".{index}") for index in range(count)]

    def refuse_before_download(self, message):
        with self.assertRaisesRegex(ValueError, message):
            self.verify(True)
        self.assertEqual(self.github.downloads, 0)
        self.assertEqual(self.github.patches, [])

    def test_exact_tag_on_second_page_ignores_similar_tags(self):
        self.github.listed_releases = self.other_releases(publish.RELEASES_PER_PAGE)
        self.github.listed_releases.append(self.github.release_record())
        self.assertEqual(self.verify()["release"]["id"], 7)
        self.assertIn((f"repos/{publish.REPOSITORY}/releases?per_page=100&page=2", "GET"),
                      self.github.api_calls)

    def test_published_duplicate_tag_on_later_page_refuses_publication(self):
        self.github.listed_releases = [self.github.release_record()]
        self.github.listed_releases += self.other_releases(publish.RELEASES_PER_PAGE - 1)
        self.github.listed_releases.append(dict(self.github.release_record(), id=8, draft=False))
        self.refuse_before_download("multiple releases match the exact tag")

    def test_repeated_release_id_across_pages_refuses_publication(self):
        self.github.listed_releases = self.other_releases(publish.RELEASES_PER_PAGE)
        self.github.listed_releases += [self.github.release_record(),
                                       self.github.listed_releases[0]]
        self.refuse_before_download("duplicate release ID")

    def test_full_final_page_refuses_incomplete_scan_even_after_match(self):
        self.github.listed_releases = self.other_releases(
            publish.RELEASES_PER_PAGE * publish.MAX_RELEASE_PAGES)
        self.github.listed_releases[0] = self.github.release_record()
        self.refuse_before_download("release listing exceeds the page limit")
        pages = [endpoint for endpoint, _ in self.github.api_calls if "/releases?" in endpoint]
        self.assertEqual(len(pages), publish.MAX_RELEASE_PAGES)

    def test_missing_exact_tag_refuses_publication(self):
        for records in ([], self.other_releases(1)):
            with self.subTest(records=records):
                self.github.listed_releases = records
                self.refuse_before_download("no release matches the exact tag")

    def test_malformed_listing_refuses_before_asset_lookup(self):
        record = self.github.release_record()
        pages = [None, {}, [None], [dict(record, tag_name=None)],
                 self.other_releases(publish.RELEASES_PER_PAGE + 1)]
        pages += [[dict(record, id=value)] for value in (True, "7", 0, -1)]
        for page in pages:
            with self.subTest(page=page):
                with patch.object(self.github, "api", return_value=page) as api:
                    with self.assertRaises(ValueError):
                        publish.release_snapshot(self.github, TAG)
                    self.assertEqual(api.call_count, 1)
                self.assertEqual(self.github.patches, [])

    def test_listed_build_commit_requires_full_sha(self):
        for commit in ("main", COMMIT[:7], None, True):
            with self.subTest(commit=commit):
                self.github.build_commit = commit
                self.refuse_before_download("release draft must record the exact build commit")

    def test_id_lookup_rechecks_listed_release_identity(self):
        for updates in ({"id": 8}, {"id": "7"}, {"tag_name": TAG + "-rc.1"},
                        {"target_commitish": "b" * 40}, {"target_commitish": COMMIT[:7]},
                        {"target_commitish": "main"}, {"target_commitish": None}):
            with self.subTest(updates=updates):
                self.github.release_detail_updates = updates
                self.refuse_before_download("release identity changed after listing")

    def test_changed_release_identity_during_verification_refuses_publication(self):
        for updates in ({"id": 8}, {"tag_name": TAG + "-rc.1"},
                        {"target_commitish": "b" * 40}):
            with self.subTest(updates=updates):
                self.github = FakeGithub(self.bundle)
                self.github.changed_release_identity = updates
                with self.assertRaises(ValueError):
                    self.verify(True)
                self.assertGreater(self.github.downloads, 0)
                self.assertEqual(self.github.patches, [])

    def test_new_duplicate_tag_during_download_refuses_publication(self):
        self.github.listed_releases = [self.github.release_record()]
        download = self.github.download

        def add_duplicate(asset, destination):
            download(asset, destination)
            if self.github.downloads == 1:
                self.github.listed_releases.append(dict(self.github.release_record(), id=8))

        with patch.object(self.github, "download", side_effect=add_duplicate):
            with self.assertRaisesRegex(ValueError, "multiple releases match the exact tag"):
                self.verify(True)
        self.assertEqual(self.github.patches, [])

    def test_publish_is_one_mutation_after_all_checks(self):
        evidence = self.verify(True)
        self.assertTrue(evidence["published"])
        self.assertEqual(len(self.github.patches), 1)
        self.assertEqual(self.github.patches[0][1],
                         ("draft=false", f"tag_name={TAG}", f"target_commitish={COMMIT}"))

    def test_publish_preserves_identity_when_omitted_tag_is_reassigned(self):
        self.github.reassign_tag_if_omitted = True
        evidence = self.verify(True)
        self.assertTrue(evidence["published"])
        self.assertEqual(self.github.release_record()["tag_name"], TAG)
        self.assertEqual(self.github.release_record()["target_commitish"], COMMIT)

    def test_concurrent_changes_after_patch_still_refuse_success(self):
        for change in ("release_id", "release_tag", "build_commit", "git_tag", "asset_id"):
            with self.subTest(change=change):
                self.github = FakeGithub(self.bundle)
                api = self.github.api

                def change_after_patch(endpoint, method="GET", fields=()):
                    response = api(endpoint, method, fields)
                    if method == "PATCH":
                        if change == "release_id":
                            self.github.changed_release_identity = {"id": 8}
                        elif change == "release_tag":
                            self.github.changed_release_identity = {"tag_name": TAG + "-rc.1"}
                        elif change == "build_commit":
                            self.github.changed_release_identity = {"target_commitish": "b" * 40}
                        elif change == "git_tag":
                            self.github.reference = {"type": "commit", "sha": "b" * 40}
                        else:
                            self.github.assets[0]["id"] += 100
                    return response

                with patch.object(self.github, "api", side_effect=change_after_patch):
                    with self.assertRaises(ValueError):
                        self.verify(True)
                self.assertEqual(len(self.github.patches), 1)

    def test_changed_asset_id_refuses_publication(self):
        self.github.changed_assets = True
        with self.assertRaises(ValueError):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

    def test_changed_tag_refuses_publication(self):
        self.github.changed_tag = True
        with self.assertRaises(ValueError):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

    def test_changed_ci_refuses_publication(self):
        self.github.changed_checks = True
        with self.assertRaises(ValueError):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

    def test_missing_required_reality_check_refuses_publication(self):
        self.github.missing_job = "Reality installation and accounting"
        self.refuse_before_download("required CI job is missing")

    def test_missing_required_gnu_linux_check_refuses_publication(self):
        self.github.missing_job = "Agent Linux gnu (arm64)"
        self.refuse_before_download("required CI job is missing")

    def test_failed_main_ci_refuses_before_download(self):
        self.github.run_failed = True
        self.refuse_before_download("latest tagged main commit CI has not passed")

    def test_tag_must_match_the_recorded_draft_build_commit(self):
        self.github.build_commit = "b" * 40
        self.refuse_before_download("tag differs from the draft build commit")

    def test_signed_single_arch_ci_bundle_cannot_be_published(self):
        single = self.assemble_and_sign("single-arch", targets=("amd64",), require_paired=False)
        self.github = FakeGithub(single)
        with self.assertRaises(ValueError):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

    def test_matching_github_digest_does_not_replace_release_signature(self):
        target = self.bundle / f"agent-{VERSION}-linux-musl-amd64"
        target.write_bytes(target.read_bytes() + b"tampered")
        self.github = FakeGithub(self.bundle)
        with self.assertRaises(ValueError):
            self.verify(True)
        self.assertEqual(self.github.patches, [])

    def test_annotated_tag_resolves_exact_build_commit(self):
        self.github.reference = {"type": "tag", "sha": "c" * 40}
        self.assertEqual(publish.tag_identity(self.github, TAG),
                         {"ref_type": "tag", "ref_sha": "c" * 40, "commit": COMMIT})


if __name__ == "__main__":
    unittest.main()
