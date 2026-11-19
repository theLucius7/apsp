#!/usr/bin/env python3
"""Assemble self-contained Agent releases and verify signed release bundles."""

import argparse
import base64
import contextlib
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tarfile

REPOSITORY = "theLucius7/apsp"
TEST_ONLY_PUBLIC_KEY = "RWS3NbDikg3VqWRlxJMUyaB1dTvErk0ptJ695xQ50Kyb+MmtynMhN/lq"
TEST_ONLY_ROTATION_PUBLIC_KEY = "RWRURVNUUk9UMjMuvo0ny3Mjs6QBwcE7XdZLzMDhDs2hwrXRGgN3moXl"
SOURCE_ROOT = Path(__file__).resolve().parents[1]
TEST_PUBLIC_KEY_DIRS = (SOURCE_ROOT / "fixtures", SOURCE_ROOT / "crates/protocol/tests/fixtures")
SINGBOX_VERSION = "1.14.4-apsp.2"
NODEQUALITY_VERSION = "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r22"
IPQUALITY_DEFAULT_VERSION = "87397e2c3196ec796f5477c83343c2354df601ea-node-r1"
WORKBENCH_VERSION = "1.0.0"
TCPQUALITY_ENGINE_VERSION = "0.3.0"
AGENT_TARGETS = (
    "amd64", "arm64", "linux-gnu-amd64", "linux-gnu-arm64",
    "linux-musl-amd64", "linux-musl-arm64", "macos-arm64",
    "freebsd-amd64", "freebsd-arm64", "windows-amd64", "windows-arm64",
)
LINUX_ARCHIVE_COMPONENTS = ("sing-box", "nodequality", "ipquality", "network-workbench")
LINUX_REQUIRED_EMBEDDED_COMPONENTS = LINUX_ARCHIVE_COMPONENTS + ("tcpquality",)
EXPECTED_EMBEDDED_ABI = {
    "amd64": ("linux", "x86_64", "musl"),
    "arm64": ("linux", "aarch64", "musl"),
    "linux-musl-amd64": ("linux", "x86_64", "musl"),
    "linux-musl-arm64": ("linux", "aarch64", "musl"),
    "x86_64-unknown-linux-musl": ("linux", "x86_64", "musl"),
    "aarch64-unknown-linux-musl": ("linux", "aarch64", "musl"),
    "linux-gnu-amd64": ("linux", "x86_64", "gnu"),
    "linux-gnu-arm64": ("linux", "aarch64", "gnu"),
    "x86_64-unknown-linux-gnu": ("linux", "x86_64", "gnu"),
    "aarch64-unknown-linux-gnu": ("linux", "aarch64", "gnu"),
    "macos-arm64": ("macos", "aarch64", ""),
    "aarch64-apple-darwin": ("macos", "aarch64", ""),
    "freebsd-amd64": ("freebsd", "x86_64", ""),
    "freebsd-arm64": ("freebsd", "aarch64", ""),
    "x86_64-unknown-freebsd": ("freebsd", "x86_64", ""),
    "aarch64-unknown-freebsd": ("freebsd", "aarch64", ""),
    "windows-amd64": ("windows", "x86_64", "msvc"),
    "windows-arm64": ("windows", "aarch64", "msvc"),
    "x86_64-pc-windows-msvc": ("windows", "x86_64", "msvc"),
    "aarch64-pc-windows-msvc": ("windows", "aarch64", "msvc"),
}
EMBEDDED_FRAME_PREFIX = b"\x00APSP_EMBEDDED_" + b"COMPONENTS_V1:"
EMBEDDED_FRAME_SUFFIX = b"\n:END_APSP_EMBEDDED_" + b"COMPONENTS_V1\x00"
SEGMENT = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+_-]{0,127}\Z")
VERSION = re.compile(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?\Z")
MAX_AGENT_RAW = 128 * 1024 * 1024
MAX_BINARY = 256 * 1024 * 1024
MAX_PAIRED_SOURCE = 1024 * 1024 * 1024
PAIRED_SOURCE_HEADER_RE = re.compile(
    r"# APSP_PAIRED_SOURCE: ([0-9a-f]{64}) ([1-9][0-9]{0,11}) ([0-9A-Za-z.+_-]+-sources\.tar\.gz)\Z"
)
EMBEDDED_SOURCE_ASSET_RE = re.compile(
    r"(?:sing-box-[0-9A-Za-z][0-9A-Za-z.+_-]{0,127}-sources\.tar\.gz"
    r"|(?:nodequality|network-workbench)-[0-9A-Za-z][0-9A-Za-z.+_-]{0,127}-linux-(?:amd64|arm64)-sources\.tar\.gz)\Z"
)


def ensure(condition, message):
    if not condition:
        raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read_regular(path, limit=MAX_BINARY):
    ensure(path.is_file() and not path.is_symlink(), "asset must be an ordinary file")
    ensure(0 < path.stat().st_size <= limit, "asset size outside permitted range")
    return path.read_bytes()


def regular_file_proof(path, limit=MAX_BINARY, destination=None):
    """Hash or copy a bounded ordinary asset without loading the asset into memory."""
    path = Path(path)
    ensure(path.is_file() and not path.is_symlink(), "asset must be an ordinary file")
    before = path.lstat()
    ensure(stat.S_ISREG(before.st_mode) and before.st_nlink == 1 and 0 < before.st_size <= limit,
           "asset size or identity outside permitted range")
    identity = lambda v: (v.st_dev, v.st_ino, v.st_mode, v.st_nlink, v.st_size, v.st_mtime_ns, v.st_ctime_ns)
    flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_NONBLOCK", 0) | getattr(os, "O_CLOEXEC", 0)
    value, consumed = hashlib.sha256(), 0
    with os.fdopen(os.open(path, flags), "rb") as source:
        ensure(identity(os.fstat(source.fileno())) == identity(before), "asset changed before reading")
        target = Path(destination).open("xb") if destination is not None else contextlib.nullcontext()
        with target as output:
            while content := source.read(1024 * 1024):
                consumed += len(content)
                ensure(consumed <= before.st_size and consumed <= limit, "asset grew while reading")
                value.update(content)
                if output is not None:
                    output.write(content)
        ensure(consumed == before.st_size and identity(os.fstat(source.fileno())) == identity(before)
               and identity(path.lstat()) == identity(before), "asset changed while reading")
    return {"sha256": value.hexdigest(), "size": consumed}


def source_offer_asset_limit(name):
    """Only canonical paired corresponding-source archives may exceed the binary bound."""
    if not isinstance(name, str):
        return MAX_BINARY
    if EMBEDDED_SOURCE_ASSET_RE.fullmatch(name):
        return MAX_PAIRED_SOURCE
    if not re.fullmatch(r"ipquality-[0-9A-Za-z.+_-]+-linux-(?:amd64|arm64)-sources\.tar\.gz", name):
        return MAX_BINARY
    from ipquality_artifact import MAX_SOURCE_OFFER, VERSION as IPQUALITY_VERSION
    if name in {f"ipquality-{IPQUALITY_VERSION}-linux-{arch}-sources.tar.gz" for arch in ("amd64", "arm64")}:
        return MAX_SOURCE_OFFER
    return MAX_BINARY


def source_offer_url(tag, asset):
    ensure(isinstance(tag, str) and tag.startswith("agent-v") and VERSION.fullmatch(tag[7:]),
           "invalid source-offer release tag")
    ensure(source_offer_asset_limit(asset) != MAX_BINARY, "invalid source-offer asset identity")
    return f"https://github.com/{REPOSITORY}/releases/download/{tag}/{asset}"


def parse_paired_source_headers(installer_bytes):
    offers, seen = [], set()
    for line in installer_bytes.decode("utf-8").splitlines():
        if not line.startswith("# APSP_PAIRED_SOURCE"):
            continue
        match = PAIRED_SOURCE_HEADER_RE.fullmatch(line)
        ensure(match is not None, "malformed paired source declaration in installer")
        sha256, raw_size, asset = match.groups()
        size, limit = int(raw_size), source_offer_asset_limit(asset)
        ensure(limit != MAX_BINARY and 0 < size <= limit and asset not in seen,
               "invalid or duplicate paired source declaration")
        seen.add(asset)
        offers.append({"asset": asset, "sha256": sha256, "size": size})
    ensure([item["asset"] for item in offers] == sorted(seen),
           "paired source declarations must be sorted by asset name")
    return offers


def inject_paired_source_headers(installer_bytes, offers):
    ensure(b"APSP_PAIRED_SOURCE" not in installer_bytes, "installer already contains paired source declarations")
    if not offers:
        return installer_bytes
    ordered = sorted(offers, key=lambda item: item["asset"])
    ensure(len({item["asset"] for item in ordered}) == len(ordered), "duplicate paired source asset")
    headers = [f"# APSP_PAIRED_SOURCE: {i['sha256']} {i['size']} {i['asset']}\n" for i in ordered]
    lines = installer_bytes.decode("utf-8").splitlines(keepends=True)
    marker = "# APSP_BOOTSTRAP_AGENT_SOURCE=preloaded-github-v1\n"
    insert_at = lines.index(marker) + 1 if marker in lines else 1
    rendered = "".join(lines[:insert_at] + headers + lines[insert_at:]).encode("utf-8")
    ensure(len(rendered) <= 256 * 1024, "installer with paired source headers exceeds limit")
    return rendered


def bundle_source_offers(bundle, metadata=None):
    bundle = Path(bundle)
    if metadata is None:
        metadata = json.loads(read_regular(bundle / "release.json", 32768))
    by_asset = {o["asset"]: o for o in parse_paired_source_headers(read_regular(bundle / "install.sh", 256 * 1024))}
    for entry in metadata.get("artifacts", ()):
        if entry.get("name") == "ipquality":
            from ipquality_artifact import archive_files, source_offer
            offer = source_offer(archive_files(read_regular(bundle / entry["asset_name"])),
                                 entry["version"], entry["arch"])
            ensure(by_asset.get(offer["asset"], offer) == offer, "conflicting IPQuality paired source declarations")
            by_asset[offer["asset"]] = offer
    return [by_asset[name] for name in sorted(by_asset)]


def _strict_json_object(pairs):
    result = {}
    for key, value in pairs:
        ensure(isinstance(key, str) and key not in result,
               f"duplicate JSON key in embedded component manifest: {key}")
        result[key] = value
    return result


def _reject_json_constant(token):
    raise ValueError(f"invalid JSON constant in embedded component manifest: {token}")


def extract_embedded_agent_manifest(payload):
    ensure(isinstance(payload, (bytes, bytearray)), "Agent binary payload must be bytes")
    matches, cursor = [], 0
    while (index := payload.find(EMBEDDED_FRAME_PREFIX, cursor)) >= 0:
        cursor = index + 1
        header_end = index + 103
        ensure(header_end <= len(payload) and payload[index + 37:index + 38] == b":"
               and payload[header_end - 1:header_end] == b"\n",
               "malformed embedded component manifest frame header")
        try:
            raw_len = payload[index + 29:index + 37].decode("ascii")
            declared_sha = payload[index + 38:index + 102].decode("ascii")
        except UnicodeDecodeError as error:
            raise ValueError("non-ASCII embedded component manifest frame header") from error
        ensure(re.fullmatch(r"[0-9a-f]{8}", raw_len) and re.fullmatch(r"[0-9a-f]{64}", declared_sha),
               "invalid hex fields in embedded component manifest frame header")
        json_len = int(raw_len, 16)
        body_end = header_end + json_len
        ensure(1 <= json_len <= 65536
               and payload[body_end:body_end + len(EMBEDDED_FRAME_SUFFIX)] == EMBEDDED_FRAME_SUFFIX,
               "malformed or truncated embedded component manifest frame")
        body = bytes(payload[header_end:body_end])
        ensure(digest(body) == declared_sha, "embedded component manifest frame digest mismatch")
        try:
            decoded = body.decode("utf-8")
        except UnicodeDecodeError as error:
            raise ValueError("embedded component manifest is not valid UTF-8") from error
        matches.append(json.loads(decoded, object_pairs_hook=_strict_json_object,
                                  parse_constant=_reject_json_constant))
    ensure(len(matches) == 1 and payload.count(EMBEDDED_FRAME_SUFFIX) == 1,
           "raw Agent binary must contain exactly one embedded component manifest frame")
    return matches[0]


def verify_embedded_agent_manifest(payload, expected_version, expected_target, *,
                                   require_release_closure=True, expected_versions=None):
    ensure(isinstance(expected_version, str) and VERSION.fullmatch(expected_version),
           "invalid expected Agent version")
    ensure(expected_target in EXPECTED_EMBEDDED_ABI, f"unsupported Agent target: {expected_target}")
    manifest = extract_embedded_agent_manifest(payload)
    ensure(isinstance(manifest, dict) and set(manifest) == {
        "schema", "agent_version", "target_os", "target_arch", "target_env", "components",
    }, "unexpected or missing fields in embedded component manifest")
    ensure(type(manifest["schema"]) is int and manifest["schema"] == 1,
           "invalid embedded component manifest schema")
    ensure(isinstance(manifest["agent_version"], str) and manifest["agent_version"] == expected_version,
           "embedded component manifest version differs from signed Agent entry")
    expected_abi = EXPECTED_EMBEDDED_ABI[expected_target]
    actual_abi = (manifest["target_os"], manifest["target_arch"], manifest["target_env"])
    ensure(all(isinstance(part, str) for part in actual_abi) and actual_abi == expected_abi,
           f"embedded Agent ABI {actual_abi} does not match target '{expected_target}' ({expected_abi})")
    components = manifest["components"]
    ensure(isinstance(components, dict) and len(components) > 0,
           "embedded component manifest must contain components")
    if expected_versions is None:
        pinned = {
            "sing-box": SINGBOX_VERSION,
            "nodequality": NODEQUALITY_VERSION,
            "ipquality": IPQUALITY_DEFAULT_VERSION,
            "network-workbench": WORKBENCH_VERSION,
        }
    else:
        pinned = {}
        for key, value in expected_versions.items():
            ensure(key in LINUX_ARCHIVE_COMPONENTS and isinstance(value, str) and SEGMENT.fullmatch(value),
                   f"invalid expected embedded component version for '{key}'")
            pinned[key] = value
    if expected_abi[0] == "linux":
        unknown = sorted(set(components) - set(LINUX_REQUIRED_EMBEDDED_COMPONENTS))
        ensure(not unknown, "unsupported embedded Linux component: " + ", ".join(unknown))
        required = LINUX_REQUIRED_EMBEDDED_COMPONENTS if require_release_closure else ("sing-box", "tcpquality")
        for name in required:
            ensure(name in components, f"Linux Agent '{expected_target}' lacks verified embedded '{name}'")
    else:
        ensure(set(components) == {"sing-box"}, f"non-Linux Agent '{expected_target}' must embed only 'sing-box'")
    seen_digests = set()
    pinned_tcp_re = re.escape(TCPQUALITY_ENGINE_VERSION) + r"-[0-9a-f]{40}-r1"
    for name, record in components.items():
        ensure(isinstance(record, dict), f"embedded component '{name}' record must be a JSON object")
        if name == "tcpquality":
            ensure(set(record) == {"format", "subcommand", "version"},
                   "unexpected or missing fields in embedded 'tcpquality' record")
            ensure(record["format"] == "agent_subcommand" and record["subcommand"] == "tcp-probe",
                   "invalid embedded 'tcpquality' format or subcommand")
            tcp_ver = record["version"]
            ensure(isinstance(tcp_ver, str) and SEGMENT.fullmatch(tcp_ver),
                   "invalid embedded 'tcpquality' version segment")
            if require_release_closure:
                ensure(re.fullmatch(pinned_tcp_re, tcp_ver) is not None,
                       f"Linux Agent '{expected_target}' lacks build-commit-pinned built-in tcp-probe")
            else:
                ensure(tcp_ver == TCPQUALITY_ENGINE_VERSION or re.fullmatch(pinned_tcp_re, tcp_ver),
                       "invalid embedded 'tcpquality' version")
        else:
            ensure(name in LINUX_ARCHIVE_COMPONENTS and set(record) == {"format", "sha256", "version"},
                   f"unexpected or missing fields in embedded '{name}' record")
            ensure(record["format"] == "tar.gz", f"embedded '{name}' record must use 'tar.gz' format")
            ver, sha = record["version"], record["sha256"]
            ensure(isinstance(ver, str) and SEGMENT.fullmatch(ver),
                   f"invalid version segment in embedded '{name}' record")
            ensure(not (name == "nodequality" and ver.endswith("-offline-rootfs-r1")),
                   "embedded 'nodequality' cannot use historical offline-rootfs-r1 replacement")
            if name in pinned:
                ensure(ver == pinned[name],
                       f"embedded '{name}' version '{ver}' differs from expected '{pinned[name]}'")
            ensure(isinstance(sha, str) and re.fullmatch(r"[0-9a-f]{64}", sha),
                   f"invalid sha256 digest in embedded '{name}' record")
            ensure(sha not in seen_digests, f"duplicate embedded archive digest in '{expected_target}'")
            seen_digests.add(sha)
    return manifest


def _is_self_contained_release_version(version):
    match = re.match(r"^([0-9]+)\.([0-9]+)\.([0-9]+)", version)
    return bool(match) and (int(match.group(1)), int(match.group(2))) >= (0, 4)


def canonical_path(entry):
    ensure(entry["name"] in ("agent", "sing-box", "nodequality", "tcpquality", "ipquality"),
           "unsupported module")
    ensure(SEGMENT.fullmatch(entry["version"]), "invalid version segment")
    ensure(entry["arch"] in AGENT_TARGETS, "unsupported architecture")
    return "/".join(entry[k] for k in ("name", "version", "arch"))


def asset_name(entry):
    name, version, arch = (entry[k] for k in ("name", "version", "arch"))
    ensure(entry["format"] in ("raw", "tar.gz"), "unsupported format")
    if arch not in ("amd64", "arm64"):
        return f"{name}-{version}-{arch}" + (".tar.gz" if entry["format"] == "tar.gz" else "")
    return f"{name}-{version}-linux-musl-{arch}" if entry["format"] == "raw" else f"{name}-{version}-linux-{arch}.tar.gz"


def validate_auxiliary_files(value, binary_name, archive_format):
    ensure(isinstance(value, dict) and len(value) <= 7, "invalid auxiliary file list")
    ensure(not value or archive_format == "tar.gz", "raw artifacts cannot contain auxiliary files")
    denied = {".", "..", binary_name, "release.json", "SHA256SUMS", "SHA256SUMS.minisig", ".artifact.json"}
    for name, entry in value.items():
        ensure(isinstance(name, str) and re.fullmatch(r"[0-9A-Za-z._-]{1,128}", name)
               and name not in denied and not name.startswith("-"), "invalid auxiliary file name")
        ensure(isinstance(entry, dict) and set(entry) == {"sha256", "size"}, "invalid auxiliary file metadata")
        ensure(type(entry["size"]) is int and 0 < entry["size"] <= MAX_BINARY, "invalid auxiliary file size")
        ensure(isinstance(entry["sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", entry["sha256"]),
               "invalid auxiliary file digest")
    return value


def binary_bytes(data, archive_format, binary_name, auxiliary_files=None):
    auxiliary = validate_auxiliary_files(auxiliary_files if auxiliary_files is not None else {},
                                         binary_name, archive_format)
    if archive_format == "raw":
        return data
    ensure(archive_format == "tar.gz", "unsupported format")
    expected = {binary_name} | set(auxiliary)
    seen, binary, consumed = set(), None, 0
    with gzip.GzipFile(fileobj=io.BytesIO(data), mode="rb") as compressed:
        def read(size):
            nonlocal consumed
            content = compressed.read(min(size, MAX_BINARY - consumed + 1))
            consumed += len(content)
            ensure(consumed <= MAX_BINARY, "archive exceeds unpacked size limit")
            return content

        while True:
            header = read(tarfile.BLOCKSIZE)
            ensure(len(header) in (0, tarfile.BLOCKSIZE), "truncated archive header")
            if not header or not any(header):
                break
            try:
                member = tarfile.TarInfo.frombuf(header, encoding="utf-8", errors="strict")
            except tarfile.HeaderError as error:
                raise ValueError("invalid archive header") from error
            ensure(member.name in expected and member.name not in seen
                   and member.type in (tarfile.REGTYPE, tarfile.AREGTYPE), "unsafe archive member")
            seen.add(member.name)
            ensure(0 < member.size <= MAX_BINARY, "invalid archive member size")
            if member.name != binary_name:
                ensure(member.size == auxiliary[member.name]["size"], "auxiliary file size mismatch")
            content = read(member.size)
            ensure(len(content) == member.size, "archive file length mismatch")
            if member.name == binary_name:
                binary = content
            else:
                ensure(digest(content) == auxiliary[member.name]["sha256"], "auxiliary file digest mismatch")
            padding = (-member.size) % tarfile.BLOCKSIZE
            ensure(len(read(padding)) == padding, "truncated archive padding")
        ensure(seen == expected, "archive does not contain the exact signed file set")
        while tail := read(8192):
            ensure(not any(tail), "archive contains trailing data")
    return binary


def _resolve_targets(args, source):
    explicit = getattr(args, "target", None)
    if explicit:
        ensure(len(set(explicit)) == len(explicit) and set(explicit) <= set(AGENT_TARGETS),
               "invalid or duplicate Agent target")
        return list(explicit)
    architectures = getattr(args, "arch", None) or ("amd64", "arm64")
    ensure(len(set(architectures)) == len(architectures) and set(architectures) <= {"amd64", "arm64"},
           "invalid or duplicate architecture")
    targets = list(architectures)
    if getattr(args, "include_present_targets", False):
        agent_dir = source / "agent" / args.agent_version
        for candidate in AGENT_TARGETS:
            if candidate in targets or candidate in ("amd64", "arm64"):
                continue
            if candidate.startswith("linux-musl-") and candidate.removeprefix("linux-musl-") in targets:
                continue
            path = agent_dir / candidate
            if path.is_file() and not path.is_symlink():
                targets.append(candidate)
    return targets


def _collect_paired_sources(args, source, targets, component_versions):
    collected = {}
    for spec in getattr(args, "paired_source", None) or ():
        asset, raw_path = spec.split("=", 1) if "=" in spec else (Path(spec).name, spec)
        ensure(source_offer_asset_limit(asset) != MAX_BINARY and asset not in collected,
               f"invalid or duplicate paired source specification: {spec}")
        collected[asset] = Path(raw_path)
    linux_arches = sorted({
        arch for arch in ("amd64", "arm64")
        if any(t in (arch, f"linux-musl-{arch}", f"linux-gnu-{arch}") for t in targets)
    })
    sb_ver = component_versions.get("sing-box", SINGBOX_VERSION)
    nq_ver = component_versions.get("nodequality", NODEQUALITY_VERSION)
    wb_ver = component_versions.get("network-workbench", WORKBENCH_VERSION)
    ipq_ver = component_versions.get("ipquality", IPQUALITY_DEFAULT_VERSION)
    if not getattr(args, "no_auto_paired_sources", False) and linux_arches:
        sb_asset = f"sing-box-{sb_ver}-sources.tar.gz"
        if sb_asset not in collected:
            for candidate in (source / "sing-box" / sb_ver / sb_asset, source / sb_asset):
                if candidate.exists() or candidate.is_symlink():
                    collected[sb_asset] = candidate
                    break
        for arch in linux_arches:
            for component, ver in (("nodequality", nq_ver), ("network-workbench", wb_ver), ("ipquality", ipq_ver)):
                asset = f"{component}-{ver}-linux-{arch}-sources.tar.gz"
                if asset in collected:
                    continue
                for candidate in (source / component / ver / f"{arch}.sources.tar.gz",
                                  source / component / ver / asset, source / asset):
                    if candidate.exists() or candidate.is_symlink():
                        collected[asset] = candidate
                        break
    if getattr(args, "require_paired_sources", False) and linux_arches:
        required = {f"sing-box-{sb_ver}-sources.tar.gz"} | {
            f"{comp}-{ver}-linux-{arch}-sources.tar.gz"
            for arch in linux_arches
            for comp, ver in (("nodequality", nq_ver), ("network-workbench", wb_ver), ("ipquality", ipq_ver))
        }
        missing = sorted(required - set(collected))
        ensure(not missing, "missing required paired source archives: " + ", ".join(missing))
    return collected


def assemble(args):
    ensure(getattr(args, "runtime_version", None) is None
           and not getattr(args, "include_musl_runtime", False)
           and getattr(args, "tcp_probe_version", None) is None,
           "legacy separate-runtime assembly has been removed; assemble only self-contained Agent releases")
    ensure(VERSION.fullmatch(args.agent_version), "invalid Agent version")
    ensure(args.tag == "agent-v" + args.agent_version, "tag differs from Agent version")
    source, output = Path(args.source), Path(args.output)
    ensure(not output.exists() and not output.is_symlink(), "release output already exists")
    targets = _resolve_targets(args, source)
    output.mkdir(parents=True)
    try:
        paths, artifacts, paired_offers, seen_assets = {}, [], [], set()
        component_versions = {}
        for arch in targets:
            binary_name = "apsp-agent.exe" if arch.startswith("windows-") else "apsp-agent"
            entry = {"name": "agent", "version": args.agent_version, "arch": arch,
                     "format": "raw", "binary_name": binary_name}
            path = canonical_path(entry)
            data = read_regular(source / path, MAX_AGENT_RAW)
            manifest = verify_embedded_agent_manifest(data, args.agent_version, arch, require_release_closure=True)
            for comp_name, comp_record in manifest["components"].items():
                if comp_name in LINUX_ARCHIVE_COMPONENTS:
                    previous = component_versions.setdefault(comp_name, comp_record["version"])
                    ensure(previous == comp_record["version"],
                           f"all targets in one release must embed the same '{comp_name}' version")
            binary = binary_bytes(data, "raw", binary_name)
            entry.update(archive_size=len(data), binary_sha256=digest(binary),
                         binary_size=len(binary), asset_name=asset_name(entry))
            ensure(entry["asset_name"] not in seen_assets, f"duplicate release asset name: {entry['asset_name']}")
            seen_assets.add(entry["asset_name"])
            (output / entry["asset_name"]).write_bytes(data)
            paths[path] = digest(data)
            artifacts.append(entry)

        paired_map = _collect_paired_sources(args, source, targets, component_versions)
        for asset in sorted(paired_map):
            paired_path = paired_map[asset]
            limit = source_offer_asset_limit(asset)
            ensure(limit != MAX_BINARY, f"invalid paired source asset: {asset}")
            ipq_match = re.fullmatch(
                r"ipquality-([0-9A-Za-z.+_-]+)-linux-(amd64|arm64)-sources\.tar\.gz", asset)
            if ipq_match is not None:
                ver, arch = ipq_match.groups()
                ipq_archive = source / "ipquality" / ver / arch
                if ipq_archive.is_file() and not ipq_archive.is_symlink():
                    from ipquality_artifact import archive_files, source_offer, validate_files, validate_source_offer
                    files = archive_files(read_regular(ipq_archive))
                    validate_files(files, ver, arch)
                    offer = source_offer(files, ver, arch)
                    validate_source_offer(paired_path, files, ver, arch)
                    proof = regular_file_proof(paired_path, limit, output / asset)
                    ensure(proof == {k: offer[k] for k in ("sha256", "size")},
                           "paired source archive differs from the signed declaration")
                    paired_offers.append({"asset": asset, "sha256": proof["sha256"], "size": proof["size"]})
                    continue
            proof = regular_file_proof(paired_path, limit, output / asset)
            paired_offers.append({"asset": asset, "sha256": proof["sha256"], "size": proof["size"]})

        artifacts.sort(key=canonical_path)
        metadata = {"schema": 1, "source_repo": REPOSITORY, "tag": args.tag,
                    "protocol_min": 1, "protocol_max": 1, "artifacts": artifacts}
        encoded = (json.dumps(metadata, sort_keys=True, separators=(",", ":")) + "\n").encode("utf-8")
        ensure(len(encoded) <= 32768, "metadata too large")
        (output / "release.json").write_bytes(encoded)
        installer = inject_paired_source_headers(read_regular(Path(args.installer), 256 * 1024), paired_offers)
        (output / "install.sh").write_bytes(installer)
        paths.update({"release.json": digest(encoded), "install.sh": digest(installer)})
        checksums = "".join(f"{paths[path]}  {path}\n" for path in sorted(paths)).encode("utf-8")
        ensure(len(checksums) <= 8192, "checksums too large")
        (output / "SHA256SUMS").write_bytes(checksums)
    except BaseException:
        shutil.rmtree(output, ignore_errors=True)
        raise


def installer_source(template, agent_unit, runtime_unit, source_root=SOURCE_ROOT):
    """Render audited static Linux installation logic for release or trusted bootstrap."""
    source_root = Path(source_root)
    text = read_regular(Path(template), 262144).decode("utf-8")
    if "@@LEGACY_CHECKPOINT_PREFLIGHT@@" in text:
        ensure(text.count("@@LEGACY_CHECKPOINT_PREFLIGHT@@") == 2,
               "missing or duplicate legacy preflight marker")
        guard = read_regular(source_root / "tools/legacy_agent_checkpoint.py", 65536).decode("utf-8").rstrip()
        text = text.replace("@@LEGACY_CHECKPOINT_PREFLIGHT@@", guard)
    for marker, filename in (("@@AGENT_UNIT@@", agent_unit), ("@@RUNTIME_UNIT@@", runtime_unit)):
        ensure(text.count(marker) == 1, "missing or duplicate installer unit marker")
        text = text.replace(marker, read_regular(Path(filename), 65536).decode("utf-8").rstrip())
    for marker, filename in (
        ("@@AGENT_OPENRC@@", source_root / "deploy/apsp-agent.openrc"),
        ("@@RUNTIME_OPENRC@@", source_root / "deploy/apsp-singbox.openrc"),
        ("@@RUNTIME_OPENRC_LEGACY@@", source_root / "deploy/apsp-singbox-legacy.openrc"),
    ):
        if marker in text:
            ensure(text.count(marker) == 1, "duplicate installer unit marker")
            text = text.replace(marker, read_regular(filename, 65536).decode("utf-8").rstrip())
    ensure("@@" not in text, "unexpanded installer marker")
    return text


def render_installer(args):
    text = installer_source(args.template, args.agent_unit, args.runtime_unit)
    output = Path(args.output)
    ensure(not output.exists(), "installer output exists")
    output.write_bytes(text.encode("utf-8"))


def public_record(key):
    ensure(isinstance(key, str) and len(key.encode("utf-8")) <= 4096, "invalid public key")
    lines = key.splitlines()
    record = lines[-1] if len(lines) in (1, 2) else ""
    if len(lines) == 2:
        ensure(lines[0].startswith("untrusted comment: "), "invalid public key comment")
    binary = base64.b64decode(record, validate=True)
    ensure(len(binary) == 42 and binary[:2] == b"Ed", "invalid minisign public key")
    return record, binary[10:]


def test_public_material():
    denied = {public_record(k)[1] for k in (TEST_ONLY_PUBLIC_KEY, TEST_ONLY_ROTATION_PUBLIC_KEY)}
    for directory in TEST_PUBLIC_KEY_DIRS:
        for path in directory.glob("TEST_ONLY*.pub"):
            denied.add(public_record(read_regular(path, 4096).decode("utf-8"))[1])
    return denied


def load_roots(path, publication=False, require_protected=False):
    path = Path(path)
    if require_protected:
        for part in (path,) + tuple(path.parents):
            st = part.lstat()
            ensure(not part.is_symlink() and st.st_uid == 0 and st.st_mode & 0o022 == 0,
                   "bootstrap trust file and parents must be root-owned and protected")
    value = json.loads(read_regular(path, 32768))
    ensure(isinstance(value, list) and 0 < len(value) <= 8, "invalid trusted key set")
    denied = test_public_material() if publication else set()
    roots, public_material = [], set()
    for key in value:
        record, material = public_record(key)
        ensure(material not in public_material, "duplicate public key")
        if publication:
            ensure(material not in denied, "TEST_ONLY key cannot publish an official release")
        roots.append(record)
        public_material.add(material)
    return roots


def require_protected_file(path):
    path = Path(path)
    for part in (path,) + tuple(path.parents):
        st = part.lstat()
        ensure(not part.is_symlink() and st.st_uid == 0 and st.st_mode & 0o022 == 0,
               f"trusted file and parents must be root-owned and protected: {part} "
               f"(owner {st.st_uid}, mode {st.st_mode & 0o7777:o})")


def verify_signature(bundle, roots, minisign):
    lines = read_regular(bundle / "SHA256SUMS.minisig", 16384).decode("utf-8").splitlines()
    ensure(len(lines) == 4 and lines[0].startswith("untrusted comment: ")
           and lines[2].startswith("trusted comment: "), "signature must contain all four minisign lines")
    ensure(base64.b64decode(lines[1], validate=True)[:2] == b"ED", "legacy signature prohibited")
    for key in roots:
        result = subprocess.run([minisign, "-V", "-H", "-q", "-m", str(bundle / "SHA256SUMS"),
                                 "-x", str(bundle / "SHA256SUMS.minisig"), "-P", key],
                                capture_output=True, check=False)
        if result.returncode == 0:
            return
    raise ValueError("no trusted key verifies the complete signature")


def verify_manifest(bundle, roots, minisign, expected_tag=None, protocol_version=1):
    verify_signature(Path(bundle), roots, minisign)
    return validate_manifest(bundle, expected_tag, protocol_version)


def validate_manifest(bundle, expected_tag=None, protocol_version=1):
    """Validate signed metadata, including read-only verification of historical 0.3.x releases."""
    bundle = Path(bundle)
    ensure(not bundle.is_symlink(), "bundle must not be a symlink")
    checksums = read_regular(bundle / "SHA256SUMS", 8192).decode("utf-8")
    rows = {}
    for line in checksums.splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([0-9A-Za-z.+_/-]+)", line)
        ensure(match is not None, "noncanonical checksum entry")
        value, path = match.groups()
        ensure(path not in rows and not path.startswith("/") and ".." not in path.split("/"),
               "duplicate or unsafe checksum path")
        rows[path] = value
    ensure(checksums == "".join(f"{rows[p]}  {p}\n" for p in sorted(rows)),
           "checksums must be sorted canonical LF text")
    metadata_bytes = read_regular(bundle / "release.json", 32768)
    ensure(rows.get("release.json") == digest(metadata_bytes), "metadata hash mismatch")
    metadata = json.loads(metadata_bytes)
    ensure(set(metadata) == {"schema", "source_repo", "tag", "protocol_min", "protocol_max", "artifacts"},
           "unexpected metadata fields")
    ensure(type(metadata["schema"]) is int and metadata["schema"] == 1 and metadata["source_repo"] == REPOSITORY,
           "wrong release identity")
    ensure(type(metadata["protocol_min"]) is int and type(metadata["protocol_max"]) is int
           and 1 <= metadata["protocol_min"] <= metadata["protocol_max"] <= 65535
           and (protocol_version is None or metadata["protocol_min"] <= protocol_version <= metadata["protocol_max"]),
           "unsupported protocol range")
    ensure(expected_tag is None or metadata["tag"] == expected_tag, "wrong release tag")
    ensure(isinstance(metadata["artifacts"], list) and 0 < len(metadata["artifacts"]) <= 30,
           "invalid artifact list")
    expected_paths, seen_assets = {"release.json", "install.sh"}, set()
    required = {"name", "version", "arch", "format", "binary_name", "archive_size",
                "binary_sha256", "binary_size", "asset_name"}
    for entry in metadata["artifacts"]:
        ensure(isinstance(entry, dict) and set(entry) in (required, required | {"auxiliary_files"}),
               "unexpected artifact fields")
        path = canonical_path(entry)
        ensure(path not in expected_paths, "duplicate artifact identity")
        expected_paths.add(path)
        ensure(entry["asset_name"] == asset_name(entry) and entry["asset_name"] not in seen_assets,
               "asset name differs from signed identity or duplicates another entry")
        seen_assets.add(entry["asset_name"])
        ensure(type(entry["archive_size"]) is int and 0 < entry["archive_size"] <= MAX_BINARY
               and type(entry["binary_size"]) is int and 0 < entry["binary_size"] <= MAX_BINARY,
               "invalid signed sizes")
        ensure(isinstance(entry["binary_sha256"], str) and re.fullmatch(r"[0-9a-f]{64}", entry["binary_sha256"]),
               "invalid binary digest")
        ensure(isinstance(entry["binary_name"], str) and SEGMENT.fullmatch(entry["binary_name"])
               and entry["binary_name"] not in (".", ".."), "invalid signed binary name")
        validate_auxiliary_files(entry.get("auxiliary_files", {}), entry["binary_name"], entry["format"])
        if entry["name"] == "tcpquality":
            from tcp_probe_artifact import BINARY, FILES, TOOL_VERSION
            ensure(entry["format"] == "tar.gz" and entry["binary_name"] == BINARY
                   and entry["arch"] in ("amd64", "arm64")
                   and re.fullmatch(re.escape(TOOL_VERSION) + r"-[0-9a-f]{40}-r1", entry["version"])
                   and set(entry.get("auxiliary_files", {})) == FILES - {BINARY},
                   "wrong or incomplete native TCP artifact identity")
        if entry["name"] == "ipquality":
            from ipquality_artifact import BINARY, FILES, VERSION as IPQUALITY_VERSION
            ensure(entry["format"] == "tar.gz" and entry["binary_name"] == BINARY
                   and entry["arch"] in ("amd64", "arm64") and entry["version"] == IPQUALITY_VERSION
                   and set(entry.get("auxiliary_files", {})) == FILES - {BINARY},
                   "wrong or incomplete offline IPQuality artifact identity")
        nq_offline = {
            "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r20",
            "a92fca6c0067df29ddd03fdc2fee6f3000f64545-offline-rootfs-r1",
        }
        if entry["name"] == "nodequality" and entry["version"] in nq_offline:
            from nodequality_rootfs_artifact import BINARY, FILES
            ensure(entry["format"] == "tar.gz" and entry["binary_name"] == BINARY
                   and entry["arch"] in ("amd64", "arm64")
                   and set(entry.get("auxiliary_files", {})) == FILES - {BINARY},
                   "wrong or incomplete offline NodeQuality artifact identity")
        if entry["name"] == "nodequality" and entry["version"] not in nq_offline:
            ensure(not entry.get("auxiliary_files"),
                   "runner-only NodeQuality identity cannot claim offline auxiliary files")
        if entry["name"] == "nodequality" and entry["version"] == "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r21":
            from nodequality_node_query_artifact import BINARY
            ensure(entry["format"] == "tar.gz" and entry["binary_name"] == BINARY
                   and entry["arch"] in ("amd64", "arm64") and not entry.get("auxiliary_files", {}),
                   "wrong official node-query artifact identity")
        if entry["name"] == "sing-box" and entry["version"] == "1.14.4-apsp.2":
            arch = entry["arch"]
            expected = ({"libcronet.dll"} if arch.startswith("windows-")
                        else {"libcronet.so"} if arch in ("amd64", "arm64") or arch.startswith("linux-gnu-")
                        else set())
            ensure(set(entry.get("auxiliary_files", {})) == expected, "incomplete runtime library contract")
        if entry["name"] == "agent":
            binary_name = "apsp-agent.exe" if entry["arch"].startswith("windows-") else "apsp-agent"
            ensure(metadata["tag"] == "agent-v" + entry["version"] and entry["format"] == "raw"
                   and entry["binary_name"] == binary_name and entry["archive_size"] <= MAX_AGENT_RAW,
                   "wrong Agent release identity")
        if entry["format"] == "raw":
            ensure(entry["archive_size"] == entry["binary_size"] and rows.get(path) == entry["binary_sha256"],
                   "raw binary proof mismatch")
    ensure(set(rows) == expected_paths, "unsigned or unused checksum paths")
    installer_bytes = read_regular(bundle / "install.sh", 256 * 1024)
    ensure(rows["install.sh"] == digest(installer_bytes), "installer mismatch")
    parse_paired_source_headers(installer_bytes)
    return metadata, rows


def verify_bundle(bundle, roots, minisign, expected_tag=None, exact_assets=True):
    bundle = Path(bundle)
    metadata, rows = verify_manifest(bundle, roots, minisign, expected_tag)
    paired_headers = parse_paired_source_headers(read_regular(bundle / "install.sh", 256 * 1024))
    enforce_self_contained = all(
        entry["name"] == "agent" and _is_self_contained_release_version(entry["version"])
        for entry in metadata["artifacts"]
    )
    expected_files = {"release.json", "install.sh", "SHA256SUMS", "SHA256SUMS.minisig"}
    for entry in metadata["artifacts"]:
        path = canonical_path(entry)
        expected_files.add(entry["asset_name"])
        limit = MAX_AGENT_RAW if entry["name"] == "agent" else MAX_BINARY
        data = read_regular(bundle / entry["asset_name"], limit)
        ensure(len(data) == entry["archive_size"] and rows.get(path) == digest(data), "archive mismatch")
        binary = binary_bytes(data, entry["format"], entry["binary_name"], entry.get("auxiliary_files", {}))
        ensure(len(binary) == entry["binary_size"] and digest(binary) == entry["binary_sha256"],
               "binary mismatch")
        if enforce_self_contained and entry["name"] == "agent":
            verify_embedded_agent_manifest(data, entry["version"], entry["arch"],
                                           require_release_closure=True, expected_versions={})
        if entry["name"] == "tcpquality":
            from tcp_probe_artifact import archive_files, validate_files
            validate_files(archive_files(data), entry["version"], entry["arch"])
        if entry["name"] == "ipquality":
            from ipquality_artifact import (MAX_SOURCE_OFFER, archive_files, source_offer,
                                            validate_files, validate_source_offer)
            files = archive_files(data)
            validate_files(files, entry["version"], entry["arch"])
            offer = source_offer(files, entry["version"], entry["arch"])
            expected_files.add(offer["asset"])
            paired = bundle / offer["asset"]
            ensure(regular_file_proof(paired, MAX_SOURCE_OFFER) == {k: offer[k] for k in ("sha256", "size")},
                   "paired source archive differs from the signed declaration")
            validate_source_offer(paired, files, entry["version"], entry["arch"])
        if entry["name"] == "nodequality" and entry["version"] == "a92fca6c0067df29ddd03fdc2fee6f3000f64545-offline-rootfs-r1":
            from nodequality_native_rootfs_artifact import archive_files, validate_files
            validate_files(archive_files(data), entry["version"], entry["arch"])
        if entry["name"] == "nodequality" and entry["version"] == "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r20":
            from nodequality_rootfs_artifact import archive_files, validate_files
            validate_files(archive_files(data), entry["version"], entry["arch"])
        if entry["name"] == "nodequality" and entry["version"] == "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r21":
            from nodequality_node_query_artifact import archive_files, validate_files
            validate_files(archive_files(data), entry["version"], entry["arch"])
    for offer in paired_headers:
        ensure(offer["asset"] not in expected_files, "duplicate paired source asset")
        expected_files.add(offer["asset"])
        ensure(regular_file_proof(bundle / offer["asset"], source_offer_asset_limit(offer["asset"]))
               == {k: offer[k] for k in ("sha256", "size")},
               "paired source archive differs from the signed declaration")
    if exact_assets:
        ensure({p.name for p in bundle.iterdir()} == expected_files, "missing or extra release assets")
    return metadata


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    build = commands.add_parser("assemble")
    for argument in ("source", "output", "tag", "agent-version", "installer"):
        build.add_argument("--" + argument, required=True)
    build.add_argument("--arch", action="append", choices=("amd64", "arm64"),
                       help="Linux architectures; defaults to both amd64 and arm64")
    build.add_argument("--target", action="append", choices=AGENT_TARGETS,
                       help="explicit Agent release target(s)")
    build.add_argument("--include-present-targets", action="store_true",
                       help="include additional built Agent targets present under source/agent/<version>/")
    build.add_argument("--paired-source", action="append", default=[],
                       help="explicit paired source archive (<path> or <asset_name>=<path>)")
    build.add_argument("--no-auto-paired-sources", action="store_true",
                       help="disable automatic discovery of paired source archives under --source")
    build.add_argument("--require-paired-sources", action="store_true",
                       help="require corresponding source archives for all embedded Linux components")
    render = commands.add_parser("render-installer")
    for argument in ("template", "agent-unit", "runtime-unit", "output"):
        render.add_argument("--" + argument, required=True)
    verify = commands.add_parser("verify")
    verify.add_argument("--bundle", required=True)
    verify.add_argument("--trusted-keys", required=True)
    verify.add_argument("--minisign", default="minisign")
    verify.add_argument("--tag")
    verify.add_argument("--publication", action="store_true")
    args = parser.parse_args()
    if args.command == "assemble":
        assemble(args)
    elif args.command == "render-installer":
        render_installer(args)
    else:
        roots = load_roots(args.trusted_keys, args.publication)
        verify_bundle(args.bundle, roots, args.minisign, args.tag)
        print("Complete signature, canonical metadata, installer, and every artifact verified.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError, json.JSONDecodeError) as error:
        raise SystemExit(f"Release verification failed: {error}") from error
