#!/usr/bin/env python3
"""Stage a frozen source and reviewed private harness; never launch or install."""
import argparse
import ast
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import tarfile

OLD_NAMESPACE = "apsp-plugin-install-20261001-r3"
HARNESSES = ("common.py", "setup.py", "artifact.py", "exercise.py", "finish.py", "driver.py", "build-v3.py")
MAX_ARCHIVE = 32 * 1024 * 1024


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def digest(value):
    return hashlib.sha256(value).hexdigest()


def ordinary(path, limit):
    path = path.absolute()
    require(not any(component.is_symlink() for component in (path, *path.parents)), "symlink in input path")
    with os.fdopen(os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK), "rb") as stream:
        before = os.fstat(stream.fileno())
        require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1 and before.st_size <= limit,
                "ordinary bounded input required")
        value = stream.read(limit + 1)
        after = os.fstat(stream.fileno())
        identity = lambda row: (row.st_dev, row.st_ino, row.st_size, row.st_mtime_ns, row.st_ctime_ns)
        require(len(value) == before.st_size and identity(before) == identity(after), "input changed while reading")
        return value


def git(source, *arguments):
    return subprocess.check_output(["git", "-C", str(source), *arguments], timeout=30)


def frozen_files(archive):
    result = {}
    total = 0
    with tarfile.open(fileobj=io.BytesIO(archive), mode="r:gz") as source:
        for entry in source:
            path = PurePosixPath(entry.name)
            require(not path.is_absolute() and not any(part in ("", ".", "..") for part in path.parts),
                    "unsafe archive path")
            if entry.isdir():
                continue
            require(entry.isfile() and entry.name not in result and 0 <= entry.size <= MAX_ARCHIVE,
                    "unsupported or duplicate source archive member")
            total += entry.size
            require(total <= MAX_ARCHIVE, "source archive expands beyond limit")
            stream = source.extractfile(entry)
            require(stream is not None, "missing source bytes")
            value = stream.read(entry.size + 1)
            require(len(value) == entry.size, "source member size mismatch")
            result[entry.name] = (value, bool(entry.mode & 0o111))
    require("Cargo.lock" in result and "scripts/ci-test-trust.py" in result, "incomplete source archive")
    return result


def write_new(path, value):
    with path.open("xb") as output:
        output.write(value)


def json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, indent=2) + "\n").encode()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", required=True, type=Path)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--harness-source", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--namespace", required=True)
    args = parser.parse_args()
    require(re.fullmatch(r"[0-9a-f]{40}", args.source_commit), "explicit full source commit required")
    require(re.fullmatch(r"apsp-plugin-install-20[0-9]{6}-r[1-9][0-9]{0,2}", args.namespace)
            and args.namespace != OLD_NAMESPACE, "new private namespace required")
    source = args.source.absolute()
    require(git(source, "rev-parse", "HEAD").decode().strip() == args.source_commit, "HEAD differs from freeze")
    require(not git(source, "status", "--porcelain", "--untracked-files=normal"), "freeze a clean committed source")
    archive = git(source, "archive", "--format=tar.gz", args.source_commit)
    require(len(archive) <= MAX_ARCHIVE, "source archive exceeds limit")
    files = frozen_files(archive)

    harness = args.harness_source.absolute()
    identity_bytes = ordinary(harness / "harness-identity.json", 1024 * 1024)
    identity = json.loads(identity_bytes)
    hashes = identity["script_sha256"]
    scripts = {}
    for name in HARNESSES:
        value = ordinary(harness / name, 1024 * 1024)
        require(hashes.get(name) == digest(value), "reviewed lifecycle/build harness identity differs")
        ast.parse(value)
        scripts[name] = value.replace(OLD_NAMESPACE.encode(), args.namespace.encode())
        ast.parse(scripts[name])
    require(args.namespace.encode() in scripts["common.py"] and OLD_NAMESPACE.encode() not in scripts["common.py"],
            "private lifecycle root was not relocated")

    output = args.output.absolute()
    require(not output.exists() and not output.is_symlink(), "output already exists; preserve prior attempts")
    require(output.parent.is_dir() and not any(p.is_symlink() for p in (output.parent, *output.parents)),
            "ordinary existing private parent required")
    require(stat.S_IMODE(output.parent.stat().st_mode) & 0o077 == 0, "private output parent must be mode 0700")
    # Recheck source and reviewed scripts before the first write. No SSH, service,
    # credential, compiler, network, signing or test operation occurs in this tool.
    require(git(source, "rev-parse", "HEAD").decode().strip() == args.source_commit
            and not git(source, "status", "--porcelain", "--untracked-files=normal")
            and git(source, "archive", "--format=tar.gz", args.source_commit) == archive, "source changed during freeze")
    require(ordinary(harness / "harness-identity.json", 1024 * 1024) == identity_bytes, "review manifest changed")
    for name in HARNESSES:
        require(ordinary(harness / name, 1024 * 1024).replace(OLD_NAMESPACE.encode(), args.namespace.encode())
                == scripts[name], "harness changed during freeze")
    os.umask(0o077)
    output.mkdir(mode=0o700)
    write_new(output / "source.tar.gz", archive)
    metadata = {"source_commit": args.source_commit, "archive_sha256": digest(archive),
                "cargo_lock_sha256": digest(files["Cargo.lock"][0]),
                "input_hashes": {name: digest(value[0]) for name, value in sorted(files.items())},
                "scope": "Frozen current source; TEST_ONLY private guest build and installation; no official release"}
    write_new(output / "SOURCE.json", json_bytes(metadata))
    (output / "src").mkdir(mode=0o700)
    for name, (value, executable) in files.items():
        target = output / "src" / name
        target.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        write_new(target, value)
        if executable:
            target.chmod(0o700)
    for name, value in scripts.items():
        write_new(output / name, value)
    plan = {"schema": 1, "status": "prepared_not_executed", "source_commit": args.source_commit,
            "namespace": args.namespace, "guest_root": "/home/l7.guest/" + args.namespace,
            "original_harness_manifest_sha256": digest(identity_bytes),
            "script_sha256": {name: digest(value) for name, value in scripts.items()},
            "steps": ["review guest baseline and disk reserve", "copy frozen inputs and verify every hash",
                      "build GNU Agent and Panel with compiled public TEST_ONLY roots",
                      "verify cgroup limits and exact source/binary identities",
                      "run normal Agent empty signed runtime lifecycle", "collect allowed receipts and verify cleanup"],
            "outside_scope": ["historical unsigned 0.1/0.2 migration", "Reality payload diagnosis",
                              "musl TCP artifact build", "production deployment", "full NodeQuality execution"]}
    write_new(output / "harness-identity.json", json_bytes(plan))
    print(json.dumps({"status": plan["status"], "output": str(output), "source_commit": args.source_commit,
                      "manifest_sha256": digest(json_bytes(plan))}))


if __name__ == "__main__":
    main()
