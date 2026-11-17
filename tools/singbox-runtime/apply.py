#!/usr/bin/env python3
"""Apply the APSP patch queue to the pinned stable sing-box checkout.

The result is committed with a fixed identity and date, so the commit hash is a
function of the upstream commit and the patches alone. Release verification
pins that hash, which binds a runtime binary to exactly this patch queue.

`utls-reality-hardening.patch` changes the REALITY server in
github.com/metacubex/utls. The module is copied from the verified Go module
cache into third_party/utls, patched there and wired in with a go.mod replace,
so the vendored source is part of the pinned commit as well.
"""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess

HERE = Path(__file__).resolve().parent
UPSTREAM = "c005f35300ce76427dc285481ec5695f5b179cd2"
FIXED_DATE = "2026-11-17T00:00:00+00:00"
UTLS_MODULE = "github.com/metacubex/utls"
UTLS_VERSION = "v1.8.7"
VENDORED = "third_party/utls"


def run(arguments, source, **environment):
    return subprocess.run(
        arguments, cwd=source, check=True, text=True, stdout=subprocess.PIPE,
        env={**os.environ, "GOFLAGS": "", "GOTOOLCHAIN": "local", **environment},
    ).stdout


def git(source, *arguments, **environment):
    return run(["git", "-c", "core.autocrlf=false", "-c", "core.filemode=false", *arguments], source, **environment)


def vendor_utls(source, patches):
    requirement = rf"^\s*{re.escape(UTLS_MODULE)} {re.escape(UTLS_VERSION)}$"
    if not re.search(requirement, (source / "go.mod").read_text(), re.M):
        raise ValueError("upstream no longer pins the patched utls version; rebase the patches")
    module = json.loads(run(["go", "mod", "download", "-json", f"{UTLS_MODULE}@{UTLS_VERSION}"], source))
    if module.get("Error") or not module.get("Dir"):
        raise ValueError("could not obtain the pinned utls module")
    vendored = source / VENDORED
    shutil.copytree(module["Dir"], vendored)
    # The module cache is read-only and drops executable bits; normalise so the
    # committed tree (and therefore the pinned hash) is the same on every host.
    for path in [vendored, *vendored.rglob("*")]:
        path.chmod(0o755 if path.is_dir() else 0o644)
    git(source, "apply", f"--directory={VENDORED}", str(patches / "utls-reality-hardening.patch"))
    git(source, "apply", str(patches / "sing-box-reality-prepare.patch"))
    run(["go", "mod", "edit", f"-replace={UTLS_MODULE}={'./' + VENDORED}"], source)


def apply(source, patches=HERE):
    if git(source, "rev-parse", "HEAD").strip() != UPSTREAM:
        raise ValueError("expected the pinned v1.14.4 upstream commit")
    if git(source, "status", "--porcelain").strip():
        raise ValueError("runtime checkout must be clean before applying patches")
    git(source, "apply", str(patches / "user-limits.patch"))
    git(source, "apply", str(patches / "sessions.patch"))
    vendor_utls(source, patches)
    git(source, "add", "-A", "-f")
    identity = {
        "GIT_AUTHOR_NAME": "apsp-runtime", "GIT_AUTHOR_EMAIL": "apsp-runtime@invalid",
        "GIT_COMMITTER_NAME": "apsp-runtime", "GIT_COMMITTER_EMAIL": "apsp-runtime@invalid",
        "GIT_AUTHOR_DATE": FIXED_DATE, "GIT_COMMITTER_DATE": FIXED_DATE,
    }
    git(source, "commit", "--no-gpg-sign", "-q", "-m",
        "feat: enforce APSP limits, expose authenticated sessions and harden REALITY", **identity)
    return git(source, "rev-parse", "HEAD").strip()


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path)
    print(apply(parser.parse_args().source.resolve()))
