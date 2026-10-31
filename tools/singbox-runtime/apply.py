#!/usr/bin/env python3
"""Apply the Apsp patch queue to a pinned sing-box checkout.

The result is committed with a fixed identity and date, so the commit hash is
a function of the upstream commit and the patches alone. Release verification
pins that hash, which binds a runtime binary to exactly this patch queue.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess

UTLS_MODULE = "github.com/metacubex/utls"
UTLS_VERSION = "v1.8.7"
VENDORED = "third_party/utls"
HERE = Path(__file__).resolve().parent
FIXED_DATE = "2026-11-01T00:00:00+00:00"


def run(arguments, source, **environment):
    return subprocess.run(
        arguments,
        cwd=source,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        env={**os.environ, "GOFLAGS": "", "GOTOOLCHAIN": "local", **environment},
    ).stdout


def git(source, *arguments, **environment):
    return run(
        ["git", "-c", "core.autocrlf=false", "-c", "core.filemode=false", *arguments],
        source,
        **environment,
    )


def apply(source, patches=HERE):
    requirement = rf"^\s*{re.escape(UTLS_MODULE)} {re.escape(UTLS_VERSION)}$"
    if not re.search(requirement, (source / "go.mod").read_text(), re.M):
        raise ValueError("upstream no longer pins the patched utls version; rebase the patches")
    module = json.loads(run(["go", "mod", "download", "-json", f"{UTLS_MODULE}@{UTLS_VERSION}"], source))
    vendored = source / VENDORED
    shutil.copytree(module["Dir"], vendored)
    for path in [vendored, *vendored.rglob("*")]:
        mode = stat.S_IMODE(path.stat().st_mode)
        path.chmod(0o755 if path.is_dir() else 0o644 if not mode & 0o111 else 0o755)
    git(source, "apply", f"--directory={VENDORED}", str(patches / "utls-reality-hardening.patch"))
    git(source, "apply", str(patches / "sing-box-reality-prepare.patch"))
    run(["go", "mod", "edit", f"-replace={UTLS_MODULE}=./{VENDORED}"], source)
    git(source, "add", "-A", "-f")
    identity = {
        "GIT_AUTHOR_NAME": "apsp-runtime",
        "GIT_AUTHOR_EMAIL": "apsp-runtime@invalid",
        "GIT_COMMITTER_NAME": "apsp-runtime",
        "GIT_COMMITTER_EMAIL": "apsp-runtime@invalid",
        "GIT_AUTHOR_DATE": FIXED_DATE,
        "GIT_COMMITTER_DATE": FIXED_DATE,
    }
    git(source, "commit", "--no-gpg-sign", "-q", "-m", "Apply Apsp runtime patches", **identity)
    return git(source, "rev-parse", "HEAD").strip()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    print(apply(parser.parse_args().source.resolve()))
