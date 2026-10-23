"""Validate the complete, pinned native TCP artifact without executing it."""
import gzip
import hashlib
import io
import json
import re
import struct
import tarfile

TOOL_VERSION = "0.3.0"
BINARY = "apsp-tcp-probe"
FILES = {BINARY, "build-info.json", "LICENSE", "source.tar.gz", "Cargo.lock", "THIRD_PARTY_NOTICES.txt"}
TARGETS = {"amd64": "x86_64-unknown-linux-musl", "arm64": "aarch64-unknown-linux-musl"}
SOURCE_LIMIT = 16 * 1024 * 1024
BUNDLED_MUSL_FILES = ("tools/licenses/bundled-musl.json", "tools/licenses/musl-1.2.5-COPYRIGHT",
                      "tools/licenses/rust-1.98.1-musl-recipe.txt")
BUNDLED_MUSL_IDENTITY = {
    "schema": 1, "version": "1.2.5",
    "source_url": "https://musl.libc.org/releases/musl-1.2.5.tar.gz",
    "source_sha256": "a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4",
    "copyright_sha256": "f9bc4423732350eb0b3f7ed7e91d530298476f8fec0c6c427a1c04ade22655af",
    "rustc_commit": "48a229ceaefd4985c50990b14116b6d856af0985",
    "rust_recipe_url": "https://raw.githubusercontent.com/rust-lang/rust/48a229ceaefd4985c50990b14116b6d856af0985/src/ci/docker/scripts/musl.sh",
    "rust_recipe_sha256": "2f218a2dc7b7e73509212bfd4319ebddc2ddac7c651fca142c2b29bd7ea0aa38",
    "patches": "CVE-2025-26519: two iconv patches in the pinned Rust recipe; copyright unchanged",
}


def ensure(condition, message):
    if not condition:
        raise ValueError(message)


def digest(value):
    return hashlib.sha256(value).hexdigest()


def artifact_version(commit):
    ensure(isinstance(commit, str) and re.fullmatch(r"[0-9a-f]{40}", commit),
           "TCP source commit must be an actual full lowercase SHA")
    return f"{TOOL_VERSION}-{commit}-r1"


def verify_elf(binary, arch):
    ensure(arch in TARGETS and len(binary) >= 64 and binary[:6] == b"\x7fELF\x02\x01",
           "TCP binary must be a little-endian ELF64")
    ensure(struct.unpack_from("<H", binary, 18)[0] == (62 if arch == "amd64" else 183),
           "TCP binary architecture mismatch")
    offset = struct.unpack_from("<Q", binary, 32)[0]
    size, count = struct.unpack_from("<HH", binary, 54)
    ensure(size == 56 and 0 < count <= 128 and offset + size * count <= len(binary),
           "invalid ELF program headers")
    for index in range(count):
        kind, _, start, _, _, length, _, _ = struct.unpack_from("<IIQQQQQQ", binary, offset + index * size)
        ensure(start + length <= len(binary), "ELF segment exceeds binary")
        ensure(kind != 3, "TCP binary has a dynamic interpreter")
        if kind == 2:
            ensure(length % 16 == 0, "invalid ELF dynamic segment")
            for entry in range(start, start + length, 16):
                tag = struct.unpack_from("<q", binary, entry)[0]
                ensure(tag != 1, "TCP binary has a dynamic library dependency")
                if tag == 0:
                    break


