#!/usr/bin/env python3
"""Reject proxy business references in core while preserving native account APIs.

Two cores are checked: the Agent core (`crates/agent-core`) and the panel host
(`crates/panel-host`). Crate manifests are also checked so that plugins depend
on the host and never the reverse.
"""
import argparse
from pathlib import Path
import re
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
FORBIDDEN = {"user", "users", "subscription", "subscriptions", "quota", "quotas"}
TOKENS = re.compile(r"[A-Za-z][A-Za-z0-9_]*")
CAMEL_PARTS = re.compile(r"_|(?<=[a-z0-9])(?=[A-Z])|(?<=[A-Z])(?=[A-Z][a-z])")

# Exact operating-system/SQLite API spellings, scoped to their existing callers.
# Mask only these expressions: a forbidden reference on the same line still fails.
EXCEPTIONS = {
    "src/system/jobs.rs": (
        r'(?<=--property=)CPUQuota(?==)',
        r'(?<=--property=)CPUQuotaPeriodSec(?==100ms")',
    ),
    "src/system/budgets.rs": (
        r'(?<=\(")CPUQuota(?=", format!)',
        r'(?<=\(")CPUQuotaPeriodSec(?=", "100ms"\.into\(\))',
    ),
    "src/system/tests.rs": (
        r'(?<=CPUWeight,)CPUQuotaPerSecUSec(?=,IOWeight,OOMScoreAdjust")',
        r'(?<=\(")CPUQuotaPerSecUSec(?=", "1s"\.into\(\))',
    ),
    "src/system/services.rs": (
        r'(?<=CPUUsageNSec,)User(?=,Group")',
        r'(?<=LoadState,MainPID,)User(?=,Group,SupplementaryGroups")',
        r'properties\s*\.get\("User"\)',
    ),
    "src/system_network/certificates.rs": (
        r'properties\s*\.get\("User"\)',
        r'(?<=loaded\.insert\(")User(?="\.into\(\),)',
    ),
    "src/system_network/firewall.rs": (
        r'(?<=WantedBy=)multi-user\.target(?=\\n")',
    ),
    "src/system_network/tunnel.rs": (
        r'(?<=format!\(")UserKnownHostsFile(?==\{\}")',
    ),
    "src/system/windows.rs": (
        r"\[Security\.Principal\.WindowsIdentity\]::GetCurrent\(\)\.User\b",
    ),
    "src/system/deploy/native/windows.rs": (
        r"\[Security\.Principal\.WindowsIdentity\]::GetCurrent\(\)\.User\b",
        r"\b(?:Get|New|Set|Enable)-LocalUser\b",
        r"(?<![\w-])-(?:UserMayNotChangePassword|UserId|User)\b",
        r"\bUSER_RIGHTS\b",
    ),
    "src/system/deploy/native/unix.rs": (
        r'(?<=")/Users(?=/|")',
        r'"UserShell"',
        r"<key>UserName</key>",
    ),
    "tests/usage.rs": (r"\bPRAGMA user_version\b",),
    "tests/usage_bounds.rs": (
        r"\bPRAGMA user_version\b",
        r'\.pragma_update\(None, "user_version",',
    ),
}

# Panel host: HTTP and Windows API spellings plus URL userinfo test fixtures.
HOST_EXCEPTIONS = {
    "src/config.rs": (r'"https://user:secret@example\.invalid"',),
    "src/diagnostics/tests.rs": (r'"https://user:secret@nodequality\.com/r/x"',),
    "src/exchange/fetch.rs": (r"\.user_agent\(",),
    "src/installation/windows.rs": (
        r"\[Security\.Principal\.WindowsIdentity\]::GetCurrent\(\)\.User\b",
        r"\.DefaultRequestHeaders\.UserAgent\b",
    ),
    "src/ip_quality/providers/tests.rs": (r'contains_key\("user-agent"\)',),
    "src/releases/network.rs": (r"\.user_agent\(", r'"https://user@github\.com/"'),
    "src/network_workbench/http_probe.rs": (r"\breqwest::header::USER_AGENT\.as_str\(\)",),
    # Wire/storage names for external provider request limits, never proxy plans.
    "src/network_workbench/reports.rs": (
        r'(?<=serde\(rename = ")daily_quota(?="\))',
        r'r\.get\("daily_quota"\)',
        r'(?<=credential_ref,)daily_quota(?=,cache_seconds)',
        r'daily_quota=EXCLUDED\.daily_quota(?=,cache_seconds=EXCLUDED\.cache_seconds)',
        r'(?<=CASE WHEN )quota_day(?=<>\$2 THEN 1 ELSE requests_today\+1 END,)',
        r'(?<=END,)quota_day(?==\$2 WHERE id=\$1 AND NOT disabled AND \()',
        r'(?<=AND \()quota_day(?=<>\$2 OR requests_today<daily_quota\))',
        r'(?<=requests_today<)daily_quota(?=\) RETURNING requests_today")',
        r'(?<=AND NOT disabled AND )quota_day(?==\$2 AND requests_today<daily_quota RETURNING)',
        r'(?<=requests_today<)daily_quota(?= RETURNING requests_today")',
    ),
    # Exact official cloud initialization field used to prove its omission.
    "src/operations/hetzner/client/tests.rs": (r'raw\["user_data"\]',),

}

