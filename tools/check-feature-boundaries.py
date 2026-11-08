#!/usr/bin/env python3
"""Report code that crosses domain boundaries and fail on new crossings (ADR 0128).

Ownership comes from tools/feature-boundaries.toml: every panel table has one owner, and
every Rust and web source path belongs to the platform, a domain or the application layer.
Owners are ordered by dependency (docs/architecture.md section 5.1). Checked crossings:

- sql: Rust code reading or writing a table of another owner. The only exception is reading
  a published identity table whose owner comes in an earlier group of the order;
- path: `#[path]` including a source file from another crate;
- ref: Rust code naming a module of another owner in the same crate (`crate::`, `super::`)
  against the dependency order: the platform naming a domain, a domain naming a later
  domain or a peer. Naming earlier owners is the normal direction; the application may
  name everything;
- import: a web domain importing another domain or the application shell, or the platform
  layer importing a domain or the shell. The shell composes and may import everything.

Known crossings are listed in tools/feature-boundaries.baseline. The check fails when a
crossing is not in that list; crossings that disappear are reported so the list can shrink
with --update, which never adds entries.
"""
import argparse
from collections import Counter
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "tools/feature-boundaries.toml"
BASELINE = ROOT / "tools/feature-boundaries.baseline"
SCHEMA = ROOT / "crates/panel/migrations/0001_current.sql"
EXACT = 1 << 20
# Test fixtures seed tables of every owner; only production code is held to the boundaries.
TEST_FILE = re.compile(r"(?:^|/)tests/|(?:^|/|_)tests?\.rs$")

SQL_HINT = re.compile(r"\b(select|insert|update|delete|with|truncate|lock)\b", re.I)
WRITES = re.compile(
    r"\b(?:insert\s+into|delete\s+from|truncate(?:\s+table)?|lock\s+table|alter\s+table|"
    r"drop\s+table(?:\s+if\s+exists)?)\s+(?:only\s+)?(?:public\.)?([a-z_][a-z0-9_]*)|"
    r"\bupdate\s+(?:only\s+)?(?:public\.)?([a-z_][a-z0-9_]*)\s+(?:as\s+)?(?:[a-z_][a-z0-9_]*\s+)?set\b", re.I)
READS = re.compile(r"\b(?:from|join)\s+(?:only\s+)?(?:public\.)?([a-z_][a-z0-9_]*)", re.I)
PATH_ATTR = re.compile(r'#\[path\s*=\s*"([^"]+)"\]')
IMPORT = re.compile(r"""(?:\bfrom\s+|\bimport\s*\(\s*|^\s*import\s+)['"]([^'"]+)['"]""", re.M)


def schema_objects(text):
    return set(re.findall(r"^CREATE (?:TABLE|VIEW|MATERIALIZED VIEW) public\.(\w+)", text, re.M))


def table_owner(name, rules):
    best = None
    for owner, patterns in rules.items():
        for pattern in patterns:
            if pattern.endswith("*"):
                if not name.startswith(pattern[:-1]):
                    continue
                score = len(pattern) - 1
            elif pattern == name:
                score = EXACT
            else:
                continue
            if best is None or score > best[0]:
                best = (score, owner)
            elif score == best[0] and owner != best[1]:
                raise ValueError(f"table {name} matches owners {best[1]} and {owner} equally")
    return best[1] if best else None


def ranks(config):
    """Dependency group index of every owner."""
    rank = {}
    for index, group in enumerate(config["domains"]["order"]):
        for owner in group:
            if owner in rank:
                raise ValueError(f"owner {owner} appears twice in the dependency order")
            rank[owner] = index
    return rank


def sql_allowed(mode, table, source, target, rank, published):
    """Owners use their own tables and read published tables of earlier groups only."""
    if source == target:
        return True
    return (mode == "read" and table in published
            and rank.get(target, len(rank)) < rank.get(source, -1))


def path_owner(rel, mapping):
    """Owner of the longest path prefix that matches, or None."""
    best = None
    for prefix, owner in mapping.items():
        if rel.startswith(prefix) and (best is None or len(prefix) > len(best[0])):
            best = (prefix, owner)
    return best[1] if best else None


