#!/usr/bin/env python3
"""Prepare the pinned upstream Chromium/musl toolchain without shell evaluation."""
import argparse
from pathlib import Path
import subprocess
import sys

COMMIT = "0d28acc44093df24b2526dea3d6ffefd6b0a54f0"


def prepare(arch, root):
    root = root.resolve()
    root.mkdir()
    def run(args, capture=False):
        return subprocess.run(args, cwd=root, check=True, text=True,
                              stdout=subprocess.PIPE if capture else sys.stderr).stdout
    run(["git", "init", "--quiet"])
    run(["git", "remote", "add", "origin", "https://github.com/SagerNet/cronet-go.git"])
    run(["git", "sparse-checkout", "set", "--no-cone", "/*", "!/lib"])
    run(["git", "fetch", "--depth=1", "--filter=blob:none", "origin", COMMIT])
    run(["git", "checkout", "--detach", "FETCH_HEAD"])
    if run(["git", "rev-parse", "HEAD"], True).strip() != COMMIT:
        raise ValueError("unexpected Cronet toolchain source")
    run(["git", "submodule", "update", "--init", "--recursive", "--depth=1", "--filter=blob:none"])
    command = ["go", "run", "./cmd/build-naive", "--target=linux/" + arch, "--libc=musl"]
    run(command + ["download-toolchain"])
    environment = {}
    for line in run(command + ["env"], True).splitlines():
        key, separator, value = line.partition("=")
        if not separator or key not in {"CC", "CXX", "CGO_LDFLAGS", "QEMU_LD_PREFIX"} or key in environment:
            raise ValueError("unexpected toolchain environment")
        environment[key] = value
    if set(environment) != {"CC", "CXX", "CGO_LDFLAGS", "QEMU_LD_PREFIX"}:
        raise ValueError("incomplete toolchain environment")
    for key, value in environment.items():
        print(f"{key}={value}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("arch", choices=["amd64", "arm64"])
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    prepare(args.arch, args.directory)
