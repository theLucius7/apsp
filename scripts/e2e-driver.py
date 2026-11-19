#!/usr/bin/env python3
"""Drive real acceptance through the panel; the operator controls devices and traffic."""

import argparse
import contextlib
import fcntl
import getpass
import http.cookiejar
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid


SELF_CONTAINED_CAPABILITY = "agent:self-contained:v1"
SERVICE_RUNTIME_STATES = (
    "unconfigured",
    "waiting_agent",
    "upgrade_required",
    "offline",
    "pending",
    "ready",
    "sync_blocked",
    "failed",
)
DRIFT_STATES = (
    "unknown",
    "stale",
    "pending_publish",
    "revision_mismatch",
    "configuration_mismatch",
    "configuration_verified",
    "inspection_pending",
    "unhealthy",
    "revision_aligned",
)
CHECKPOINT_VOCABULARY = {
    "state": ("unknown", "stale", "verified", "mismatch", "expired", "pending"),
    "outcome": ("verified", "mismatch", "late", "superseded", "failed"),
    "failure_kind": ("configuration_mismatch", "runtime_identity_mismatch", "unhealthy", "inspection_failed"),
}
ERROR_CATEGORIES = (
    ("health", re.compile(r"health|unhealthy")),
    ("activation", re.compile(r"activation|certif")),
    ("runtime_identity", re.compile(r"instance|binary|executable|signed runtime|bundled")),
    ("configuration", re.compile(r"configuration|(?<![a-z])bundles?(?![a-z])|revision")),
    ("timeout", re.compile(r"timed out|timeout")),
)


class AcceptanceError(Exception):
    pass


def ensure(condition, message):
    if not condition:
        raise AcceptanceError(message)


def private_write(path, content):
    ensure(not path.is_symlink(), "私有输出不能是符号链接")
    descriptor, temporary = tempfile.mkstemp(dir=path.parent, prefix=".e2e-")
    try:
        with os.fdopen(descriptor, "w") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def save(path, state):
    private_write(path, json.dumps(state, ensure_ascii=False, indent=2) + "\n")


def origin(value):
    parsed = urllib.parse.urlsplit(value)
    ensure(
        parsed.scheme in ("http", "https") and parsed.hostname
        and not parsed.username and not parsed.password and not parsed.query
        and not parsed.fragment and parsed.path in ("", "/"),
        "面板地址必须是 HTTP(S) origin",
    )
    return value.rstrip("/")


def origin_header(base):
    # The panel compares Origin with the canonical serialization of its public
    # URL: lowercase scheme and host, default port omitted.
    parsed = urllib.parse.urlsplit(base)
    host = parsed.hostname if ":" not in parsed.hostname else f"[{parsed.hostname}]"
    default = {"http": 80, "https": 443}[parsed.scheme.lower()]
    port = "" if parsed.port in (None, default) else f":{parsed.port}"
    return f"{parsed.scheme.lower()}://{host}{port}"


def failure_detail(path, error):
    route = urllib.parse.urlsplit(path).path
    if not route.startswith("/api/") or "enrollment" in route or "token" in route:
        return ""
    try:
        message = json.loads(error.read(4096)).get("error")
    except Exception:
        message = None
    detail = " " + re.sub(r"/[0-9]+(?=/|$)", "/{id}", route)
    if isinstance(message, str):
        detail += "：" + message[:160]
    return detail


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, new_url):
        raise AcceptanceError("验收接口出现重定向；请直接使用最终面板 origin")


class Panel:
    def __init__(self, base, password, totp_code=None):
        self.base = base
        self.client = urllib.request.build_opener(
            NoRedirect(), urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())
        )
        self.request("/api/login", {"password": password, "totp_code": totp_code})

    def request(self, path, data=None, raw=False, method=None):
        body = None if data is None else json.dumps(data).encode()
        headers = {} if body is None else {"Content-Type": "application/json", "Origin": origin_header(self.base)}
        request = urllib.request.Request(self.base + path, data=body, headers=headers, method=method)
        try:
            with self.client.open(request, timeout=20) as response:
                content = response.read(2 * 1024 * 1024 + 1)
        except urllib.error.HTTPError as error:
            # Do not expose enrollment or subscription tokens from request URLs:
            # report only token-free API paths and the panel's fixed message.
            raise AcceptanceError(f"面板请求失败，HTTP {error.code}" + failure_detail(path, error)) from None
        except urllib.error.URLError:
            raise AcceptanceError("无法连接面板，请检查 origin、TLS 与网络") from None
        ensure(len(content) <= 2 * 1024 * 1024, "面板响应超过验收脚本上限")
        return content.decode() if raw else json.loads(content)

    def close(self):
        with contextlib.suppress(Exception):
            self.request("/api/logout", {})


def password():
    filename = os.environ.get("APSP_E2E_ADMIN_PASSWORD_FILE")
    if filename:
        path = Path(filename)
        metadata = path.lstat()
        ensure(path.is_file() and not path.is_symlink(), "密码文件必须是普通文件")
        ensure(metadata.st_mode & 0o077 == 0, "密码文件需仅当前用户可读写")
        return path.read_text().rstrip("\r\n")
    value = os.environ.get("APSP_E2E_ADMIN_PASSWORD")
    return value if value is not None else getpass.getpass("管理员密码（不保存）：")


