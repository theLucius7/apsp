#!/usr/bin/env python3
"""Import original Nexus node information into the apsp node inventory."""
import argparse
from contextlib import closing
import json
import os
from pathlib import Path

import time

from migration_snapshot import connect, rows
from nexus_agent_migration import (FORMAT, HOST_FIELDS, MACHINE_FIELDS, NODE_FIELDS, Panel, digest, environment,
                                   private_write, readiness, server_request, snapshot, source_tag)

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


def servers_by_tag(panel):
    servers = panel.call("/api/servers")
    return servers.get("servers", servers) if isinstance(servers, dict) else servers


def tagged(servers, source_id, host_id):
    tag = source_tag({"source_id": source_id}, host_id)
    return [s for s in servers if tag in ((s.get("asset_settings") or {}).get("tags") or [])]


def append_snapshot(connection, source_id, node_ids, servers, now=None):
    """Only the requested nodes, their machines and hosts. Nodes imported
    earlier are not re-exported, so their stored records stay untouched."""
    wanted = set(node_ids)
    if not wanted or len(wanted) != len(node_ids):
        raise ValueError("List every node to append exactly once")
    nodes = sorted((row for row in rows(connection, "v2_server", NODE_FIELDS) if row["id"] in wanted),
                   key=lambda row: row["id"])
    if {row["id"] for row in nodes} != wanted:
        raise ValueError("Some requested nodes do not exist in the source")
    if any(row["machine_id"] is None or (row["parent_id"] or 0) > 0 for row in nodes):
        raise ValueError("Alias nodes have no listener of their own; append their machine-bound parent instead")
    links = {row["machine_id"]: row["host_id"] for row in rows(connection, "v2_infra_machine", ["machine_id", "host_id"])}
    machine_ids = {row["machine_id"] for row in nodes}
    if machine_ids - set(links):
        raise ValueError("Node machine has no physical host mapping in the source")
    machines = sorted(({**machine, "host_id": links[machine["id"]]}
                       for machine in rows(connection, "v2_server_machine", MACHINE_FIELDS) if machine["id"] in machine_ids),
                      key=lambda row: row["id"])
    host_ids = sorted({links[machine] for machine in machine_ids})
    hosts = sorted((host for host in rows(connection, "v2_infra_host", HOST_FIELDS) if host["id"] in host_ids),
                   key=lambda row: row["id"])
    mapping = {}
    for host_id in host_ids:
        found = tagged(servers, source_id, host_id)
        if len(found) != 1:
            raise ValueError(f"Source host {host_id} needs exactly one apsp server with its source tag")
        mapping[host_id] = found[0]["id"]
    # The digest covers only the appended slice, so later source edits elsewhere
    # do not block an append; the server re-checks every host by its tag.
    infrastructure = {"format": FORMAT, "source_id": source_id, "hosts": hosts, "machines": machines, "nodes": nodes}
    return {
        "format": "apsp.nexus.nodes.v1", "source_id": source_id,
        "infrastructure_digest": digest(infrastructure), "exported_at": now or int(time.time()),
        "hosts": [{"legacy_id": host_id, "server_id": mapping[host_id]} for host_id in host_ids],
        "machines": machines, "nodes": nodes,
        "groups": sorted(rows(connection, "v2_server_group", "id name created_at updated_at".split()), key=lambda row: row["id"]),
        "routes": sorted(rows(connection, "v2_server_route", "id remarks match action action_value created_at updated_at".split()), key=lambda row: row["id"]),
    }


def link_server(panel, source_id, host_id, server_id):
    """Tag an existing apsp server, such as one enrolled before its source host existed."""
    servers = servers_by_tag(panel)
    owners = tagged(servers, source_id, host_id)
    if owners and [s["id"] for s in owners] != [server_id]:
        raise ValueError(f"Source host {host_id} is already linked to another server")
    if owners:
        return False
    server = panel.call(f"/api/servers/{server_id}")
    settings = dict(server.get("asset_settings") or {})
    settings["tags"] = list(settings.get("tags") or []) + [source_tag({"source_id": source_id}, host_id)]
    panel.call(f"/api/servers/{server_id}", "PATCH", {"name": server["name"], "asset_settings": settings})
    return True


