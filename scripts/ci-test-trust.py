#!/usr/bin/env python3
"""Select the deliberately public TEST_ONLY trust root for disposable CI builds."""

import argparse
import base64
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
FIXTURES = ROOT / "crates/protocol/tests/fixtures"


def public_keys():
    public = (FIXTURES / "TEST_ONLY.pub").read_text().splitlines()
    if len(public) != 2 or not public[0].startswith("untrusted comment: "):
        raise ValueError("invalid TEST_ONLY public fixture")
    material = base64.b64decode(public[1], validate=True)
    if len(material) != 42 or material[:2] != b"Ed":
        raise ValueError("invalid TEST_ONLY minisign record")
    roots = json.loads((FIXTURES / "public-keys.json").read_text())
    if roots != [public[1]]:
        raise ValueError("CI roots must exactly match the deliberately public signing fixture")
    return json.dumps(roots, separators=(",", ":"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--github-env", type=Path)
    args = parser.parse_args()
    value = public_keys()
    if args.github_env:
        with args.github_env.open("a") as output:
            output.write("APSP_RELEASE_PUBLIC_KEYS=" + value + "\n")
    else:
        print(value)


if __name__ == "__main__":
    main()