def totp_code(prompt=False):
    value = os.environ.pop("APSP_E2E_TOTP_CODE", None)
    if value is None and (prompt or os.environ.get("APSP_E2E_TOTP") == "1"):
        value = getpass.getpass("当前 TOTP 验证码（不保存，请用未使用的新码）：")
    ensure(value is None or re.fullmatch(r"[0-9]{6}", value), "TOTP 验证码需为六位数字")
    return value


def resource(panel, path, state, kind, fields, state_path):
    name = state["prefix"] + "-" + kind
    identifier = state.get(kind + "_id")
    if identifier is None:
        # The unique name is persisted before POST, so a lost response is recoverable.
        matches = [item for item in panel.request(path) if item["name"] == name]
        ensure(len(matches) <= 1, "专用验收名称出现重复；保留现状并人工检查")
        item = matches[0] if matches else panel.request(path, {"name": name, **fields})
        state[kind + "_id"] = item["id"]
        save(state_path, state)
    else:
        item = panel.request(path + "/" + str(identifier))
    ensure(item["name"] == name, "验收对象已改名；拒绝操作其他对象")
    for key, expected in fields.items():
        ensure(item[key] == expected, "验收对象配置与私有 state 不一致")
    return item


def policy_group(panel, state, node_id, state_path):
    name = state["prefix"] + "-group"
    matches = [item for item in panel.request("/api/access/policy-groups") if item["name"] == name]
    ensure(len(matches) <= 1, "专用验收名称出现重复；保留现状并人工检查")
    if matches:
        group = matches[0]
    else:
        # The unique name is persisted before POST, so a lost response is recoverable.
        save(state_path, state)
        group = panel.request("/api/access/policy-groups",
                              {"name": name, "node_ids": [node_id], "chain_ids": []})
    ensure(state.get("group_id") in (None, group["id"]), "验收对象已改名；拒绝操作其他对象")
    ensure(group["node_ids"] == [node_id] and not group["chain_ids"], "验收策略组配置与私有 state 不一致")
    state["group_id"] = group["id"]
    save(state_path, state)
    return group


def ensure_writable_server(panel, server_id):
    runtime_server = panel.request(f"/api/traffic/servers/{server_id}")
    ensure(
        runtime_server.get("read_only") is not True
        and runtime_server.get("source") != "nexus",
        "只读 Nexus 兼容主机不能作为新节点配置写入目标",
    )
    return runtime_server


