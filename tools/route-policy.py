#!/usr/bin/env python3
"""Keeps the route policy table in step with the routes the panel registers.

`crates/panel/tests/fixtures/route-policy.tsv` holds one line per registered route and
method with the request guard's verdict for it (who may call it, in which server scope,
whether a recent proof of identity is required). The Rust test `route_policy` compares the
verdicts with the guard; this tool compares the list of routes with the source.

    python3 tools/route-policy.py            # fail when a route is missing from or stale in the table
    python3 tools/route-policy.py --write    # add missing routes (verdict "?") and drop stale ones

After `--write`, fill the verdicts with
`APSP_BLESS_ROUTE_POLICY=1 cargo test -p apsp-panel --test route_policy` and review the diff.
"""
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
TABLE = ROOT / "crates/panel/tests/fixtures/route-policy.tsv"
TEST_FILE = re.compile(r"(?:^|/)tests/|(?:^|/|_)tests?\.rs$")
ROUTE = re.compile(r"\.route\(\s*\"([^\"]+)\"\s*,")
METHODS = "get|post|put|patch|delete|head|options"
METHOD = re.compile(rf"(?<![A-Za-z0-9_])({METHODS})\s*\(")
# A routing function imported under another name, as in `use axum::routing::{get as route_get}`.
ALIAS = re.compile(rf"\b({METHODS})\s+as\s+([A-Za-z_][A-Za-z0-9_]*)")
ROUTING_USE = re.compile(r"use\s+axum::routing::(?:\{[^}]*\}|[^;]*);")
TEST_MODULE = re.compile(r"#\[cfg\(test\)\]\s*(?:pub(?:\([a-z]+\))?\s+)?mod\s+\w+\s*\{")


def without_test_modules(text):
    """Source text with inline `#[cfg(test)] mod name { ... }` bodies blanked."""
    out, position = [], 0
    for match in TEST_MODULE.finditer(text):
        if match.start() < position:
            continue
        depth, index = 1, match.end()
        while index < len(text) and depth:
            depth += {"{": 1, "}": -1}.get(text[index], 0)
            index += 1
        out.append(text[position:match.start()])
        position = index
    out.append(text[position:])
    return "".join(out)


def call_end(text, start):
    """Index just past the parenthesis that closes the call opened before `start`."""
    depth, index = 1, start
    while index < len(text) and depth:
        depth += {"(": 1, ")": -1}.get(text[index], 0)
        index += 1
    return index


TEST_ONLY_MODULE = re.compile(r"#\[cfg\(test\)\]\s*(?:pub(?:\([a-z]+\))?\s+)?mod\s+(\w+)\s*;")


def test_only_paths(root):
    """Files and directories of modules that their parent declares under `#[cfg(test)]`."""
    paths = set()
    for path in root.glob("crates/*/src/**/*.rs"):
        text = path.read_text(errors="replace")
        if TEST_FILE.search(path.relative_to(root).as_posix()):
            for included in re.findall(r'#\[path\s*=\s*"([^"]+)"\]', text):
                paths.add((path.parent / included).resolve())
        base = path.parent if path.name in ("lib.rs", "main.rs", "mod.rs") else path.with_suffix("")
        for match in TEST_ONLY_MODULE.finditer(text):
            paths.add((base / match.group(1)).with_suffix(".rs"))
            paths.add(base / match.group(1))
    return paths


def registered(root=ROOT):
    routes = set()
    test_only = test_only_paths(root)
    for path in sorted(root.glob("crates/*/src/**/*.rs")):
        rel = path.relative_to(root).as_posix()
        if TEST_FILE.search(rel) or path in test_only or any(parent in test_only for parent in path.parents):
            continue
        text = without_test_modules(path.read_text(errors="replace"))
        aliases = {
            alias: method
            for statement in ROUTING_USE.findall(text)
            for method, alias in ALIAS.findall(statement)
        }
        methods = METHOD
        if aliases:
            names = "|".join([METHODS, *map(re.escape, aliases)])
            methods = re.compile(rf"(?<![A-Za-z0-9_])({names})\s*\(")
        for match in ROUTE.finditer(text):
            route = match.group(1)
            if not route.startswith("/api/"):
                continue
            handlers = text[match.end():call_end(text, match.end()) - 1]
            for method in methods.finditer(handlers):
                name = method.group(1)
                routes.add((aliases.get(name, name).upper(), route))
    return routes


def read_table(path=TABLE):
    rows = {}
    if not path.is_file():
        return rows
    for line in path.read_text().splitlines():
        if not line or line.startswith("#"):
            continue
        method, route, verdict = line.split("\t")
        rows[(method, route)] = verdict
    return rows


HEADER = (
    "# Route policy table: method, route, the request guard's verdict. Generated; see tools/route-policy.py.\n"
)


def write_table(rows, path=TABLE):
    lines = [f"{method}\t{route}\t{rows[(method, route)]}" for method, route in sorted(rows, key=lambda key: (key[1], key[0]))]
    path.write_text(HEADER + "\n".join(lines) + "\n")


def main(argv):
    routes = registered()
    rows = read_table()
    missing = sorted(routes - set(rows))
    stale = sorted(set(rows) - routes)
    if "--write" in argv:
        for key in missing:
            rows[key] = "?"
        for key in stale:
            del rows[key]
        write_table(rows)
        print(f"route policy table: {len(rows)} routes ({len(missing)} added, {len(stale)} dropped)")
        return 0
    for method, route in missing:
        print(f"missing from the route policy table: {method} {route}")
    for method, route in stale:
        print(f"no longer registered: {method} {route}")
    if missing or stale:
        print("run `python3 tools/route-policy.py --write`, bless the verdicts and review the diff")
        return 1
    print(f"route policy table covers {len(rows)} routes")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
