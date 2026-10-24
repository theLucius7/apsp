#!/usr/bin/env python3
"""Stage Nexus host assets without changing node ownership or credentials."""
import argparse
from contextlib import closing
from datetime import date, datetime, timezone
from decimal import Decimal, InvalidOperation
import fcntl
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import re
import shlex
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

from migration_snapshot import connect, rows

FORMAT = "apsp.nexus.infrastructure.v1"
HOST_FIELDS = "id name address notes metadata".split()
MACHINE_FIELDS = "id name is_active".split()
NODE_FIELDS = "id type code parent_id group_ids route_ids name rate tags host port server_port protocol_settings show sort rate_time_enable rate_time_ranges custom_outbounds custom_routes cert_config transfer_enable machine_id enabled".split()


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()).hexdigest()


def private_write(path, value):
    """Atomically replace only an explicitly selected private output file."""
    path = Path(path)
    if path.is_symlink():
        raise ValueError("Refusing a symbolic-link output")
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor, temporary = tempfile.mkstemp(prefix=".nexus-stage-", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w") as output:
            json.dump(value, output, ensure_ascii=False, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def object_value(value):
    if isinstance(value, str):
        value = json.loads(value) if value else {}
    if value is None:
        return {}
    if not isinstance(value, dict):
        raise ValueError("Expected a source JSON object")
    return value


def array_value(value):
    if isinstance(value, str):
        value = json.loads(value) if value else []
    if value is None:
        return []
    if not isinstance(value, list):
        raise ValueError("Expected a source JSON array")
    return value


def snapshot(connection, source_id):
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", source_id):
        raise ValueError("source-id must be 1..64 ASCII letters, digits, '-' or '_'")
    value = {
        "format": FORMAT, "source_id": source_id,
        "hosts": rows(connection, "v2_infra_host", HOST_FIELDS),
        "machines": rows(connection, "v2_server_machine", MACHINE_FIELDS),
        "machine_hosts": rows(connection, "v2_infra_machine", ["machine_id", "host_id"]),
        "nodes": rows(connection, "v2_server", NODE_FIELDS),
        # Passwords, SSH secrets, machine tokens and live telemetry are excluded.
        "connections": rows(connection, "v2_infra_connection", "id host_id port identity_id jump_ids host_keys".split()),
    }
    for field in ["hosts", "machines", "nodes", "connections"]:
        value[field].sort(key=lambda row: row["id"])
    value["machine_hosts"].sort(key=lambda row: row["machine_id"])
    validate(value)
    return {"snapshot": value, "digest": digest(value), "exported_at": int(time.time())}


def validate(value):
    if value.get("format") != FORMAT:
        raise ValueError("Unsupported infrastructure snapshot")
    if not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", value.get("source_id", "")):
        raise ValueError("Invalid source identity")
    for field in ["hosts", "machines", "nodes", "connections"]:
        items = value.get(field)
        if not isinstance(items, list) or len(items) > 20000:
            raise ValueError("Invalid inventory size")
        ids = [item.get("id") for item in items]
        if any(type(identifier) is not int or identifier <= 0 for identifier in ids) or len(set(ids)) != len(ids):
            raise ValueError("Duplicate or invalid inventory identity")
    hosts = {row["id"] for row in value["hosts"]}
    machines = {row["id"] for row in value["machines"]}
    links = value["machine_hosts"]
    if len({row["machine_id"] for row in links}) != len(links):
        raise ValueError("One machine is assigned to multiple hosts")
    if any(row["host_id"] not in hosts or row["machine_id"] not in machines for row in links):
        raise ValueError("Unresolved machine-to-host mapping")
    if any(row["host_id"] not in hosts for row in value["connections"]):
        raise ValueError("Unresolved connection host")
    if any(node["machine_id"] not in machines for node in value["nodes"]):
        raise ValueError("Unresolved node machine")
    if any(node["machine_id"] not in {row["machine_id"] for row in links} for node in value["nodes"]):
        raise ValueError("Node machine has no physical host mapping")
    for host in value["hosts"]:
        server_request(value, host)


def source_tag(value, identifier):
    namespace = hashlib.sha256(value["source_id"].encode()).hexdigest()[:12]
    return f"nexus:{namespace}:{identifier}"


def server_request(value, host):
    metadata = object_value(host["metadata"])
    asset = object_value(metadata.get("asset"))
    name = host["name"]
    if not isinstance(name, str) or not name.strip() or len(name) > 128 or any(ord(c) < 32 for c in name):
        raise ValueError("Invalid host display name")
    region = metadata.get("region") or ""
    region = region if isinstance(region, str) and len(region) <= 16 else ""
    settings = {"region": region, "group_name": "Nexus 迁移", "tags": [source_tag(value, host["id"])],
                "offline_notify": False, "auto_renewal": False}
    if asset.get("operational_status") == "stopped":
        settings["tags"].append("原系统已停用")
    price, currency = asset.get("renewal_price"), asset.get("currency")
    if price is not None:
        if currency is None:
            raise ValueError("A host price without its source currency cannot be migrated")
        amount = Decimal(str(price))
        if not amount.is_finite() or amount < 0 or amount > 1_000_000_000 or amount != amount.quantize(Decimal("0.01")):
            raise ValueError("Invalid source renewal price")
        settings["price"] = format(amount, ".2f")
    if currency:
        if not re.fullmatch("[A-Z]{3}", currency):
            raise ValueError("Invalid source currency")
        settings["currency"] = currency
    if asset.get("billing_cycle_days") is not None:
        settings["billing_cycle"] = int(asset["billing_cycle_days"])
        if not 1 <= settings["billing_cycle"] <= 3650:
            raise ValueError("Invalid source billing cycle")
    if asset.get("expires_on"):
        expires = date.fromisoformat(asset["expires_on"])
        # Source stores a date without a timezone. Preserve it as a UTC date;
        # never infer automatic renewal or perform a payment.
        settings["expires_at"] = int(datetime.combine(expires, datetime.min.time(), timezone.utc).timestamp())
    # GB's decimal/binary meaning and source reset day are not declared. Keep
    # them in the private snapshot; do not invent an active quota in apsp.
    return {"name": name, "asset_settings": settings, "agent_settings": {"auto_update": False}}


def readiness(envelope, release=None):
    value = envelope["snapshot"]
    validate(value)
    if digest(value) != envelope.get("digest"):
        raise ValueError("Snapshot digest mismatch")
    mappings = {row["machine_id"]: row["host_id"] for row in value["machine_hosts"]}
    nodes = []
    for node in value["nodes"]:
        protocol = object_value(node["protocol_settings"])
        reasons = ["credential_preserving_node_import_required", "metering_handoff_required"]
        if node["type"] == "vless":
            reality = object_value(protocol.get("reality_settings"))
            if len(reality.get("short_id") or "") != 8:
                reasons.append("existing_reality_short_id_not_accepted")
            if object_value(protocol.get("multiplex")).get("enabled"):
                reasons.append("vless_multiplex_mapping_required")
            encryption = object_value(protocol.get("encryption"))
            if encryption.get("enabled"):
                reasons.append("vless_encryption_effective_config_review_required")
            if protocol.get("tls") != 2 or protocol.get("network") != "tcp":
                reasons.append("vless_transport_mapping_required")
        elif node["type"] in ("hysteria", "shadowsocks"):
            reasons.append("signed_agent_with_modern_inbounds_required")
            reasons.append("existing_protocol_credentials_mapping_required")
            if node["type"] == "hysteria":
                reasons.append("certificate_and_renewal_handoff_required")
        else:
            reasons.append("unsupported_source_protocol")
        if Decimal(str(node["rate"])) != 1 or node["rate_time_enable"]:
            reasons.append("traffic_multiplier_mapping_required")
        if array_value(node["custom_routes"]) or array_value(node["custom_outbounds"]):
            reasons.append("custom_routing_and_external_egress_mapping_required")
        if str(node["port"]) != str(node["server_port"]):
            reasons.append("public_port_or_port_hopping_mapping_required")
        nodes.append({"legacy_node_id": node["id"], "legacy_host_id": mappings[node["machine_id"]],
                      "protocol": node["type"], "ready_for_cutover": False, "reasons": reasons})
    connections = {host["id"]: [row for row in value["connections"] if row["host_id"] == host["id"]]
                   for host in value["hosts"]}
    hosts = [{"legacy_host_id": host["id"],
              "machine_ids": [row["machine_id"] for row in value["machine_hosts"] if row["host_id"] == host["id"]],
              "node_ids": [node["legacy_node_id"] for node in nodes if node["legacy_host_id"] == host["id"]],
              "ssh_configured": bool(connections[host["id"]]),
              "ssh_keys_pinned": bool(connections[host["id"]]) and all(array_value(row["host_keys"]) for row in connections[host["id"]])}
             for host in value["hosts"]]
    release_check = {"status": "not_checked", "signature_verified": False}
    if release is not None:
        mismatches = []
        if release.get("source_repo") != "theLucius7/apsp":
            mismatches.append("signed_repository_identity_differs")
        if any(row.get("binary_name") not in ("apsp-agent", "apsp-agent.exe") for row in release.get("artifacts", []) if row.get("name") == "agent"):
            mismatches.append("signed_agent_name_differs")
        release_check.update(status="incompatible_identity" if mismatches else "identity_only_not_signature_or_capability_acceptance",
                             tag=release.get("tag"), reasons=mismatches)
    return {"digest": envelope["digest"], "hosts": hosts, "nodes": nodes,
            "host_count": len(hosts), "machine_count": len(value["machines"]), "node_count": len(nodes),
            "ready_for_cutover": False, "release": release_check,
            "staging_scope": "host_assets_only_no_agent_or_node_ownership_change"}


class Panel:
    def __init__(self, base, public, password):
        url = urllib.parse.urlsplit(base)
        if url.scheme != "https":
            try:
                local = ipaddress.ip_address(url.hostname or "").is_loopback
            except ValueError:
                local = False
            if url.scheme != "http" or not local:
                raise ValueError("Panel API requires HTTPS or an explicit loopback IP")
        if url.username or url.password or url.query or url.fragment or url.path not in ("", "/"):
            raise ValueError("Panel API must be a credential-free origin")
        self.base, self.public, self.password = base.rstrip("/"), public, password
        self.cookie = ""
        class NoRedirect(urllib.request.HTTPRedirectHandler):
            def redirect_request(self, req, fp, code, msg, headers, newurl):
                return None
        self.opener = urllib.request.build_opener(NoRedirect())

    def call(self, path, method="GET", payload=None):
        headers = {"Origin": self.public, "User-Agent": "apsp-nexus-infrastructure-migration/1"}
        if self.cookie:
            headers["Cookie"] = self.cookie
        body = None
        if payload is not None:
            headers["Content-Type"] = "application/json"
            body = json.dumps(payload).encode()
        request = urllib.request.Request(self.base + path, method=method, data=body, headers=headers)
        try:
            with self.opener.open(request, timeout=45) as response:
                if response.headers.get("Set-Cookie"):
                    self.cookie = response.headers["Set-Cookie"].split(";", 1)[0]
                return json.load(response)
        except urllib.error.HTTPError as error:
            raise ValueError(f"Panel {method} request failed: HTTP {error.code}") from None
        except urllib.error.URLError:
            raise ValueError("Panel request failed; rerun to reconcile the saved source tags") from None

    def login(self):
        self.call("/api/login", "POST", {"password": self.password})


def stage(panel, envelope, confirmed_digest, receipt_path):
    report = readiness(envelope)
    if report["digest"] != confirmed_digest:
        raise ValueError("Confirm the exact reviewed snapshot digest")
    value = envelope["snapshot"]
    receipt_path = Path(receipt_path)
    receipt_path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(str(receipt_path) + ".lock", os.O_WRONLY | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if receipt_path.is_symlink():
            raise ValueError("Refusing a symbolic-link receipt")
        previous = json.loads(receipt_path.read_text()) if receipt_path.exists() else None
        if previous is not None and previous.get("digest") != report["digest"]:
            raise ValueError("Receipt belongs to a different snapshot; review source changes first")
        existing = panel.call("/api/servers")
        matches = {}
        for host in value["hosts"]:
            tag = source_tag(value, host["id"])
            found = [row for row in existing if tag in row.get("asset_settings", {}).get("tags", [])]
            if len(found) > 1:
                raise ValueError("Duplicate source tags in the panel; resolve before staging")
            if not found and any(row.get("name") == host["name"] for row in existing):
                raise ValueError("An existing unlinked server has the same name; review its identity first")
            matches[host["id"]] = found[0] if found else None
        # Validate every mapping before the first mutation.
        for entry in (previous or {}).get("servers", []):
            current = matches.get(entry["legacy_host_id"])
            if current is None or current["id"] != entry["apsp_server_id"]:
                raise ValueError("A previously staged server is missing or has changed identity")
        receipt = {"digest": report["digest"], "source_id": value["source_id"], "servers": [],
                   "complete": False, "agents_installed": False, "nodes_transferred": False,
                   "original_subscriptions_unchanged": True}
        private_write(receipt_path, receipt)
        for host in value["hosts"]:
            server = matches[host["id"]]
            reused = server is not None
            if server is None:
                server = panel.call("/api/servers", "POST", server_request(value, host))
            receipt["servers"].append({"legacy_host_id": host["id"], "apsp_server_id": server["id"], "reused": reused})
            private_write(receipt_path, receipt)
        receipt["complete"] = True
        private_write(receipt_path, receipt)
        return receipt


def environment(path):
    values = {}
    for line in Path(path).read_text().splitlines():
        if "=" in line and not line.lstrip().startswith("#"):
            key, value = line.split("=", 1)
            parsed = shlex.split(value)
            values[key.strip()] = parsed[0] if parsed else ""
    return values


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    export = commands.add_parser("export")
    export.add_argument("database")
    export.add_argument("output")
    export.add_argument("--source-id", required=True)
    preview = commands.add_parser("preview")
    preview.add_argument("snapshot")
    preview.add_argument("--release-metadata")
    preview.add_argument("--output")
    apply = commands.add_parser("stage-hosts")
    apply.add_argument("snapshot")
    apply.add_argument("receipt")
    apply.add_argument("--confirmed-digest", required=True)
    apply.add_argument("--env-file", required=True)
    apply.add_argument("--panel-api", required=True)
    args = parser.parse_args()
    if args.command == "export":
        with closing(connect(args.database)) as connection:
            envelope = snapshot(connection, args.source_id)
        private_write(args.output, envelope)
        print(json.dumps({"digest": envelope["digest"], "host_count": len(envelope["snapshot"]["hosts"]), "node_count": len(envelope["snapshot"]["nodes"])}))
    else:
        envelope = json.loads(Path(args.snapshot).read_text())
        if args.command == "preview":
            release = json.loads(Path(args.release_metadata).read_text()) if args.release_metadata else None
            report = readiness(envelope, release)
            if args.output:
                private_write(args.output, report)
            print(json.dumps(report, ensure_ascii=False))
        else:
            settings = environment(args.env_file)
            panel = Panel(args.panel_api, settings["APSP_PUBLIC_URL"], settings["APSP_ADMIN_PASSWORD"])
            panel.login()
            receipt = stage(panel, envelope, args.confirmed_digest, args.receipt)
            print(json.dumps({"hosts_staged": len(receipt["servers"]), "agents_installed": False, "nodes_transferred": False}))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, InvalidOperation, KeyError) as error:
        # No source records, paths, addresses, credentials or HTTP bodies in stdout.
        raise SystemExit(str(error)) from None