def create_servers(panel, connection, source_id, node_ids):
    """Create apsp servers for source hosts of the requested nodes that have none yet."""
    links = {row["machine_id"]: row["host_id"] for row in rows(connection, "v2_infra_machine", ["machine_id", "host_id"])}
    machines = {row["machine_id"] for row in rows(connection, "v2_server", NODE_FIELDS) if row["id"] in set(node_ids)}
    host_ids = {links[m] for m in machines if m in links}
    created = []
    for host in rows(connection, "v2_infra_host", HOST_FIELDS):
        if host["id"] in host_ids and not tagged(servers_by_tag(panel), source_id, host["id"]):
            server = panel.call("/api/servers", "POST", server_request({"source_id": source_id}, host))
            created.append({"legacy_host_id": host["id"], "apsp_server_id": server["id"]})
    return created


def append_nodes(panel, value, confirmed_digest, receipt_path):
    preview = panel.call(f"{ROOT}/append/preview", "POST", {"payload": value})
    if preview["digest"] != confirmed_digest:
        raise ValueError("Confirm the exact append preview digest")
    if preview["conflicts"]:
        raise ValueError("Append preview has conflicts; review host tags and earlier imports")
    panel.call("/api/control-center/reauth", "POST", {"password": panel.password})
    result = panel.call(f"{ROOT}/append", "POST", {
        "payload": value, "expected_digest": confirmed_digest, "confirm_staged": True,
    })
    imported = {row["legacy_node_id"]: row for row in panel.call(ROOT) if row["source_id"] == value["source_id"]}
    hosts = {host["legacy_id"]: host["server_id"] for host in value["hosts"]}
    machines = {machine["id"]: machine["host_id"] for machine in value["machines"]}
    for node in value["nodes"]:
        row = imported.get(node["id"])
        if (row is None or row["legacy_machine_id"] != node["machine_id"]
                or row["server_id"] != hosts[machines[node["machine_id"]]]):
            raise ValueError("Appended node identity or host mapping differs; keep the snapshot and reconcile")
    receipt = {"source_id": value["source_id"], "digest": confirmed_digest, "complete": True,
               "appended": [node["id"] for node in value["nodes"]], "already_imported": result["already_imported"],
               "runtime_ownership_changed": False,
               "nodes": [{"legacy_node_id": n["id"], "apsp_inventory_id": imported[n["id"]]["id"],
                          "apsp_server_id": imported[n["id"]]["server_id"]} for n in value["nodes"]]}
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
    append = commands.add_parser("append", help="Add nodes the source gained after its import")
    append.add_argument("database")
    append.add_argument("snapshot", help="Written on first run and reused for preview and retries")
    append.add_argument("output")
    append.add_argument("--source-id", required=True)
    append.add_argument("--node", type=int, action="append", required=True)
    append.add_argument("--link-server", action="append", default=[], metavar="HOST=SERVER",
                        help="Tag an existing apsp server as this source host")
    append.add_argument("--create-server", action="store_true", help="Create apsp servers for untagged hosts")
    append.add_argument("--confirmed-digest")
    append.add_argument("--env-file", required=True)
    append.add_argument("--panel-api", required=True)
    args = parser.parse_args()
    if args.command == "append":
        settings = environment(args.env_file)
        panel = Panel(args.panel_api, settings["APSP_PUBLIC_URL"], settings["APSP_ADMIN_PASSWORD"])
        panel.login()
        if not Path(args.snapshot).exists():
            with closing(connect(args.database)) as connection:
                if args.link_server or args.create_server:
                    panel.call("/api/control-center/reauth", "POST", {"password": panel.password})
                for pair in args.link_server:
                    host, server = (int(part) for part in pair.split("=", 1))
                    print(json.dumps({"linked": link_server(panel, args.source_id, host, server), "host": host, "server": server}))
                if args.create_server:
                    print(json.dumps({"created": create_servers(panel, connection, args.source_id, args.node)}))
                private_write(args.snapshot, append_snapshot(connection, args.source_id, args.node, servers_by_tag(panel)))
        value = json.loads(Path(args.snapshot).read_text())
        if sorted(node["id"] for node in value["nodes"]) != sorted(args.node):
            raise ValueError("The saved append snapshot holds other nodes; choose a new snapshot path")
        if args.confirmed_digest is None:
            report = panel.call(f"{ROOT}/append/preview", "POST", {"payload": value})
            private_write(args.output, report)
            print(json.dumps(report, ensure_ascii=False))
        else:
            receipt = append_nodes(panel, value, args.confirmed_digest, args.output)
            print(json.dumps({"appended": receipt["appended"], "already_imported": receipt["already_imported"]}))
        return
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