def installation_descriptor(value, state, agent_version=None):
    ensure(isinstance(value, dict), "安装描述缺失，请先导入兼容且已签名的 Agent Release")
    version, tag = value.get("version"), value.get("tag")
    ensure(isinstance(version, str) and len(version) <= 128
           and (version == "latest"
                or re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?", version)),
           "安装描述的 Agent 版本无效")
    ensure((version == "latest" and "tag" in value and tag is None)
           or (version != "latest" and tag == "agent-v" + version),
           "安装描述的标签与 Agent 版本不一致")
    ensure(agent_version is None or version == agent_version, "安装描述不是指定的 Agent 版本")
    token = value.get("token")
    ensure(isinstance(token, str) and 0 < len(token) <= 512
           and not any(ord(character) < 32 or ord(character) == 127 for character in token),
           "安装令牌无效")
    ensure(type(value.get("expires_at")) is int, "安装令牌缺少有效到期时间")
    ensure(value.get("origin") == state["origin"] and value.get("server_id") == state["server_id"],
           "私有安装描述对应其他环境或服务器")
    return {key: value[key] for key in ("version", "tag", "token", "expires_at", "origin", "server_id")}


def install(panel, state, state_path, refresh=False, agent_version=None):
    output = state_path.parent / "enrollment.json"
    ensure(not output.is_symlink(), "私有安装描述不能是符号链接")
    if output.exists() and not refresh:
        metadata = output.lstat()
        ensure(output.is_file() and not output.is_symlink() and metadata.st_mode & 0o077 == 0,
               "私有安装描述必须是仅当前用户可读写的普通文件")
        descriptor = installation_descriptor(json.loads(output.read_text()), state, agent_version)
    else:
        server = panel.request(f"/api/servers/{state['server_id']}")
        ensure(server["name"] == state["prefix"] + "-server", "服务器不再属于本次验收")
        query = "" if agent_version is None else "?" + urllib.parse.urlencode({"agent_version": agent_version})
        enrollment = panel.request(f"/api/servers/{state['server_id']}/enrollment" + query, {})
        selection = enrollment.get("installation")
        ensure(isinstance(selection, dict), "安装描述缺失，请先导入兼容且已签名的 Agent Release")
        descriptor = installation_descriptor({**selection, "token": enrollment.get("token"),
            "expires_at": enrollment.get("expires_at"), "origin": state["origin"],
            "server_id": state["server_id"]}, state, agent_version)
        # Persist the private token before state, so retrying does not issue another token.
        private_write(output, json.dumps(descriptor, ensure_ascii=False, indent=2) + "\n")
    state["installation"] = {key: descriptor[key] for key in ("version", "tag", "expires_at")}
    save(state_path, state)
    print("私有 enrollment.json 已保存；请通过独立可信的 apsp-bootstrap 验证并安装")


def configure_resources(panel, state, state_path):
    server = panel.request(f"/api/servers/{state['server_id']}")
    ensure(server["name"] == state["prefix"] + "-server", "服务器不再属于本次验收")
    ensure_writable_server(panel, server["id"])
    fields = {"server_id": server["id"], "public_host": state["public_host"], "sni": state["sni"]}
    if state.get("requested_port") is not None:
        fields["port"] = state["requested_port"]
    node = resource(panel, "/api/traffic/nodes", state, "node", fields, state_path)
    ensure("port" not in state or node["port"] == state["port"],
           "验收节点的端口已变化；保留原 state 并检查面板")
    user = resource(panel, "/api/access/users", state, "user", {}, state_path)
    existing = panel.request(f"/api/access/users/{user['id']}/accesses")
    ensure(all(item["node_id"] == node["id"] for item in existing),
           "专用验收用户出现其他授权；拒绝修改")
    # Nodes are granted only through policy groups; this run's group holds just its node.
    group = policy_group(panel, state, node["id"], state_path)
    panel.request(f"/api/access/users/{user['id']}/policy-groups",
                  {"group_ids": [group["id"]]}, method="PUT")
    access = next((item for item in panel.request(f"/api/access/users/{user['id']}/accesses")
                   if item["node_id"] == node["id"]), None)
    ensure(access is not None, "策略组没有把验收节点授权给专用用户")
    state["port"] = node["port"]
    state["stat_name"] = access["stat_name"]
    save(state_path, state)
    print(f"专用对象就绪：server={server['id']} node={node['id']} user={user['id']} port={node['port']}")


def prepare(panel, state, args):
    server = resource(panel, "/api/servers", state, "server", {}, args.state)
    runtime_server = ensure_writable_server(panel, server["id"])
    if getattr(args, "server_only", False):
        ensure(
            "node_id" not in state
            and runtime_server.get("configured") is False
            and (runtime_server.get("status") or {}).get("state") == "unconfigured",
            "纯监控接入准备阶段要求服务器尚未配置任何节点或链路",
        )
        install(panel, state, args.state, agent_version=getattr(args, "agent_version", None))
        print(f"专用纯监控服务器就绪：server={server['id']}")
        return
    configure_resources(panel, state, args.state)
    install(panel, state, args.state, agent_version=getattr(args, "agent_version", None))


def configure(panel, state, args):
    configure_resources(panel, state, args.state)


def usage_totals(view):
    values = []
    for field in ("uplink", "downlink", "total"):
        value = view[field]
        ensure(isinstance(value, str) and re.fullmatch(r"0|[1-9][0-9]*", value),
               "面板用量必须是精确非负十进制字符串")
        values.append(int(value))
    ensure(values[0] + values[1] == values[2], "面板上下行与总量不一致")
    return tuple(values)


def snapshot(panel, state, command=None, include_usage=True):
    server = panel.request(f"/api/servers/{state['server_id']}")
    ensure(server["name"] == state["prefix"] + "-server", "服务器不再属于本次验收")
    runtime_server = panel.request(f"/api/traffic/servers/{state['server_id']}")
    deployment_view = panel.request(f"/api/traffic/servers/{state['server_id']}/deployments")
    deployment = deployment_view["status"]
    history = deployment_view.get("history") if isinstance(deployment_view.get("history"), list) else []
    applied_entry = next(
        (item for item in history if isinstance(item, dict)
         and isinstance(deployment, dict) and item.get("rev") == deployment.get("applied_rev")),
        None,
    )
    applied_bundle_sha256 = applied_entry.get("bundle_sha256") if isinstance(applied_entry, dict) else None
    static_info = server.get("static_info") if isinstance(server.get("static_info"), dict) else {}
    components = static_info.get("components") if isinstance(static_info.get("components"), dict) else None
    runtime_version = (
        runtime_server.get("runtime_version")
        or static_info.get("runtime_version")
        or (components.get("sing-box") if isinstance(components, dict) else None)
    )
    result = {
        "observed_at": int(time.time()),
        "deployment": deployment,
        "applied_bundle_sha256": applied_bundle_sha256,
        "runtime": {
            "configured": runtime_server.get("configured"),
            "agent_supported": runtime_server.get("agent_supported"),
            "read_only": runtime_server.get("read_only"),
            "source": runtime_server.get("source"),
            "runtime_version": runtime_server.get("runtime_version"),
            "status": runtime_server.get("status"),
        },
        "publication_pending": deployment_view.get("pending"),
        "enabled_nodes": deployment_view.get("enabled_nodes"),
        "authorized_nodes": deployment_view.get("authorized_nodes"),
        "server": {
            "id": server["id"],
            "online": server["online"],
            "device_public_key": server["device_public_key"],
            "capabilities": server.get("capabilities") if isinstance(server.get("capabilities"), list) else [],
            "agent_version": static_info.get("agent_version"),
            "runtime_version": runtime_version,
            "components": components,
        },
    }
    if include_usage:
        query = urllib.parse.urlencode({"user_id": state["user_id"], "node_id": state["node_id"]})
        usage = panel.request("/api/access/usage?" + query)
        usage_totals(usage)
        result["usage"] = usage
    if command:
        try:
            process = subprocess.run(shlex.split(command), check=True, capture_output=True,
                                     text=True, timeout=20)
        except (subprocess.SubprocessError, OSError):
            raise AcceptanceError("只读 Agent status 命令失败；请在设备检查服务") from None
        ensure(len(process.stdout) <= 65536, "Agent status 响应过大")
        result["agent"] = json.loads(process.stdout)
    return result


def runtime_state(view):
    runtime = view.get("runtime")
    status = runtime.get("status") if isinstance(runtime, dict) else None
    stage = status.get("state") if isinstance(status, dict) else None
    return stage if isinstance(stage, str) and stage in SERVICE_RUNTIME_STATES else None


def healthy(view):
    status = view["deployment"]
    runtime = view.get("runtime")
    runtime_ready = True
    if isinstance(runtime, dict):
        status_view = runtime.get("status")
        runtime_ready = (
            runtime.get("configured") is True
            and runtime.get("agent_supported") is True
            and runtime.get("read_only") is not True
            and runtime.get("source") != "nexus"
            and isinstance(status_view, dict)
            and status_view.get("state") == "ready"
            and status_view.get("current_healthy") is True
        )
    capabilities = view["server"].get("capabilities")
    self_contained = (
        not isinstance(capabilities, list)
        or not capabilities
        or SELF_CONTAINED_CAPABILITY in capabilities
    )
    return (
        view["server"]["online"] is True
        and self_contained
        and status is not None
        and status["target_rev"] > 0
        and status["target_rev"] == status["applied_rev"]
        and status["healthy"] is True
        and not status["last_error"]
        and view.get("publication_pending") is not True
        and runtime_ready
    )


def readiness_status(view, expected_version):
    """Return only bounded counters and readiness booleans, never response text."""
    server = view.get("server")
    server = server if isinstance(server, dict) else {}
    deployment = view.get("deployment")
    present = isinstance(deployment, dict)
    deployment = deployment if present else {}

    def revision(name):
        value = deployment.get(name)
        return value if type(value) is int and 0 <= value <= 2 ** 63 - 1 else None

    def boolean(value):
        return value if type(value) is bool else None

    target, applied = revision("target_rev"), revision("applied_rev")
    version_required = bool(expected_version)
    result = {
        "online": boolean(server.get("online")),
        "deployment_present": present,
        "target_rev": target,
        "applied_rev": applied,
        "target_positive": target > 0 if target is not None else None,
        "revisions_match": target == applied if target is not None and applied is not None else None,
        "healthy": boolean(deployment.get("healthy")),
        "has_last_error": bool(deployment.get("last_error")) if present else None,
        "version_required": version_required,
        "version_matches": server.get("agent_version") == expected_version if version_required else None,
    }
    runtime = view.get("runtime")
    if isinstance(runtime, dict):
        if type(runtime.get("configured")) is bool:
            result["configured"] = runtime["configured"]
        if type(runtime.get("agent_supported")) is bool:
            result["agent_supported"] = runtime["agent_supported"]
        if type(runtime.get("read_only")) is bool:
            result["read_only"] = runtime["read_only"]
        stage = runtime_state(view)
        if stage is not None:
            result["runtime_state"] = stage
    if type(view.get("publication_pending")) is bool:
        result["publication_pending"] = view["publication_pending"]
    return result


def systemd_status(output, query_succeeded):
    """Filter fixed-unit systemctl properties; unknown/free-form values stay private."""
    result = {"query_succeeded": query_succeeded is True}
    if query_succeeded is not True or not isinstance(output, str) or len(output) > 4096:
        return result
    allowed = {
        "ActiveState": {"active", "reloading", "inactive", "failed", "activating", "deactivating", "maintenance", "refreshing"},
        "SubState": {"dead", "condition", "start-pre", "start", "start-post", "running", "exited", "reload",
                     "reload-signal", "reload-notify", "stop", "stop-watchdog", "stop-sigterm", "stop-sigkill",
                     "stop-post", "final-watchdog", "final-sigterm", "final-sigkill", "failed", "cleaning",
                     "auto-restart", "auto-restart-queued"},
        "Result": {"success", "resources", "timeout", "exit-code", "signal", "core-dump", "watchdog",
                   "start-limit-hit", "exec-condition", "oom-kill", "protocol"},
    }
    values = {}
    for line in output.splitlines():
        key, separator, value = line.partition("=")
        if separator and key in (*allowed, "ExecMainStatus"):
            # A repeated property is ambiguous, even if one value looks valid.
            values[key] = value if key not in values else None
    for key, options in allowed.items():
        if values.get(key) in options:
            result[key] = values[key]
    status = values.get("ExecMainStatus")
    if isinstance(status, str) and re.fullmatch(r"0|[1-9][0-9]{0,2}", status) and int(status) <= 255:
        result["ExecMainStatus"] = int(status)
    return result


def checkpoint(state, name):
    ensure(name in state["checkpoints"], "找不到指定基线；请先完成相应阶段")
    return state["checkpoints"][name]


def record(state, args, view):
    state["checkpoints"][args.label] = view
    save(args.state, state)
    save(args.state.parent / (args.label + ".json"), view)
    up, down, _ = usage_totals(view["usage"])
    print(f"阶段 {args.label} 通过：上传={up} 下载={down} 字节")


def install_status(status, drift):
    """Summarize deployment and checkpoint state with fixed vocabulary only, never error text."""
    status = status if isinstance(status, dict) else {}
    drift = drift if isinstance(drift, dict) else {}
    checkpoint_view = drift.get("checkpoint") if isinstance(drift.get("checkpoint"), dict) else {}
    error = status.get("last_error")
    error = error.lower() if isinstance(error, str) else ""
    result = {
        key: status.get(key) if type(status.get(key)) is int and 0 <= status.get(key) <= 2 ** 63 - 1 else None
        for key in ("target_rev", "applied_rev")
    }
    result["healthy"] = status.get("healthy") if type(status.get("healthy")) is bool else None
    result["error_categories"] = [name for name, pattern in ERROR_CATEGORIES
                                  if pattern.search(error)] or (["other"] if error else [])
    drift_state = drift.get("state")
    result["drift"] = drift_state if isinstance(drift_state, str) and drift_state in DRIFT_STATES else None
    result["checkpoint_supported"] = drift.get("checkpoint_supported") if type(drift.get("checkpoint_supported")) is bool else None
    for key, allowed in CHECKPOINT_VOCABULARY.items():
        value = checkpoint_view.get(key)
        result["checkpoint_" + key] = value if isinstance(value, str) and value in allowed else None
    return result


def diagnose(panel, state, args):
    """Print the deployment and checkpoint state in fixed vocabulary only."""
    has_usage = "user_id" in state and "node_id" in state
    base = f"/api/traffic/servers/{state['server_id']}/operations-view"
    print("运行时状态=" + json.dumps(
        install_status(snapshot(panel, state, include_usage=has_usage)["deployment"],
                       panel.request(base).get("drift")),
        ensure_ascii=False,
    ))


def unconfigured_monitoring(view, expected_version, require_agent_status):
    runtime = view.get("runtime") if isinstance(view.get("runtime"), dict) else {}
    status_view = runtime.get("status") if isinstance(runtime.get("status"), dict) else {}
    ensure(
        runtime.get("read_only") is not True and runtime.get("source") != "nexus",
        "只读 Nexus 兼容主机不能冒充新建纯监控服务器",
    )
    ensure(
        runtime.get("configured") is False and runtime_state(view) == "unconfigured",
        "纯监控阶段误标记为已配置或触发了运行时状态迁移",
    )
    ensure(
        status_view.get("target_rev") == 0
        and status_view.get("applied_rev") == 0
        and status_view.get("current_healthy") is False,
        "纯监控阶段出现了非零运行时版本或健康运行标记",
    )
    ensure(
        view.get("deployment") is None
        and view.get("publication_pending") is not True
        and view.get("enabled_nodes") in (None, 0),
        "纯监控阶段出现了部署记录或待发布任务",
    )
    if require_agent_status:
        agent = view.get("agent") if isinstance(view.get("agent"), dict) else {}
        ensure(
            not (agent.get("applied") or {}).get("singbox")
            and (agent.get("healthy") or {}).get("singbox") is not True,
            "纯监控阶段设备端误应用或启动了 sing-box",
        )
        if agent.get("connected") is not True:
            return False
        if expected_version and agent.get("agent_version") != expected_version:
            raise AcceptanceError("设备上报的 Agent 版本与期望版本不符")
    if view["server"].get("online") is not True:
        return False
    capabilities = view["server"].get("capabilities")
    ensure(
        isinstance(capabilities, list)
        and SELF_CONTAINED_CAPABILITY in capabilities
        and runtime.get("agent_supported") is True,
        "在线 Agent 未声明自带能力（agent:self-contained:v1），已签旧 Agent 不能冒充新自带能力",
    )
    components = view["server"].get("components")
    if components is None and view["server"].get("agent_version") is None:
        return False
    ensure(
        isinstance(components, dict) and bool(components.get("sing-box")),
        "在线一体化 Agent 未上报内置 sing-box 组件版本",
    )
    if expected_version:
        actual_version = view["server"].get("agent_version")
        if actual_version is None:
            return False
        ensure(actual_version == expected_version, "面板记录的 Agent 版本与期望版本不符")
    return True


def monitor(panel, state, args):
    ensure("node_id" not in state, "已创建验收节点，不能再执行首次纯监控核验")
    command = getattr(args, "status_command", None) or os.environ.get("APSP_E2E_STATUS_COMMAND")
    settle = getattr(args, "settle", 6)
    ensure(settle >= 1, "纯监控稳定观察窗口需为正整数秒")
    deadline = time.monotonic() + args.timeout
    first = None
    while True:
        view = snapshot(panel, state, command, include_usage=False)
        if unconfigured_monitoring(view, args.agent_version, bool(command)):
            first = view
            break
        ensure(time.monotonic() < deadline, "超时：服务器未进入在线纯监控状态")
        time.sleep(min(2, args.timeout))
    ensure(time.monotonic() + settle <= deadline, "超时：纯监控观察窗口不足")
    time.sleep(settle)
    second = snapshot(panel, state, command, include_usage=False)
    ensure(
        unconfigured_monitoring(second, args.agent_version, bool(command)),
        "纯监控观察窗口内状态异常变化",
    )
    ensure(
        second["server"]["device_public_key"] == first["server"]["device_public_key"],
        "纯监控阶段设备身份发生变化",
    )
    second["monitor_samples"] = [first]
    second["settle_secs"] = settle
    state["checkpoints"]["monitor"] = second
    save(args.state, state)
    save(args.state.parent / "monitor.json", second)
    print("纯监控阶段通过：设备在线且未启用或启动 sing-box")


def ready(panel, state, args):
    last_snapshot = args.state.with_name("ready-timeout.json")
    ensure(not last_snapshot.is_symlink(), "私有输出不能是符号链接")
    last_snapshot.unlink(missing_ok=True)
    deadline = time.monotonic() + args.timeout
    while True:
        view = snapshot(panel, state)
        runtime = view.get("runtime") if isinstance(view.get("runtime"), dict) else {}
        stage = runtime_state(view)
        capabilities = view["server"].get("capabilities")
        legacy_online = (
            view["server"].get("online") is True
            and (
                stage == "upgrade_required"
                or runtime.get("agent_supported") is False
                or (
                    isinstance(capabilities, list)
                    and len(capabilities) > 0
                    and SELF_CONTAINED_CAPABILITY not in capabilities
                )
            )
        )
        if runtime.get("read_only") is True or runtime.get("source") == "nexus":
            save(last_snapshot, {"snapshot": view, "expected_agent_version": args.agent_version})
            raise AcceptanceError(
                "目标服务器为只读 Nexus 兼容主机，拒绝继续验收；状态="
                + json.dumps(readiness_status(view, args.agent_version), ensure_ascii=False, separators=(",", ":"))
            )
        if legacy_online:
            save(last_snapshot, {"snapshot": view, "expected_agent_version": args.agent_version})
            raise AcceptanceError(
                "在线 Agent 缺少自带运行时能力（upgrade_required），已签旧 Agent 不能冒充新自带能力；状态="
                + json.dumps(readiness_status(view, args.agent_version), ensure_ascii=False, separators=(",", ":"))
            )
        if stage in ("failed", "sync_blocked"):
            save(last_snapshot, {"snapshot": view, "expected_agent_version": args.agent_version})
            raise AcceptanceError(
                "运行时配置应用或同步已明确失败；请在私有面板检查原因；状态="
                + json.dumps(readiness_status(view, args.agent_version), ensure_ascii=False, separators=(",", ":"))
            )
        if healthy(view) and (not args.agent_version
                             or view["server"]["agent_version"] == args.agent_version):
            break
        if time.monotonic() >= deadline:
            save(last_snapshot, {"snapshot": view, "expected_agent_version": args.agent_version})
            status = readiness_status(view, args.agent_version)
            raise AcceptanceError("超时：设备未在线、配置未健康应用或版本不符；状态="
                                  + json.dumps(status, ensure_ascii=False, separators=(",", ":")))
        time.sleep(min(2, args.timeout))
    if "applied_bundle_sha256" in view:
        ensure(
            isinstance(view["applied_bundle_sha256"], str)
            and re.fullmatch(r"[0-9a-f]{64}", view["applied_bundle_sha256"]),
            "面板部署历史缺少已应用配置包的 SHA256 证据",
        )
        ensure(
            view.get("enabled_nodes") == 1 and view.get("authorized_nodes") == 1,
            "面板部署视图未记录唯一的启用且已授权验收节点",
        )
    operations_view = panel.request(f"/api/traffic/servers/{state['server_id']}/operations-view")
    drift = operations_view.get("drift") if isinstance(operations_view, dict) else None
    summary_drift = install_status(view["deployment"], drift)
    if summary_drift.get("checkpoint_supported") is True:
        ensure(
            isinstance(drift, dict)
            and drift.get("target_revision") == view["deployment"]["target_rev"]
            and drift.get("applied_revision") == view["deployment"]["applied_rev"]
            and summary_drift.get("checkpoint_state") == "verified"
            and summary_drift.get("checkpoint_outcome") == "verified",
            "设备尚未通过可追踪的实际运行配置与进程核对（checkpoint）",
        )
    view["drift"] = summary_drift
    user = panel.request(f"/api/access/users/{state['user_id']}")
    token = urllib.parse.quote(user["subscription_token"], safe="")
    client = panel.request(f"/sub/{token}?format=singbox")
    proxies = [item for item in client["outbounds"] if item["type"] == "vless"]
    ensure(len(proxies) == 1 and proxies[0]["tag"] == f"node-{state['node_id']}",
           "订阅必须仅包含本次专用节点")
    mixed = [item for item in client["inbounds"] if item["type"] == "mixed"]
    ensure(len(mixed) == 1, "订阅缺少唯一的本地混合入口")
    mixed[0]["listen"] = "127.0.0.1"
    mixed[0]["listen_port"] = 2080
    if args.client_port:
        proxies[0]["server_port"] = args.client_port
    save(args.state.parent / "client.json", client)
    state["client_port"] = args.client_port or state["port"]
    args.label = "ready"
    if "ready" not in state["checkpoints"]:
        record(state, args, view)
    else:
        save(args.state, state)
        print("设备与订阅就绪；已保留初次 ready 基线")


def traffic(panel, state, args):
    baseline = checkpoint(state, args.after)
    before = usage_totals(baseline["usage"])
    deadline = time.monotonic() + args.timeout
    while True:
        view = snapshot(panel, state)
        current = usage_totals(view["usage"])
        ensure(all(new >= old for new, old in zip(current, before)), "确认用量发生倒退")
        if (healthy(view) and current[0] - before[0] >= args.min_uplink
                and current[1] - before[1] >= args.min_downlink):
            record(state, args, view)
            return
        ensure(time.monotonic() < deadline, "超时：未观察到要求的双向代理用量增量")
        time.sleep(min(5, args.timeout))


def verify(panel, state, args):
    ensure(args.interval >= 30, "稳定采样间隔不能小于 Agent 的 30 秒采样周期")
    command = args.status_command or os.environ.get("APSP_E2E_STATUS_COMMAND")
    ensure(command, "verify 需要显式提供只读 Agent status 命令以核对 outbox")
    baseline = checkpoint(state, args.unchanged_from) if args.unchanged_from else None
    expected = usage_totals(baseline["usage"]) if baseline else None
    samples, previous = [], None
    deadline = time.monotonic() + args.timeout
    while True:
        view = snapshot(panel, state, command)
        totals = usage_totals(view["usage"])
        if expected is not None:
            ensure(totals == expected, "暂停期间确认用量变化：可能仍有流量、重复入账或历史丢失")
            ensure(view["server"]["device_public_key"] == baseline["server"]["device_public_key"],
                   "设备身份发生变化")
            ensure(view["server"].get("id") == baseline["server"].get("id"), "服务器编号发生变化")
            ensure(all(view["deployment"].get(key) == baseline["deployment"].get(key)
                       for key in ("target_rev", "applied_rev")),
                   "已应用配置版本发生变化；不能作为原地升级连续性证据")
            if baseline.get("applied_bundle_sha256") is not None or view.get("applied_bundle_sha256") is not None:
                ensure(view.get("applied_bundle_sha256") == baseline.get("applied_bundle_sha256"),
                       "已应用配置包摘要发生变化；违反同配置一致性")
            ensure(view["server"].get("runtime_version") == baseline["server"].get("runtime_version"),
                   "内置运行时版本发生变化")
        agent = view["agent"]
        status = view["deployment"]
        required_version = getattr(args, "agent_version", None)
        if required_version:
            ensure(view["server"].get("agent_version") == required_version
                   and agent.get("agent_version") == required_version,
                   "面板与设备本次上报的 Agent 版本不符")
        if getattr(args, "agent_restarted", False):
            ensure(baseline is not None and type(baseline.get("agent", {}).get("pid")) is int
                   and baseline["agent"]["pid"] > 0 and type(agent.get("pid")) is int and agent["pid"] > 0,
                   "升级重启检查需要前后真实 Agent PID")
            ensure(agent["pid"] != baseline["agent"]["pid"], "Agent 进程未替换，不能证明升级成功")
        drained = (healthy(view) and agent.get("connected") is True
                   and agent.get("pending_batches") == 0
                   and agent.get("applied", {}).get("singbox") == status["applied_rev"]
                   and agent.get("healthy", {}).get("singbox") is True)
        if drained and (previous is None or totals == previous):
            samples.append(view)
        else:
            samples = [view] if drained else []
        previous = totals
        if len(samples) >= 3:
            view["stable_samples"] = samples[:-1]
            view["sample_interval_secs"] = args.interval
            record(state, args, view)
            return
        ensure(time.monotonic() + args.interval <= deadline,
               "超时：连续两采样间隔内用量未稳定或 outbox 未清空")
        time.sleep(args.interval)


def parser():
    result = argparse.ArgumentParser(description="真实 Reality 验收驱动；设备和流量由操作者控制")
    result.add_argument("--state", type=Path, required=True, help="仓库外私有目录中的 state.json")
    result.add_argument("--totp", action="store_true", help="启用 TOTP 的面板：隐藏输入本次登录验证码")
    stages = result.add_subparsers(dest="stage", required=True)
    setup = stages.add_parser("prepare", help="创建专用资源并保存安装描述，可中断恢复")
    setup.add_argument("--origin", required=True)
    setup.add_argument("--public-host", required=True)
    setup.add_argument("--sni", required=True)
    setup.add_argument("--port", type=int, help="显式指定节点监听端口；省略时由面板分配")
    setup.add_argument("--agent-version", help="首次签发安装描述时显式指定 Agent 版本")
    setup.add_argument("--server-only", action="store_true",
                       help="仅创建服务器与安装描述，待纯监控核验后再执行 configure")
    installation = stages.add_parser("install", help="取得私有安装描述，不在本机执行")
    installation.add_argument("--agent-version", help="选择已导入且协议兼容的 Agent 版本")
    installation.add_argument("--refresh", action="store_true", help="签发新令牌，用于过期重试或升级")
    monitoring = stages.add_parser("monitor", help="核验新接入的一体化 Agent 处于纯监控且未误启用 sing-box")
    monitoring.add_argument("--agent-version", help="同时核对面板与设备上报的 Agent 版本")
    monitoring.add_argument("--status-command", help="只读命令，按 shlex 分词执行；不经过本机 shell")
    monitoring.add_argument("--settle", type=int, default=6, help="纯监控连续观察窗口秒数（默认覆盖发布周期）")
    monitoring.add_argument("--timeout", type=int, default=120)
    stages.add_parser("configure", help="在已准备服务器上保存首个有效节点、专用用户与策略组以触发自动发布")
    active = stages.add_parser("ready", help="等待自动发布与健康应用，核对可追踪部署证据并保存客户端 JSON")
    active.add_argument("--agent-version", help="同时检查面板上报的 Agent 版本")
    active.add_argument("--client-port", type=int, help="本次容器端口映射的客户端目标端口")
    active.add_argument("--timeout", type=int, default=600)
    growth = stages.add_parser("traffic", help="等待本用户节点的上下行均增长")
    growth.add_argument("--label", required=True)
    growth.add_argument("--after", default="ready", help="用量基线标签")
    growth.add_argument("--min-uplink", type=int, default=1)
    growth.add_argument("--min-downlink", type=int, default=1)
    growth.add_argument("--timeout", type=int, default=180)
    stages.add_parser("diagnose", help="只用固定词汇报告部署与核对状态，不输出错误文本")
    stable = stages.add_parser("verify", help="暂停流量后检查稳定、同配置一致性、outbox 与历史用量")
    stable.add_argument("--label", required=True)
    stable.add_argument("--unchanged-from", help="必须保持原封不动的稳定基线标签")
    stable.add_argument("--status-command", help="只读命令，按 shlex 分词执行；不经过本机 shell")
    stable.add_argument("--agent-version", help="同时核对面板与设备 status 的实际 Agent 版本")
    stable.add_argument("--agent-restarted", action="store_true", help="要求 --unchanged-from 的设备 PID 已替换")
    stable.add_argument("--interval", type=int, default=35)
    stable.add_argument("--timeout", type=int, default=240)
    return result


def main():
    args = parser().parse_args()
    if args.stage == "prepare" and args.port is not None:
        ensure(1 <= args.port <= 65535 and args.port != 18085,
               "节点端口需为 1–65535，且不能占用统计接口 18085")
    os.umask(0o077)
    args.state = args.state.absolute()
    repository = Path(__file__).resolve().parents[1]
    ensure(not args.state.resolve().is_relative_to(repository), "真实验收 state 必须保存在仓库外")
    args.state.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    ensure(not args.state.is_symlink() and not args.state.parent.is_symlink(), "state 路径不能是符号链接")
    ensure(args.state.parent.stat().st_mode & 0o077 == 0, "state 目录需仅当前用户可访问")
    if args.state.exists():
        ensure(args.state.stat().st_mode & 0o077 == 0, "state 文件需仅当前用户可读写")
    if hasattr(args, "label"):
        ensure(re.fullmatch(r"[A-Za-z0-9_-]{1,64}", args.label), "阶段标签只能含字母、数字、下划线和连字符")
    if hasattr(args, "timeout"):
        ensure(args.timeout > 0, "超时必须为正数")
    if hasattr(args, "settle"):
        ensure(args.settle >= 1, "纯监控稳定观察窗口需为正整数秒")
    if hasattr(args, "min_uplink"):
        ensure(args.min_uplink > 0 and args.min_downlink > 0, "上下行验收增量必须为正数")
    if getattr(args, "client_port", None) is not None:
        ensure(1 <= args.client_port <= 65535, "客户端目标端口需为 1–65535")
    if getattr(args, "agent_restarted", False):
        ensure(args.unchanged_from and args.agent_version,
               "升级重启检查需同时指定 --unchanged-from 与 --agent-version")
    lock_descriptor = os.open(str(args.state) + ".lock", os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    with os.fdopen(lock_descriptor, "w") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        state = json.loads(args.state.read_text()) if args.state.exists() else None
        if args.stage == "prepare":
            requested = {"origin": origin(args.origin), "public_host": args.public_host, "sni": args.sni,
                         "requested_port": args.port}
            if state is None:
                state = {"schema": 1, "prefix": "apsp-e2e-" + uuid.uuid4().hex,
                         "checkpoints": {}, **requested}
                save(args.state, state)
            ensure(all(state.get(key) == value for key, value in requested.items()),
                   "此 state 对应不同环境；请使用新的私有目录")
        ensure(state is not None and state["schema"] == 1, "请先用 prepare 创建本次私有 state")
        if args.stage in ("install", "monitor", "configure", "diagnose"):
            ensure("server_id" in state, "服务器尚未创建；请先运行 prepare")
            if args.stage == "configure":
                ensure(all(key in state for key in ("public_host", "sni")),
                       "state 缺少节点参数；请先运行 prepare")
        elif args.stage != "prepare":
            ensure(all(key in state for key in ("server_id", "node_id", "user_id")),
                   "专用节点与用户尚未就绪；请先完成 prepare 或 configure")
        panel = Panel(state["origin"], password(), totp_code(args.totp))
        try:
            if args.stage == "install":
                install(panel, state, args.state, args.refresh, args.agent_version)
            else:
                globals()[args.stage](panel, state, args)
        finally:
            panel.close()


if __name__ == "__main__":
    try:
        main()
    except (AcceptanceError, KeyError, ValueError, OSError) as error:
        # JSON parse and subprocess errors can contain private paths or response text.
        message = str(error) if isinstance(error, AcceptanceError) else "验收状态或本地文件异常，请检查私有证据"
        print("验收失败：" + message, file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("验收已中断；使用相同私有 state 可恢复", file=sys.stderr)
        sys.exit(130)