def rust_strings(text):
    """String literal contents of Rust source, skipping comments, chars and lifetimes."""
    strings, i, n = [], 0, len(text)
    while i < n:
        c = text[i]
        if text.startswith("//", i):
            i = text.find("\n", i)
            i = n if i < 0 else i
        elif text.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif text.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
        elif (raw := re.compile(r'b?r(#*)"').match(text, i)) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            end = text.find('"' + raw.group(1), raw.end())
            end = n if end < 0 else end
            strings.append(text[raw.end():end])
            i = end + 1 + len(raw.group(1))
        elif c == '"' or (c == "b" and text.startswith('b"', i) and (i == 0 or not text[i - 1].isalnum())):
            i += 2 if c == "b" else 1
            start = i
            while i < n and text[i] != '"':
                i += 2 if text[i] == "\\" else 1
            strings.append(text[start:i])
            i += 1
        elif c == "'" and (char := re.compile(r"'(?:\\.[^']*|[^'\\])'").match(text, i)):
            i = char.end()
        else:
            i += 1
    return strings


def rust_code(text):
    """Rust source with comments, string and char literals blanked; offsets and lines stay."""
    out, i, n = list(text), 0, len(text)

    def blank(start, end):
        for k in range(start, min(end, n)):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        if text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
            blank(i, end)
            i = end
        elif text.startswith("/*", i):
            start, depth, i = i, 1, i + 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif text.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            blank(start, i)
        elif (raw := re.compile(r'b?r(#*)"').match(text, i)) and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            end = text.find('"' + raw.group(1), raw.end())
            end = n if end < 0 else end + 1 + len(raw.group(1))
            blank(i, end)
            i = end
        elif c == '"' or (c == "b" and text.startswith('b"', i) and (i == 0 or not text[i - 1].isalnum())):
            start = i
            i += 2 if c == "b" else 1
            while i < n and text[i] != '"':
                i += 2 if text[i] == "\\" else 1
            i += 1
            blank(start, i)
        elif c == "'" and (char := re.compile(r"'(?:\\.[^']*|[^'\\])'").match(text, i)):
            blank(i, char.end())
            i = char.end()
        else:
            i += 1
    return "".join(out)


MODULE_REF = re.compile(r"(?<![\w:])((?:crate|super)(?:\s*::\s*super)*)\s*::\s*")
IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def use_paths(code, start):
    """Expand the path or use tree that starts at `start` into lists of segments."""
    def tree(i):
        paths, segments = [], []
        while True:
            while i < len(code) and code[i].isspace():
                i += 1
            if i < len(code) and code[i] == "{":
                i += 1
                while True:
                    sub, i = tree(i)
                    paths += [segments + s for s in sub]
                    while i < len(code) and code[i].isspace():
                        i += 1
                    if i < len(code) and code[i] == ",":
                        i += 1
                        continue
                    if i < len(code) and code[i] == "}":
                        i += 1
                    return paths or [segments], i
            ident = IDENT.match(code, i)
            if not ident:
                return [segments], i
            segments.append(ident.group())
            i = ident.end()
            sep = re.compile(r"\s*::\s*").match(code, i)
            if sep and code.startswith("*", sep.end()):
                return [segments], sep.end() + 1
            if not sep or (sep.end() < len(code) and code[sep.end()] not in "{" and not IDENT.match(code, sep.end())):
                return [segments], i
            i = sep.end()
    return tree(start)[0]


def crate_source(crate_dir):
    """Directory that holds the crate root file, from `[lib] path` or `src/lib.rs`."""
    manifest = crate_dir / "Cargo.toml"
    try:
        data = tomllib.loads(manifest.read_text())
    except (OSError, tomllib.TOMLDecodeError):
        return None
    if "package" not in data:
        return None
    lib = data.get("lib", {}).get("path")
    root_file = crate_dir / (lib or "src/lib.rs")
    if not root_file.exists():
        root_file = crate_dir / "src/main.rs"
    return root_file if root_file.exists() else None


