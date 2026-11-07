#!/usr/bin/env python3
"""Apply the APSP user limits patch to the pinned stable sing-box checkout."""
import argparse
import os
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
UPSTREAM = "c005f35300ce76427dc285481ec5695f5b179cd2"
FIXED_DATE = "2026-11-07T00:00:00+00:00"


def git(source, *arguments, **environment):
    return subprocess.run(
        ["git", "-c", "core.autocrlf=false", "-c", "core.filemode=false", *arguments],
        cwd=source, check=True, text=True, stdout=subprocess.PIPE,
        env={**os.environ, **environment},
    ).stdout


def apply(source, patches=HERE):
    if git(source, "rev-parse", "HEAD").strip() != UPSTREAM:
        raise ValueError("expected the pinned v1.14.4 upstream commit")
    if git(source, "status", "--porcelain").strip():
        raise ValueError("runtime checkout must be clean before applying patches")
    git(source, "apply", str(patches / "user-limits.patch"))
    git(source, "apply", str(patches / "sessions.patch"))
    git(source, "add", "-A", "-f")
    identity = {
        "GIT_AUTHOR_NAME": "imengying", "GIT_AUTHOR_EMAIL": "yuzuzv@gmail.com",
        "GIT_COMMITTER_NAME": "imengying", "GIT_COMMITTER_EMAIL": "yuzuzv@gmail.com",
        "GIT_AUTHOR_DATE": FIXED_DATE, "GIT_COMMITTER_DATE": FIXED_DATE,
    }
    git(source, "commit", "--no-gpg-sign", "-q", "-m", "feat: enforce APSP limits and expose authenticated sessions", **identity)
    return git(source, "rev-parse", "HEAD").strip()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    print(apply(parser.parse_args().source.resolve()))
