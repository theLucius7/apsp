#!/usr/bin/env python3
"""Package an explicit offline artifact; no diagnostics, downloads or signing."""
import argparse
import contextlib
import hashlib
import os
from pathlib import Path
import re
import stat
import signal
import sys
import tempfile

sys.dont_write_bytecode = True
import nodequality_rootfs_artifact as artifact
import release


@contextlib.contextmanager
def publication_signals():
    signals = {signal.SIGINT, signal.SIGTERM, signal.SIGHUP}
    previous = signal.pthread_sigmask(signal.SIG_BLOCK, signals)
    try:
        yield
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous)


@contextlib.contextmanager
def cli_signals():
    def interrupted(number, _frame):
        raise SystemExit(128 + number)
    previous = {number: signal.getsignal(number) for number in (signal.SIGTERM, signal.SIGHUP)}
    try:
        for number in previous:
            signal.signal(number, interrupted)
        yield
    finally:
        for number, handler in previous.items():
            signal.signal(number, handler)


def ordinary(path, limit):
    path = Path(path).absolute()
    parent = artifact.runtime()._directory(str(path.parent))
    try:
        descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
    finally:
        os.close(parent)
    with os.fdopen(descriptor, "rb") as source:
        metadata = os.fstat(source.fileno())
        artifact.ensure(stat.S_ISREG(metadata.st_mode) and 0 < metadata.st_size <= limit,
                        "build input must be a bounded ordinary file")
        content = source.read(limit + 1)
    artifact.ensure(0 < len(content) <= limit, "build input exceeds byte limit")
    return content


def output_directory(path):
    path = Path(path).absolute()
    artifact.ensure(all(part not in (".", "..") for part in path.parts[1:]), "output path traversal is forbidden")
    descriptor = os.open(path.anchor, os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in path.parts[1:]:
            try:
                child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=descriptor)
            except FileNotFoundError:
                try:
                    os.mkdir(part, 0o755, dir_fd=descriptor)
                except FileExistsError:
                    pass
                child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=descriptor)
            os.close(descriptor)
            descriptor = child
        metadata = os.fstat(descriptor)
        artifact.ensure(metadata.st_uid == os.geteuid() and not metadata.st_mode & 0o022,
                        "offline output must be owned and not writable by another account")
    finally:
        os.close(descriptor)
    return path


def checksums(directory):
    manifest = directory / "SHA256SUMS"
    expected = {}
    if manifest.exists() or manifest.is_symlink():
        for line in ordinary(manifest, 1024).decode().splitlines():
            match = re.fullmatch(r"([0-9a-f]{64})  (amd64|arm64)", line)
            artifact.ensure(match is not None and match[2] not in expected,
                            "invalid offline checksum inventory")
            expected[match[2]] = match[1]
    for arch in ("amd64", "arm64"):
        path = directory / arch
        if path.exists() or path.is_symlink():
            data = ordinary(path, artifact.MAX_OUTER_STREAM)
            artifact.ensure(expected.get(arch) == artifact.digest(data),
                            "existing architecture lacks its matching immutable checksum")
        else:
            artifact.ensure(arch not in expected, "checksum inventory refers to a missing architecture")
    return expected


def build(args):
    artifact.ensure(re.fullmatch(r"[0-9a-f]{64}", args.approved_builder_image_sha256) is not None,
                    "an independently approved builder image SHA256 is required")
    builder = artifact.module("apsp_rootfs_preparation", artifact.ROOT / "tools/nodequality-rootfs-build.py")
    prepared = builder.verify_prepared(args.prepared_directory, args.approved_builder_image_sha256)
    receipt = builder.verify_export(args.rootfs_directory, prepared, args.arch)
    legacy = release.binary_bytes(ordinary(args.legacy_artifact, artifact.MAX_RUNNER),
                                  "tar.gz", artifact.BINARY)
    files = {artifact.BINARY: artifact.offline_runner(legacy),
             "rootfs.tar.gz": ordinary(args.rootfs_directory / "rootfs.tar.gz", artifact.MAX_OUTER_STREAM),
             "rootfs-manifest.json": ordinary(args.rootfs_directory / "rootfs-manifest.json", artifact.MAX_MANIFEST)}
    artifact.ensure(receipt["archive"]["sha256"] == artifact.digest(files["rootfs.tar.gz"])
                    and receipt["manifest"]["sha256"] == artifact.digest(files["rootfs-manifest.json"]),
                    "offline export changed after its preparation verification")
    artifact.validate_files(files, artifact.VERSION, args.arch)
    data = artifact.pack(files)
    # The original strict outer parser also checks tail bytes and the complete
    # auxiliary inventory. Inner safety is independently checked above.
    auxiliary = {name: {"size": len(content), "sha256": artifact.digest(content)}
                 for name, content in files.items() if name != artifact.BINARY}
    release.binary_bytes(data, "tar.gz", artifact.BINARY, auxiliary)
    output = output_directory(args.output / "nodequality" / artifact.VERSION)
    lock = output / ".build.lock"
    temporary = []
    locked = False
    created = False
    published = False
    try:
        with publication_signals():
            lock.mkdir(mode=0o700)
            locked = True
        expected = checksums(output)
        target = output / args.arch
        artifact.ensure(not target.exists() and not target.is_symlink(), "immutable offline artifact already exists")
        with publication_signals():
            stage = tempfile.NamedTemporaryFile(prefix=".artifact-", dir=output, delete=False)
            temporary.append(Path(stage.name))
        with stage:
            stage.write(data)
            stage.flush()
            os.fsync(stage.fileno())
            os.fchmod(stage.fileno(), 0o644)
        with publication_signals():
            os.link(temporary[-1], target)
            created = True
        expected[args.arch] = hashlib.sha256(data).hexdigest()
        with publication_signals():
            sums = tempfile.NamedTemporaryFile(prefix=".sums-", dir=output, delete=False)
            temporary.append(Path(sums.name))
        with sums:
            sums.write("".join(f"{expected[arch]}  {arch}\n" for arch in sorted(expected)).encode())
            sums.flush()
            os.fsync(sums.fileno())
            os.fchmod(sums.fileno(), 0o644)
        with publication_signals():
            os.replace(temporary[-1], output / "SHA256SUMS")
            published = True
        return target
    finally:
        if created and not published:
            (output / args.arch).unlink(missing_ok=True)
        for path in temporary:
            path.unlink(missing_ok=True)
        if locked:
            lock.rmdir()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", choices=("amd64", "arm64"), required=True)
    parser.add_argument("--legacy-artifact", type=Path, required=True,
                        help="unchanged canonical r19 artifact built from this checkout")
    parser.add_argument("--rootfs-directory", type=Path, required=True,
                        help="verified rootfs.tar.gz and rootfs-manifest.json export")
    parser.add_argument("--prepared-directory", type=Path, required=True,
                        help="authenticated snapshot indices, complete package and source cache")
    parser.add_argument("--approved-builder-image-sha256", required=True,
                        help="independently reviewed immutable builder identity")
    parser.add_argument("--output", type=Path, required=True)
    target = build(parser.parse_args())
    print(target)


if __name__ == "__main__":
    try:
        with cli_signals():
            main()
    except (OSError, ValueError, TypeError, KeyError) as error:
        raise SystemExit("Error: " + str(error)) from None