CORES = {
    "crates/agent-core": EXCEPTIONS,
    "crates/panel-host": HOST_EXCEPTIONS,
}

# Workspace crates each layer may depend on (all dependency tables).
AGENT_CORE_DEPENDENCIES = {"apsp-adapter-sdk", "apsp-protocol"}
ADAPTER_DEPENDENCIES = {"apsp-adapter-sdk"}
HOST_DEPENDENCIES = {"apsp-protocol"}
PLUGIN_DEPENDENCIES = {"apsp-panel-host", "apsp-protocol", "apsp-compiler", "apsp-cloud-api"}


def violations(relative, source, exceptions=None):
    exceptions = EXCEPTIONS if exceptions is None else exceptions
    findings = []
    masked = source
    for expression in exceptions.get(relative, ()):
        # Formatting can split an exact native expression across lines. Preserve
        # every line break while masking only that expression's characters.
        masked = re.sub(expression, lambda match: re.sub(r"[^\r\n]", " ", match.group()), masked)
    for number, (original, line) in enumerate(zip(source.splitlines(), masked.splitlines()), 1):
        if re.search(r"singbox|sing-box", original, re.IGNORECASE):
            findings.append((number, "runtime name"))
        for token in TOKENS.findall(line):
            parts = {part.lower() for part in CAMEL_PARTS.split(token)}
            banned = sorted(parts & FORBIDDEN)
            if banned:
                findings.append((number, "/".join(banned)))
    return findings


def check(directory, exceptions=None):
    failed = False
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            print(f"{path}: symlinks are not allowed in core", file=sys.stderr)
            failed = True
        elif path.is_file():
            relative = path.relative_to(directory).as_posix()
            try:
                source = path.read_text(encoding="utf-8")
            except UnicodeError:
                print(f"{path}: unreadable core source", file=sys.stderr)
                failed = True
                continue
            for line, reason in violations(relative, source, exceptions):
                print(f"{path}:{line}: forbidden core reference: {reason}", file=sys.stderr)
                failed = True
    return not failed


def workspace_dependencies(manifest):
    """Returns every `apsp-*` crate named in any dependency table."""
    data = tomllib.loads(manifest)
    tables = [data.get(key, {}) for key in ("dependencies", "dev-dependencies", "build-dependencies")]
    for target in data.get("target", {}).values():
        tables += [target.get(key, {}) for key in ("dependencies", "dev-dependencies", "build-dependencies")]
    return {name for table in tables for name in table if name.startswith("apsp-")}


def layer_rules(root):
    """Maps each layered manifest to the workspace crates it may depend on."""
    rules = {root / "crates/agent-core/Cargo.toml": AGENT_CORE_DEPENDENCIES}
    for manifest in sorted(root.glob("crates/adapter-*/Cargo.toml")):
        if manifest.parent.name != "adapter-sdk":
            rules[manifest] = ADAPTER_DEPENDENCIES
    rules[root / "crates/panel-host/Cargo.toml"] = HOST_DEPENDENCIES
    for manifest in sorted(root.glob("plugins/*/panel/Cargo.toml")):
        rules[manifest] = PLUGIN_DEPENDENCIES
    return rules


def check_dependencies(root):
    failed = False
    for manifest, allowed in layer_rules(root).items():
        if not manifest.is_file():
            print(f"{manifest}: layered manifest is missing", file=sys.stderr)
            failed = True
            continue
        for name in sorted(workspace_dependencies(manifest.read_text(encoding="utf-8")) - allowed):
            print(f"{manifest}: forbidden workspace dependency: {name}", file=sys.stderr)
            failed = True
    return not failed


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--core", type=Path, help="check only this core directory")
    args = parser.parse_args()
    if args.core is not None:
        if not args.core.is_dir():
            parser.error("core directory is missing")
        relative = args.core.resolve().relative_to(ROOT).as_posix() if args.core.resolve().is_relative_to(ROOT) else ""
        return 0 if check(args.core, CORES.get(relative, EXCEPTIONS)) else 1
    passed = all([check(ROOT / core, exceptions) for core, exceptions in CORES.items()])
    return 0 if check_dependencies(ROOT) and passed else 1


if __name__ == "__main__":
    sys.exit(main())