def module_of(path, root_file):
    """Module path (list of segments) of a source file inside its crate."""
    base = root_file.parent
    if path == root_file:
        return []
    parts = list(path.relative_to(base).with_suffix("").parts)
    return parts[:-1] if parts[-1] == "mod" else parts


def resolve_module(segments, root_file):
    """The deepest source file that defines a prefix of `segments`, or None."""
    base = root_file.parent
    for k in range(len(segments), 0, -1):
        prefix = base.joinpath(*segments[:k])
        for candidate in (prefix.with_suffix(".rs"), prefix / "mod.rs"):
            if candidate.is_file():
                return candidate
    return None


def module_refs(path, code, root_file):
    """Source files of the same crate that `crate::` and `super::` paths in `code` name."""
    targets, here = set(), module_of(path, root_file)
    for match in MODULE_REF.finditer(code):
        head = re.sub(r"\s+", "", match.group(1)).split("::")
        if head[0] == "crate":
            base = []
        elif len(head) <= len(here):
            base = here[:len(here) - len(head)]
        else:
            continue
        for segments in use_paths(code, match.end()):
            target = resolve_module(base + segments, root_file) if base + segments else None
            if target is not None and target != path:
                targets.add(target)
    return targets


def ref_allowed(source, target, rank):
    """Code may name its own owner, earlier owners, and the application everything."""
    return source == target or source == "app" or rank.get(target, len(rank)) < rank.get(source, -1)


def sql_accesses(literal, known):
    """(mode, table) pairs for one SQL literal; only known tables and views count."""
    if not SQL_HINT.search(literal):
        return set()
    found, spans = set(), []
    for match in WRITES.finditer(literal):
        spans.append(match.span())
        name = (match.group(1) or match.group(2)).lower()
        if name in known:
            found.add(("write", name))
    for match in READS.finditer(literal):
        if any(start <= match.start() < end for start, end in spans):
            continue
        if match.group(1).lower() in known:
            found.add(("read", match.group(1).lower()))
    return found


def crate_root(path):
    for parent in path.parents:
        if (parent / "Cargo.toml").exists():
            return parent
    return None


def rust_crossings(root, config, owners):
    crossings = set()
    rank, published = ranks(config), set(config["domains"].get("published", []))
    exempt = config.get("rust_exempt", {}).get("paths", [])
    for path in sorted(root.glob("**/*.rs")):
        rel = path.relative_to(root).as_posix()
        if "/target/" in f"/{rel}" or rel.startswith("target/") or any(rel.startswith(p) for p in exempt):
            continue
        owner = path_owner(rel, config["rust"])
        if owner is None or TEST_FILE.search(rel):
            continue
        text = path.read_text(errors="replace")
        for include in PATH_ATTR.findall(text):
            target = (path.parent / include).resolve()
            if crate_root(target) != crate_root(path):
                target_rel = target.relative_to(root.resolve()).as_posix()
                crossings.add(f"path\t{rel}\t{target_rel}\t{owner}->{path_owner(target_rel, config['rust'])}")
        crate_dir = crate_root(path)
        root_file = crate_source(crate_dir) if crate_dir is not None else None
        # Examples, benches and build scripts are separate targets with their own `crate`.
        if root_file is not None and path.is_relative_to(root_file.parent):
            for target in module_refs(path, rust_code(text), root_file):
                target_rel = target.relative_to(root).as_posix()
                target_owner = path_owner(target_rel, config["rust"])
                if target_owner is not None and not ref_allowed(owner, target_owner, rank):
                    crossings.add(f"ref\t{rel}\t{target_rel}\t{owner}->{target_owner}")
        for literal in rust_strings(text):
            for mode, table in sql_accesses(literal, owners):
                target = owners[table]
                if not sql_allowed(mode, table, owner, target, rank, published):
                    crossings.add(f"sql\t{rel}\t{mode}\t{table}\t{owner}->{target}")
    return crossings


