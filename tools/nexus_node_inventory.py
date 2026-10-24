#!/usr/bin/env python3
"""Import original Nexus node information into the apsp node inventory."""
import argparse
from contextlib import closing
import json
import os
from pathlib import Path

from migration_snapshot import connect, rows
from nexus_agent_migration import Panel, environment, private_write, readiness, snapshot

ROOT = "/api/plugins/sing-box/nexus-nodes"


def node_snapshot(connection, infrastructure, host_receipt):
    report = readiness(infrastructure)
    source = infrastructure["snapshot"]
    if (not host_receipt.get("complete") or host_receipt.get("digest") != report["digest"]
            or host_receipt.get("source_id") != source["source_id"]):
        raise ValueError("A complete host receipt for the same infrastructure snapshot is required")
    current = snapshot(connection, source["source_id"])
    if current["digest"] != report["digest"]:
        raise ValueError("Source infrastructure changed; reconcile the host snapshot before importing nodes")
    hosts = host_receipt.get("servers", [])
    host_ids = [host.get("legacy_host_id") for host in hosts]
    server_ids = [host.get("apsp_server_id") for host in hosts]
    if (len(host_ids) != len(set(host_ids)) or set(host_ids) != {host["id"] for host in source["hosts"]}
            or len(server_ids) != len(set(server_ids)) or any(type(id) is not int or id <= 0 for id in server_ids)):
        raise ValueError("Host receipt contains duplicate, missing or invalid mappings")
    machine_hosts = {row["machine_id"]: row["host_id"] for row in source["machine_hosts"]}
    # Preserve the raw node JSON strings and all original key material. The
    # server derives its display projection and encrypts these original records.
    return {
        "format": "apsp.nexus.nodes.v1", "source_id": source["source_id"],
        "infrastructure_digest": current["digest"], "exported_at": current["exported_at"],
        "hosts": [{"legacy_id": h["legacy_host_id"], "server_id": h["apsp_server_id"]} for h in hosts],
        "machines": [{**m, "host_id": machine_hosts[m["id"]]} for m in source["machines"] if m["id"] in machine_hosts],
        "nodes": source["nodes"],
        "groups": sorted(rows(connection, "v2_server_group", "id name created_at updated_at".split()), key=lambda row: row["id"]),
        "routes": sorted(rows(connection, "v2_server_route", "id remarks match action action_value created_at updated_at".split()), key=lambda row: row["id"]),
    }


def import_nodes(panel, value, confirmed_digest, receipt_path):
    preview = panel.call(f"{ROOT}/preview", "POST", {"payload": value})
    if preview["digest"] != confirmed_digest:
        raise ValueError("Confirm the exact node preview digest")
    if preview["conflicts"]:
        raise ValueError("Node preview has conflicts; review host mapping and previous imports")
    panel.call("/api/control-center/reauth", "POST", {"password": panel.password})
    result = panel.call(f"{ROOT}/import", "POST", {
        "payload": value, "expected_digest": confirmed_digest, "confirm_staged": True,
    })
    imported = [row for row in panel.call(ROOT) if row["source_id"] == value["source_id"]]
    expected = {node["id"] for node in value["nodes"]}
    if {node["legacy_node_id"] for node in imported} != expected or len(imported) != len(expected):
        raise ValueError("Imported node identities differ; keep the snapshot and reconcile before retrying")
    hosts = {host["legacy_id"]: host["server_id"] for host in value["hosts"]}
    machines = {machine["id"]: machine["host_id"] for machine in value["machines"]}
    originals = {node["id"]: node for node in value["nodes"]}
    for node in imported:
        machine = originals[node["legacy_node_id"]]["machine_id"]
        if (node["legacy_machine_id"] != machine or node["legacy_host_id"] != machines[machine]
                or node["server_id"] != hosts[machines[machine]] or node["state"] != "pending_agent"):
            raise ValueError("Imported node host mapping or state differs")
    receipt = {"source_id": value["source_id"], "digest": confirmed_digest, "complete": True,
               "node_information_imported": len(imported), "agents_installed": False,
               "runtime_ownership_changed": False, "already_imported": result["already_imported"],
               "nodes": [{"legacy_node_id": n["legacy_node_id"], "apsp_inventory_id": n["id"],
                          "apsp_server_id": n["server_id"]} for n in imported]}
    private_write(receipt_path, receipt)
    return receipt


def main():
    os.umask(0o077)
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    export = commands.add_parser("export")
    export.add_argument("database")
    export.add_argument("infrastructure")
    export.add_argument("host_receipt")
    export.add_argument("output")
    for command in ["preview", "import"]:
        action = commands.add_parser(command)
        action.add_argument("snapshot")
        action.add_argument("output")
        action.add_argument("--env-file", required=True)
        action.add_argument("--panel-api", required=True)
        if command == "import":
            action.add_argument("--confirmed-digest", required=True)
    args = parser.parse_args()
    if args.command == "export":
        if Path(args.output).exists():
            raise ValueError("Keep the existing node snapshot for retries; choose a new output for another export")
        with closing(connect(args.database)) as connection:
            value = node_snapshot(connection, json.loads(Path(args.infrastructure).read_text()),
                                  json.loads(Path(args.host_receipt).read_text()))
        private_write(args.output, value)
        print(json.dumps({"nodes_exported": len(value["nodes"]), "hosts": len(value["hosts"])}))
        return
    value = json.loads(Path(args.snapshot).read_text())
    settings = environment(args.env_file)
    panel = Panel(args.panel_api, settings["APSP_PUBLIC_URL"], settings["APSP_ADMIN_PASSWORD"])
    panel.login()
    if args.command == "preview":
        report = panel.call(f"{ROOT}/preview", "POST", {"payload": value})
        private_write(args.output, report)
        print(json.dumps(report, ensure_ascii=False))
    else:
        receipt = import_nodes(panel, value, args.confirmed_digest, args.output)
        print(json.dumps({"node_information_imported": receipt["node_information_imported"],
                          "runtime_ownership_changed": False, "already_imported": receipt["already_imported"]}))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError) as error:
        raise SystemExit(str(error)) from None