def source_files(data, commit):
    ensure(0 < len(data) <= SOURCE_LIMIT, "source archive size outside permitted range")
    with gzip.GzipFile(fileobj=io.BytesIO(data)) as compressed:
        unpacked = compressed.read(SOURCE_LIMIT + 1)
    ensure(len(unpacked) <= SOURCE_LIMIT, "source archive exceeds unpacked size limit")
    files, consumed = {}, 0
    with tarfile.open(fileobj=io.BytesIO(unpacked), mode="r:") as archive:
        ensure(archive.pax_headers.get("comment") == commit, "source archive commit mismatch")
        for member in archive:
            ensure(len(files) < 10000 and len(member.name) <= 1024
                   and not member.name.startswith("/") and "\\" not in member.name
                   and all(part not in ("", ".", "..") for part in member.name.rstrip("/").split("/")),
                   "unsafe source archive path")
            ensure(member.isdir() or member.isfile(), "source archive must contain ordinary files")
            if member.isdir():
                continue
            ensure(member.name not in files and 0 <= member.size <= SOURCE_LIMIT,
                   "duplicate or oversized source file")
            consumed += member.size
            ensure(consumed <= SOURCE_LIMIT, "source archive exceeds unpacked size limit")
            files[member.name] = archive.extractfile(member).read()
    required = {"Cargo.toml", "Cargo.lock", "LICENSE", "crates/tcp-probe/Cargo.toml",
                "crates/tcp-probe/src/lib.rs", "crates/tcp-probe/src/main.rs",
                "crates/tcp-probe/src/cli.rs", "crates/tcp-probe/src/engine.rs",
                "crates/tcp-probe/src/journal.rs", "crates/tcp-probe/src/model.rs",
                "tools/build-tcp-probe.py", "tools/tcp_probe_artifact.py", "tools/artifact_manifest.py", "tools/tcp_probe_notices.py"}
    ensure(required | set(BUNDLED_MUSL_FILES) <= files.keys(), "source archive is missing the tool or its build recipe")
    return files


def validate_files(files, version, arch):
    import tomllib

    ensure(set(files) == FILES, "TCP artifact must contain its exact complete provenance file set")
    ensure(all(0 < len(value) <= 256 * 1024 * 1024 for value in files.values()), "invalid TCP file size")
    info = json.loads(files["build-info.json"])
    expected = {"schema", "tool", "tool_version", "artifact_version", "source_repo", "source_commit",
                "target", "rustc", "cargo_locked", "source_sha256", "lock_sha256",
                "license_sha256", "binary_sha256", "notices_sha256"}
    ensure(isinstance(info, dict) and set(info) == expected, "invalid TCP build information")
    commit = info["source_commit"]
    ensure(version == artifact_version(commit) and info["artifact_version"] == version,
           "TCP artifact version must include its exact source commit")
    ensure(type(info["schema"]) is int and info["schema"] == 1 and info["tool"] == BINARY
           and info["tool_version"] == TOOL_VERSION and info["source_repo"] == "theLucius7/apsp"
           and info["cargo_locked"] is True and arch in TARGETS and info["target"] == TARGETS[arch]
           and isinstance(info["rustc"], str) and info["rustc"].startswith("rustc ") and len(info["rustc"]) <= 4096,
           "wrong TCP build identity or unlocked build")
    for filename, field in [(BINARY, "binary_sha256"), ("source.tar.gz", "source_sha256"),
                            ("Cargo.lock", "lock_sha256"), ("LICENSE", "license_sha256"),
                            ("THIRD_PARTY_NOTICES.txt", "notices_sha256")]:
        ensure(info[field] == digest(files[filename]), "TCP provenance digest mismatch")
    source = source_files(files["source.tar.gz"], commit)
    ensure(source["Cargo.lock"] == files["Cargo.lock"] and source["LICENSE"] == files["LICENSE"],
           "TCP lockfile or license differs from pinned source")
    workspace = tomllib.loads(source["Cargo.toml"].decode())
    package = tomllib.loads(source["crates/tcp-probe/Cargo.toml"].decode())["package"]
    ensure(workspace["workspace"]["package"]["version"] == TOOL_VERSION
           and workspace["workspace"]["package"]["license"] == "AGPL-3.0-only"
           and package["name"] == BINARY and package["version"] == {"workspace": True}
           and package["license"] == {"workspace": True}, "TCP source version or license mismatch")
    validate_notices(files["THIRD_PARTY_NOTICES.txt"], files["Cargo.lock"], arch, source, info["rustc"])
    verify_elf(files[BINARY], arch)
    return info


def archive_files(data):
    result, consumed = {}, 0
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
        for member in archive:
            ensure(member.name in FILES and member.name not in result and member.isfile()
                   and 0 < member.size <= 256 * 1024 * 1024, "unsafe TCP artifact member")
            consumed += member.size
            ensure(consumed <= 256 * 1024 * 1024, "TCP artifact exceeds unpacked file budget")
            result[member.name] = archive.extractfile(member).read()
    ensure(set(result) == FILES, "TCP artifact is missing provenance files")
    return result


