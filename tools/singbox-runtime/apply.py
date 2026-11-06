#!/usr/bin/env python3
"""Apply the Apsp patch queue to a pinned sing-box checkout.

The queue rebuilds the kernel the legacy service ran: upstream v1.14.0-alpha.2,
then the two commits of github.com/cedar2025/sing-box up to 2e665cb7, then the
dependency versions that the legacy binaries were linked with.

The result is committed with a fixed identity and date, so the commit hash is
a function of the upstream commit and the patches alone. Release verification
pins that hash, which binds a runtime binary to exactly this patch queue.
"""
import argparse
import os
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
FORK_PATCH = "cedar2025-2e665cb7.patch"
DEPENDENCY_PATCH = "legacy-dependencies.patch"
# Tree of github.com/cedar2025/sing-box commit 2e665cb7e295949ba7c5536f9b7754f94ab78cee.
FORK_TREE = "ce5b9a70ee94c864fce30776c3287107c6c6a579"
FIXED_DATE = "2026-11-06T00:00:00+00:00"


def git(source, *arguments, **environment):
    return subprocess.run(
        ["git", "-c", "core.autocrlf=false", "-c", "core.filemode=false", *arguments],
        cwd=source,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        env={**os.environ, **environment},
    ).stdout


def apply(source, patches=HERE):
    git(source, "apply", "--whitespace=nowarn", str(patches / FORK_PATCH))
    git(source, "add", "-A", "-f")
    if git(source, "write-tree").strip() != FORK_TREE:
        raise ValueError("the fork patch no longer reproduces cedar2025/sing-box 2e665cb7")
    git(source, "apply", str(patches / DEPENDENCY_PATCH))
    git(source, "add", "-A", "-f")
    identity = {
        "GIT_AUTHOR_NAME": "apsp-runtime",
        "GIT_AUTHOR_EMAIL": "apsp-runtime@invalid",
        "GIT_COMMITTER_NAME": "apsp-runtime",
        "GIT_COMMITTER_EMAIL": "apsp-runtime@invalid",
        "GIT_AUTHOR_DATE": FIXED_DATE,
        "GIT_COMMITTER_DATE": FIXED_DATE,
    }
    git(source, "commit", "--no-gpg-sign", "-q", "-m", "Rebuild the legacy service kernel", **identity)
    return git(source, "rev-parse", "HEAD").strip()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    print(apply(parser.parse_args().source.resolve()))