def resolve_import(path, spec, root):
    web = root / "web/src"
    if spec.startswith("@/"):
        base = web / spec[2:]
    elif spec.startswith("."):
        base = (path.parent / spec).resolve()
    else:
        return None
    for candidate in (base, *(base.with_name(base.name + ext) for ext in (".ts", ".tsx", ".js", ".mjs")),
                      *(base / f"index{ext}" for ext in (".ts", ".tsx"))):
        if candidate.is_file():
            return candidate.resolve()
    return None


def web_crossings(root, config):
    crossings = set()
    web = root / "web/src"
    for path in sorted(list(web.glob("**/*.ts")) + list(web.glob("**/*.tsx"))):
        rel = path.relative_to(root).as_posix()
        source = path_owner(rel, config["web"])
        for spec in IMPORT.findall(path.read_text(errors="replace")):
            target_path = resolve_import(path, spec, root)
            if target_path is None:
                continue
            target_rel = target_path.relative_to(root.resolve()).as_posix()
            target = path_owner(target_rel, config["web"])
            if source == target or target == "platform" or source == "app":
                continue
            crossings.add(f"import\t{rel}\t{target_rel}\t{source}->{target}")
    return crossings


def ownership(root, config):
    """Owner of every schema object, plus configuration problems."""
    known = schema_objects((root / SCHEMA.relative_to(ROOT)).read_text())
    owners, problems = {}, []
    try:
        rank = ranks(config)
    except (KeyError, ValueError) as error:
        return owners, [f"invalid dependency order in {CONFIG.relative_to(ROOT)}: {error}"]
    for name in sorted(known):
        owner = table_owner(name, config["tables"])
        if owner is None:
            problems.append(f"table {name} has no owner in {CONFIG.relative_to(ROOT)}")
        else:
            owners[name] = owner
    named = set(config["tables"]) | set(config.get("rust", {}).values()) | set(config.get("web", {}).values())
    problems += [f"owner {owner} is not in the dependency order in {CONFIG.relative_to(ROOT)}"
                 for owner in sorted(named - set(rank))]
    problems += [f"published table {name} is not in the schema"
                 for name in config["domains"].get("published", []) if name not in known]
    return owners, problems


def read_baseline(path):
    if not path.exists():
        return set()
    return {line for line in path.read_text().splitlines() if line and not line.startswith("#")}


def write_baseline(path, entries):
    header = ["# Known domain-boundary crossings (ADR 0128). Generated by tools/check-feature-boundaries.py --update.",
              "# Entries may only be removed; new crossings fail the check."]
    path.write_text("\n".join(header + sorted(entries)) + "\n")


def summary(crossings):
    kinds = Counter(entry.split("\t", 1)[0] for entry in crossings)
    pairs = Counter(entry.rsplit("\t", 1)[1] for entry in crossings)
    lines = ["crossings: " + ", ".join(f"{kind} {count}" for kind, count in sorted(kinds.items()))]
    lines += [f"  {pair}: {count}" for pair, count in pairs.most_common()]
    return "\n".join(lines)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--update", action="store_true", help="remove fixed crossings from the baseline")
    parser.add_argument("--summary", action="store_true", help="print counts by kind and owner pair")
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    root = args.root.resolve()
    config = tomllib.loads((root / CONFIG.relative_to(ROOT)).read_text())
    owners, problems = ownership(root, config)
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    crossings = rust_crossings(root, config, owners) | web_crossings(root, config)
    baseline_path = root / BASELINE.relative_to(ROOT)
    baseline = read_baseline(baseline_path)
    new, fixed = crossings - baseline, baseline - crossings
    if args.summary:
        print(summary(crossings))
    if new and baseline_path.exists():
        print(f"{len(new)} new domain-boundary crossing(s); move the code or go through the owner's interface:", file=sys.stderr)
        print("\n".join(sorted(new)), file=sys.stderr)
        return 1
    if args.update or not baseline_path.exists():
        write_baseline(baseline_path, crossings if not baseline_path.exists() else baseline & crossings)
        print(f"baseline: {len(crossings if not baseline else baseline & crossings)} known crossing(s)")
        return 0
    if fixed:
        print(f"{len(fixed)} crossing(s) no longer present; run with --update to shrink the baseline.")
    print(f"domain boundaries: {len(crossings)} known crossing(s), none new.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
