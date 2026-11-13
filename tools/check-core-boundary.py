#!/usr/bin/env python3
"""Reject proxy business references in core while preserving native account APIs.

The Agent core (`crates/agent-core`) and the platform crates (`crates/panel-host`,
`crates/cloud-api`) are checked. Crate manifests are also checked so that every
workspace crate has a layer and depends only in the documented direction: the
platform never depends on a domain (its base `panel-host` depends only on the
protocol, and `cloud-api` builds on `panel-host`), and a domain crate depends only
on the platform and on domain crates of earlier groups (ADR 0128, ADR 0133,
docs/architecture.md).
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
    "src/state/schema.rs": (r'"user_version"',),
    "tests/state_baseline.rs": (
        r"\bPRAGMA user_version\b",
        r'\.pragma_query_value\(None, "user_version",',
    ),
    "tests/usage.rs": (r"\bPRAGMA user_version\b",),
    "tests/usage_bounds.rs": (
        r"\bPRAGMA user_version\b",
        r'\.pragma_update\(None, "user_version",',
    ),
}

# Panel platform: a URL userinfo test fixture, and the HTTP client method that sets the
# product token an outbound client identifies itself with.
HOST_EXCEPTIONS = {
    "src/config.rs": (r'"https://user:secret@example\.invalid"',),
    "src/http.rs": (r'(?<=builder\.)user_agent(?=\(product\))',),
}

# Cloud API client: the HTTP header name and the constant that fills it.
CLOUD_API_EXCEPTIONS = {
    "src/globalping.rs": (
        r'(?<=const )USER_AGENT(?=: &str = "apsp-panel";)',
        r'\.header\("user-agent", USER_AGENT\)',
    ),
}

CORES = {
    "crates/agent-core": EXCEPTIONS,
    "crates/panel-host": HOST_EXCEPTIONS,
    "crates/cloud-api": CLOUD_API_EXCEPTIONS,
}

# Workspace crates each layer may depend on (all dependency tables).
AGENT_CORE_DEPENDENCIES = {"apsp-adapter-sdk", "apsp-protocol"}
ADAPTER_DEPENDENCIES = {"apsp-adapter-sdk"}
# Crates without workspace dependencies.
LEAVES = ("crates/protocol", "crates/adapter-sdk", "crates/tcp-probe")
# The platform knows no domain. The panel host is its base; the cloud provider client
# builds on the panel host's mechanisms (outbound HTTP), never the other way round.
PLATFORM_CRATES = {"crates/panel-host": "apsp-panel-host", "crates/cloud-api": "apsp-cloud-api"}
PLATFORM_DEPENDENCIES = {
    "crates/panel-host": {"apsp-protocol"},
    "crates/cloud-api": {"apsp-protocol", "apsp-panel-host"},
}
# Domain groups in dependency order (docs/architecture.md section 5.1) and the domain crates.
DOMAIN_ORDER = (("governance",), ("resources",), ("networking",), ("naming",), ("traffic",),
                ("access",), ("costs", "observability"), ("operations",))
DOMAIN_CRATES = {
    "crates/governance": ("apsp-governance", "governance"),
    "crates/resources": ("apsp-resources", "resources"),
    "crates/networking": ("apsp-networking", "networking"),
    "crates/naming": ("apsp-naming", "naming"),
    "crates/compiler": ("apsp-compiler", "traffic"),
    "crates/costs": ("apsp-costs", "costs"),
    "crates/operations": ("apsp-operations", "operations"),
    "crates/observability": ("apsp-observability", "observability"),
    "crates/traffic": ("apsp-traffic", "traffic"),
    "crates/access": ("apsp-access", "access"),
}
# The application and the Agent binary compose the layers.
COMPOSERS = ("crates/panel", "crates/agent")


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


def domain_dependencies(domain, own=None):
    """The platform, the protocol, the domain crates of earlier groups and the other crates
    of the same domain (traffic's service builds on its configuration compiler)."""
    rank = {name: index for index, group in enumerate(DOMAIN_ORDER) for name in group}
    earlier = {crate for crate, other in DOMAIN_CRATES.values()
               if rank[other] < rank[domain] or (other == domain and own not in (None, crate))}
    return {"apsp-protocol", *PLATFORM_CRATES.values(), *earlier}


def layer_rules(root):
    """Maps each layered manifest to the workspace crates it may depend on."""
    rules = {root / "crates/agent-core/Cargo.toml": AGENT_CORE_DEPENDENCIES}
    for manifest in sorted(root.glob("crates/adapter-*/Cargo.toml")):
        if manifest.parent.name != "adapter-sdk":
            rules[manifest] = ADAPTER_DEPENDENCIES
    for crate in LEAVES:
        rules[root / crate / "Cargo.toml"] = set()
    for crate in PLATFORM_CRATES:
        rules[root / crate / "Cargo.toml"] = PLATFORM_DEPENDENCIES[crate]
    for crate, (name, domain) in DOMAIN_CRATES.items():
        rules[root / crate / "Cargo.toml"] = domain_dependencies(domain, name)
    return rules


def check_dependencies(root):
    failed = False
    rules = layer_rules(root)
    composers = {root / crate / "Cargo.toml" for crate in COMPOSERS}
    for manifest in sorted(root.glob("crates/*/Cargo.toml")):
        if manifest not in rules and manifest not in composers:
            print(f"{manifest}: workspace crate has no layer in tools/check-core-boundary.py", file=sys.stderr)
            failed = True
    for manifest, allowed in rules.items():
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