def pack(files):
    output = io.BytesIO()
    with gzip.GzipFile(fileobj=output, mode="wb", filename="", mtime=0) as compressed:
        with tarfile.open(fileobj=compressed, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for name in sorted(files):
                member = tarfile.TarInfo(name)
                member.size = len(files[name])
                member.mode = 0o755 if name == BINARY else 0o644
                archive.addfile(member, io.BytesIO(files[name]))
    return output.getvalue()

def bundled_musl(source, rustc_info):
    ensure(set(BUNDLED_MUSL_FILES) <= source.keys(), "bundled musl originals are missing from pinned source")
    identity = json.loads(source[BUNDLED_MUSL_FILES[0]])
    ensure(identity == BUNDLED_MUSL_IDENTITY, "unknown bundled musl source inventory")
    copyright_text, recipe = (source[name] for name in BUNDLED_MUSL_FILES[1:])
    ensure(digest(copyright_text) == identity["copyright_sha256"]
           and digest(recipe) == identity["rust_recipe_sha256"], "bundled musl source originals differ")
    ensure(isinstance(rustc_info, str)
           and [line for line in rustc_info.splitlines() if line.startswith("commit-hash:")]
           == ["commit-hash: " + identity["rustc_commit"]],
           "bundled musl inventory does not cover this rustc commit")
    return dict(name="Rust bundled musl libc", version=identity["version"], notices=[
        dict(path="musl-1.2.5/COPYRIGHT", text=copyright_text.decode()),
        dict(path="rust-1.98.1/musl-recipe.txt", text=recipe.decode()),
        dict(path="bundled-musl/source.json", text=source[BUNDLED_MUSL_FILES[0]].decode()),
    ])


def validate_notices(encoded, lock_bytes, arch, source, rustc_info):
    import tomllib

    ensure(0 < len(encoded) <= 8 * 1024 * 1024, "invalid third-party notice inventory size")
    data = json.loads(encoded)
    ensure(isinstance(data, dict) and set(data) == {"schema", "target", "lock_sha256", "dependencies", "toolchain"}
           and type(data["schema"]) is int and data["schema"] == 1 and data["target"] == TARGETS[arch]
           and data["lock_sha256"] == digest(lock_bytes), "third-party inventory identity mismatch")
    locked = {(p["name"], p["version"], p.get("source")): p for p in tomllib.loads(lock_bytes.decode())["package"]}
    packages = data["dependencies"]
    ensure(isinstance(packages, list) and 0 < len(packages) <= 256, "native dependency notices are missing")
    seen = set()
    def originals(notices):
        ensure(isinstance(notices, list) and 0 < len(notices) <= 128, "license originals are missing")
        names = set()
        for item in notices:
            ensure(isinstance(item, dict) and set(item) == {"path", "text"}
                   and isinstance(item["path"], str) and 0 < len(item["path"]) <= 1024
                   and item["path"] not in names and not item["path"].startswith("/")
                   and all(p not in ("", ".", "..") for p in item["path"].split("/"))
                   and isinstance(item["text"], str) and 0 < len(item["text"]) <= 8 * 1024 * 1024,
                   "invalid license original")
            names.add(item["path"])
    for package in packages:
        ensure(isinstance(package, dict) and set(package) == {"name", "version", "source", "checksum", "license", "notices"},
               "invalid locked dependency notice")
        identity = (package["name"], package["version"], package["source"])
        ensure(all(isinstance(value, str) for value in identity) and identity not in seen
               and identity in locked and locked[identity].get("checksum") == package["checksum"]
               and isinstance(package["license"], str) and package["license"], "notice differs from locked dependency")
        originals(package["notices"])
        if "Unicode" in package["license"]:
            ensure(any("unicode" in n["path"].lower() for n in package["notices"]), "Unicode license original is missing")
        seen.add(identity)
    libraries = data["toolchain"]
    ensure(isinstance(libraries, list) and len(libraries) == 3
           and {p.get("name") for p in libraries} == {"Rust standard library and bundled native libraries", "system musl build tooling", "Rust bundled musl libc"},
           "native library license originals are missing")
    for library in libraries:
        ensure(set(library) == {"name", "version", "notices"} and isinstance(library["version"], str)
               and library["version"], "native library notice identity mismatch")
        originals(library["notices"])
    bundled = next(p for p in libraries if p["name"] == "Rust bundled musl libc")
    ensure(bundled == bundled_musl(source, rustc_info), "bundled musl notice differs from pinned source")
    standard = next(p for p in libraries if p["name"] == "Rust standard library and bundled native libraries")
    ensure(standard["version"] == rustc_info, "Rust notice and binary toolchain versions differ")
    return data
