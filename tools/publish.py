#!/usr/bin/env python3
"""Check the tagged CI commit and recheck a complete signed self-contained Agent draft before publication."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import selectors
import shutil
import subprocess
import tempfile
import time
import urllib.parse

from release import (AGENT_TARGETS, EMBEDDED_FRAME_PREFIX, EMBEDDED_FRAME_SUFFIX,
                     EXPECTED_EMBEDDED_ABI, LINUX_ARCHIVE_COMPONENTS, MAX_AGENT_RAW,
                     REPOSITORY, SEGMENT, VERSION, bundle_source_offers, ensure,
                     load_roots, regular_file_proof, source_offer_asset_limit,
                     source_offer_url, verify_bundle, verify_embedded_agent_manifest)

REQUIRED_JOBS = frozenset((
    "check",
    "compose-smoke",
    "Agent Linux musl (amd64)",
    "Agent Linux musl (arm64)",
    "Agent Linux gnu (amd64)",
    "Agent Linux gnu (arm64)",
    "Reality installation and accounting",
))
PLATFORM_REQUIRED_JOBS = {
    "macos-arm64": ("Agent macos (arm64)",),
    "windows-amd64": ("Agent windows (amd64)",),
    "windows-arm64": ("Agent windows (arm64)",),
    "freebsd-amd64": (
        "Agent FreeBSD 13.5 (amd64)",
        "Agent FreeBSD 14 (amd64)",
        "Agent FreeBSD 15 (amd64)",
    ),
    "freebsd-arm64": (
        "Agent FreeBSD 13.5 (arm64)",
        "Agent FreeBSD 14 (arm64)",
        "Agent FreeBSD 15 (arm64)",
    ),
}
RELEASES_PER_PAGE = 100
MAX_RELEASE_PAGES = 10
MAX_RELEASE_ASSETS = 32


class Github:
    def command(self, endpoint, method="GET", fields=(), binary=False):
        command = ["gh", "api", "--hostname", "github.com", "--method", method,
                   "-H", "X-GitHub-Api-Version: 2022-11-28", "-H",
                   "Accept: application/octet-stream" if binary else
                   "Accept: application/vnd.github+json", endpoint]
        for field in fields:
            command.extend(("-F", field))
        return command

    def environment(self):
        environment = dict(os.environ)
        for name in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy",
                     "https_proxy", "all_proxy"):
            environment.pop(name, None)
        environment["GH_HOST"] = "github.com"
        return environment

    def api(self, endpoint, method="GET", fields=()):
        result = subprocess.run(self.command(endpoint, method, fields),
                                capture_output=True, timeout=120,
                                env=self.environment(), check=False)
        ensure(result.returncode == 0, "GitHub API request failed")
        ensure(len(result.stdout) <= 4 * 1024 * 1024, "GitHub response is too large")
        return json.loads(result.stdout)

    def download(self, asset, destination):
        # Select immutable asset ID, not a mutable tag/name download lookup.
        limit = source_offer_asset_limit(asset["name"])
        ensure(type(asset["size"]) is int and 0 < asset["size"] <= limit,
               "download asset size outside permitted range")
        with destination.open("xb") as output, selectors.DefaultSelector() as selector:
            process = subprocess.Popen(self.command(
                f"repos/{REPOSITORY}/releases/assets/{asset['id']}", binary=True),
                stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                env=self.environment())
            deadline, downloaded, errors = time.monotonic() + 300, 0, 0
            selector.register(process.stdout, selectors.EVENT_READ, True)
            selector.register(process.stderr, selectors.EVENT_READ, False)
            try:
                while selector.get_map():
                    remaining = deadline - time.monotonic()
                    ensure(remaining > 0, "GitHub asset download timed out")
                    for key, _ in selector.select(min(remaining, 1)):
                        content = os.read(key.fileobj.fileno(), 65536)
                        if not content:
                            selector.unregister(key.fileobj)
                            key.fileobj.close()
                        elif key.data:
                            downloaded += len(content)
                            ensure(downloaded <= asset["size"] and downloaded <= limit,
                                   "GitHub asset download exceeds the selected size")
                            output.write(content)
                        else:
                            errors += len(content)
                            ensure(errors <= 65536, "GitHub asset error output exceeds its bound")
                remaining = deadline - time.monotonic()
                ensure(remaining > 0, "GitHub asset download timed out")
                ensure(process.wait(timeout=remaining) == 0, "GitHub asset download failed")
                ensure(downloaded == asset["size"], "GitHub asset download is incomplete")
            finally:
                if process.poll() is None:
                    process.kill()
                    process.wait(timeout=5)
                process.stdout.close()
                process.stderr.close()


def tag_identity(github, tag):
    ensure(tag.startswith("agent-v") and VERSION.fullmatch(tag[7:]), "invalid release tag")
    value = github.api(f"repos/{REPOSITORY}/git/ref/tags/{urllib.parse.quote(tag, safe='')}")
    ensure(value.get("ref") == "refs/tags/" + tag, "tag reference does not match")
    initial = value["object"]
    current = initial
    for _ in range(5):
        ensure(isinstance(current, dict) and re.fullmatch(r"[0-9a-f]{40}", current["sha"]),
               "invalid Git tag object")
        if current["type"] == "commit":
            return {"ref_sha": initial["sha"], "ref_type": initial["type"],
                    "commit": current["sha"]}
        ensure(current["type"] == "tag", "tag does not point to a commit")
        current = github.api(f"repos/{REPOSITORY}/git/tags/{current['sha']}")["object"]
    raise ValueError("tag object nesting is too deep")


def extra_required_jobs(metadata):
    jobs = set()
    for entry in metadata.get("artifacts", ()):
        jobs.update(PLATFORM_REQUIRED_JOBS.get(entry.get("arch"), ()))
    return frozenset(jobs)


def required_checks(github, commit, extra_jobs=frozenset()):
    expected_jobs = REQUIRED_JOBS | frozenset(extra_jobs)
    query = urllib.parse.urlencode({"head_sha": commit, "branch": "main", "event": "push",
                                    "per_page": 100})
    runs = github.api(f"repos/{REPOSITORY}/actions/workflows/ci.yml/runs?{query}")
    ensure(type(runs.get("total_count")) is int and 0 < runs["total_count"] <= 100,
           "missing or excessive CI runs for the tagged main commit")
    candidates = [run for run in runs["workflow_runs"] if run.get("head_sha") == commit
                  and run.get("head_branch") == "main" and run.get("event") == "push"
                  and run.get("path", "").split("@")[0] == ".github/workflows/ci.yml"]
    ensure(candidates, "tagged commit has no main push CI evidence")
    run = max(candidates, key=lambda candidate: candidate["id"])
    ensure(run["status"] == "completed" and run["conclusion"] == "success",
           "latest tagged main commit CI has not passed")
    result = github.api(f"repos/{REPOSITORY}/actions/runs/{run['id']}/jobs?filter=latest&per_page=100")
    ensure(type(result.get("total_count")) is int and result["total_count"] <= 100,
           "excessive CI jobs")
    jobs = {}
    for job in result["jobs"]:
        if job["name"] in expected_jobs:
            ensure(job["name"] not in jobs, "duplicate required CI job")
            ensure(job.get("head_sha") == commit and job["status"] == "completed"
                   and job["conclusion"] == "success", "required CI job has not passed")
            jobs[job["name"]] = job["id"]
    ensure(set(jobs) == expected_jobs, "a required CI job is missing")
    return {"run_id": run["id"], "run_attempt": run["run_attempt"], "jobs": jobs}


def release_snapshot(github, tag):
    # The tag endpoint excludes drafts; scan the authenticated list to prove uniqueness.
    selected, release_ids = None, set()
    for page in range(1, MAX_RELEASE_PAGES + 1):
        values = github.api(f"repos/{REPOSITORY}/releases?per_page={RELEASES_PER_PAGE}&page={page}")
        ensure(isinstance(values, list) and len(values) <= RELEASES_PER_PAGE,
               "invalid release listing page")
        for value in values:
            ensure(isinstance(value, dict) and type(value.get("id")) is int
                   and value["id"] > 0 and value["id"] not in release_ids,
                   "invalid or duplicate release ID in listing")
            ensure(isinstance(value.get("tag_name"), str), "invalid release tag in listing")
            release_ids.add(value["id"])
            if value["tag_name"] == tag:
                ensure(selected is None, "multiple releases match the exact tag")
                selected = value
        if len(values) < RELEASES_PER_PAGE:
            break
    else:
        raise ValueError("release listing exceeds the page limit")
    ensure(selected is not None, "no release matches the exact tag")
    ensure(isinstance(selected.get("target_commitish"), str)
           and re.fullmatch(r"[0-9a-f]{40}", selected["target_commitish"]),
           "release draft must record the exact build commit")
    value = github.api(f"repos/{REPOSITORY}/releases/{selected['id']}")
    ensure(isinstance(value, dict) and type(value.get("id")) is int
           and value["id"] == selected["id"] and value.get("tag_name") == tag
           and value.get("target_commitish") == selected["target_commitish"],
           "release identity changed after listing")
    assets = github.api(f"repos/{REPOSITORY}/releases/{value['id']}/assets?per_page=100")
    ensure(isinstance(assets, list) and 0 < len(assets) <= MAX_RELEASE_ASSETS,
           "invalid release asset count")
    normalized, names, identifiers = [], set(), set()
    for asset in assets:
        ensure(type(asset["id"]) is int and asset["id"] > 0
               and asset["id"] not in identifiers, "invalid or duplicate asset ID")
        ensure(isinstance(asset["name"], str) and
               re.fullmatch(r"[0-9A-Za-z][0-9A-Za-z.+_-]*", asset["name"])
               and asset["name"] not in names, "invalid or duplicate asset name")
        ensure(type(asset["size"]) is int
               and 0 < asset["size"] <= source_offer_asset_limit(asset["name"])
               and asset["state"] == "uploaded", "incomplete or oversized asset")
        ensure(isinstance(asset["digest"], str)
               and re.fullmatch(r"sha256:[0-9a-f]{64}", asset["digest"]), "asset lacks a SHA256 digest")
        normalized.append({field: asset[field] for field in ("id", "name", "size", "digest", "state")})
        identifiers.add(asset["id"])
        names.add(asset["name"])
    return {"id": value["id"], "tag": tag, "draft": value["draft"],
            "build_commit": value["target_commitish"],
            "assets": sorted(normalized, key=lambda asset: asset["name"])}


def require_components(metadata):
    artifacts = metadata.get("artifacts")
    ensure(isinstance(artifacts, list) and len(artifacts) > 0,
           "release must contain signed Agent artifacts")
    targets = []
    for entry in artifacts:
        ensure(isinstance(entry, dict) and entry.get("name") == "agent",
               "publication must contain only self-contained raw Agent artifacts; standalone components are prohibited")
        arch = entry.get("arch")
        ensure(isinstance(arch, str) and arch in AGENT_TARGETS, "unsupported Agent target")
        expected_binary = "apsp-agent.exe" if arch.startswith("windows-") else "apsp-agent"
        ensure(entry.get("format") == "raw"
               and entry.get("binary_name") == expected_binary
               and not entry.get("auxiliary_files")
               and type(entry.get("archive_size")) is int
               and 0 < entry["archive_size"] <= MAX_AGENT_RAW
               and entry.get("binary_size") == entry["archive_size"],
               "publication Agent entry must be a bounded raw binary without auxiliary files")
        targets.append(arch)
    target_set = set(targets)
    ensure(len(targets) == len(target_set), "duplicate Agent target in release metadata")
    for arch in ("amd64", "arm64"):
        ensure(not (arch in target_set and f"linux-musl-{arch}" in target_set),
               "cannot publish duplicate Linux musl target aliases in one release")
    has_legacy_musl = {"amd64", "arm64"} <= target_set
    has_explicit_musl = {"linux-musl-amd64", "linux-musl-arm64"} <= target_set
    ensure(has_legacy_musl or has_explicit_musl,
           "release must contain Linux static musl raw Agents on both architectures")
    ensure(not any(t in target_set for t in ("amd64", "arm64")) or has_legacy_musl,
           "release must contain both amd64 and arm64 Linux static musl raw Agents")
    ensure(not any(t in target_set for t in ("linux-musl-amd64", "linux-musl-arm64")) or has_explicit_musl,
           "release must contain both linux-musl-amd64 and linux-musl-arm64 raw Agents")
    ensure({"linux-gnu-amd64", "linux-gnu-arm64"} <= target_set,
           "release must contain Linux GNU raw Agents on both architectures")
    for prefix in ("freebsd", "windows"):
        pair = {f"{prefix}-amd64", f"{prefix}-arm64"}
        if target_set & pair:
            ensure(pair <= target_set,
                   f"release with {prefix} Agent must contain both amd64 and arm64")


def verify_embedded_closure(bundle, metadata, build_commit):
    bundle = Path(bundle)
    embedded_by_target = {}
    linux_versions = {name: set() for name in LINUX_ARCHIVE_COMPONENTS}
    singbox_digests = {}

    for entry in metadata["artifacts"]:
        arch = entry["arch"]
        payload = (bundle / entry["asset_name"]).read_bytes()
        manifest = verify_embedded_agent_manifest(
            payload, entry["version"], arch, require_release_closure=True, expected_versions={}
        )
        expected_abi = EXPECTED_EMBEDDED_ABI[arch]
        components = manifest["components"]
        if expected_abi[0] == "linux":
            for name in LINUX_ARCHIVE_COMPONENTS:
                linux_versions[name].add(components[name]["version"])
            ensure(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+-" + re.escape(build_commit) + r"-r1",
                                components["tcpquality"]["version"]),
                   f"Linux Agent '{arch}' lacks build-commit-pinned built-in tcp-probe")
            cpu_arch = "arm64" if expected_abi[1] == "aarch64" else "amd64"
            singbox_digests[(expected_abi[2], cpu_arch)] = components["sing-box"]["sha256"]
        embedded_by_target[arch] = components

    for name, versions in linux_versions.items():
        ensure(len(versions) == 1,
               f"all Linux Agent targets in one release must embed the same '{name}' version")
    for cpu_arch in ("amd64", "arm64"):
        ensure(singbox_digests.get(("gnu", cpu_arch)) != singbox_digests.get(("musl", cpu_arch)),
               f"Linux GNU and musl {cpu_arch} Agents cannot share the same runtime/Cronet archive")

    offers = bundle_source_offers(bundle, metadata)
    offer_names = {offer["asset"] for offer in offers}
    singbox_ver = next(iter(linux_versions["sing-box"]))
    expected_offers = {f"sing-box-{singbox_ver}-sources.tar.gz"}
    for cpu_arch in ("amd64", "arm64"):
        for name in ("nodequality", "network-workbench", "ipquality"):
            ver = next(iter(linux_versions[name]))
            expected_offers.add(f"{name}-{ver}-linux-{cpu_arch}-sources.tar.gz")
    ensure(offer_names == expected_offers,
           "release must include exact corresponding source archives for every embedded Linux component")
    return offers, embedded_by_target


def checked_publication(github, tag, roots, minisign, publish=False):
    identity = tag_identity(github, tag)
    checks = required_checks(github, identity["commit"])
    before = release_snapshot(github, tag)
    ensure(before["draft"] is True, "only a draft can enter final publication verification")
    ensure(before["build_commit"] == identity["commit"], "tag differs from the draft build commit")
    with tempfile.TemporaryDirectory(prefix="apsp-publication-") as temporary:
        bundle = Path(temporary)
        ensure(shutil.disk_usage(bundle).free >= sum(asset["size"] for asset in before["assets"])
               + 32 * 1024 * 1024, "insufficient space for publication assets")
        for asset in before["assets"]:
            path = bundle / asset["name"]
            github.download(asset, path)
            proof = regular_file_proof(path, source_offer_asset_limit(asset["name"]))
            ensure(proof["size"] == asset["size"] and "sha256:" + proof["sha256"] == asset["digest"],
                   "download differs from selected GitHub asset ID and digest")
        metadata = verify_bundle(bundle, roots, minisign, tag)
        require_components(metadata)
        offers, embedded_components = verify_embedded_closure(bundle, metadata, identity["commit"])
    extra_jobs = extra_required_jobs(metadata)
    if extra_jobs:
        checks = required_checks(github, identity["commit"], extra_jobs)
    # Re-fetch every identity and check immediately before the only mutation.
    ensure(tag_identity(github, tag) == identity, "tag changed during release verification")
    ensure(required_checks(github, identity["commit"], extra_jobs) == checks,
           "CI evidence changed during release verification")
    ensure(release_snapshot(github, tag) == before, "release assets changed during verification")
    paired = {asset["name"]: asset for asset in before["assets"]}
    source_offers = []
    for offer in offers:
        asset = paired[offer["asset"]]
        ensure(asset["size"] == offer["size"] and asset["digest"] == "sha256:" + offer["sha256"],
               "GitHub source-offer asset differs from the signed installer declaration")
        source_offers.append({
            "asset": offer["asset"],
            "size": offer["size"],
            "sha256": offer["sha256"],
            "url": source_offer_url(tag, offer["asset"]),
        })
    evidence = {
        "tag": tag,
        "tag_identity": identity,
        "checks": checks,
        "release": before,
        "embedded_components": embedded_components,
        "source_offers": source_offers,
        "published": False,
    }
    if publish:
        result = github.api(f"repos/{REPOSITORY}/releases/{before['id']}", "PATCH",
                            ("draft=false", f"tag_name={before['tag']}",
                             f"target_commitish={before['build_commit']}"))
        ensure(result["id"] == before["id"] and result["draft"] is False, "publication request failed")
        after = release_snapshot(github, tag)
        ensure(after == dict(before, draft=False) and tag_identity(github, tag) == identity,
               "publication state changed across the final API call; manual incident review required")
        evidence["published"] = True
    return evidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    check = commands.add_parser("check-tag")
    check.add_argument("--tag", required=True)
    check.add_argument("--expected-sha", required=True)
    publish = commands.add_parser("verify-publication")
    publish.add_argument("--tag", required=True)
    publish.add_argument("--trusted-keys", required=True)
    publish.add_argument("--minisign", default="minisign")
    publish.add_argument("--publish", action="store_true")
    parser.add_argument("--evidence")
    args = parser.parse_args()
    github = Github()
    if args.command == "check-tag":
        identity = tag_identity(github, args.tag)
        ensure(identity["commit"] == args.expected_sha, "tag differs from build commit")
        evidence = {"tag_identity": identity, "checks": required_checks(github, identity["commit"])}
    else:
        roots = load_roots(args.trusted_keys, publication=True)
        evidence = checked_publication(github, args.tag, roots, args.minisign, args.publish)
    if args.evidence:
        Path(args.evidence).write_text(json.dumps(evidence, sort_keys=True) + "\n")
    print("Tagged CI commit and release publication checks passed.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError, json.JSONDecodeError, subprocess.TimeoutExpired) as error:
        raise SystemExit(f"Publication refused: {error}") from error
