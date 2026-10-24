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
        headers = {} if body is None else {"Content-Type": "application/json"}
        request = urllib.request.Request(self.base + path, data=body, headers=headers, method=method)
        try:
            with self.client.open(request, timeout=20) as response:
                content = response.read(2 * 1024 * 1024 + 1)
        except urllib.error.HTTPError as error:
            # Do not expose enrollment or subscription tokens from request URLs.
            raise AcceptanceError(f"面板请求失败，HTTP {error.code}") from None
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


def installation_descriptor(value, state, agent_version=None):
    ensure(isinstance(value, dict), "安装描述缺失，请先导入兼容且已签名的 Release")
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
        ensure(isinstance(selection, dict), "安装描述缺失，请先导入兼容且已签名的 Release")
        descriptor = installation_descriptor({**selection, "token": enrollment.get("token"),
            "expires_at": enrollment.get("expires_at"), "origin": state["origin"],
            "server_id": state["server_id"]}, state, agent_version)
        # Persist the private token before state, so retrying does not issue another token.
        private_write(output, json.dumps(descriptor, ensure_ascii=False, indent=2) + "\n")
    state["installation"] = {key: descriptor[key] for key in ("version", "tag", "expires_at")}
    save(state_path, state)
    print("私有 enrollment.json 已保存；请通过独立可信的 apsp-bootstrap 验证并安装")


def prepare(panel, state, args):
    server = resource(panel, "/api/servers", state, "server", {}, args.state)
    panel.request(f"/api/plugins/sing-box/servers/{server['id']}/enable", {})
    fields = {"server_id": server["id"], "public_host": state["public_host"], "sni": state["sni"]}
    if state.get("requested_port") is not None:
        fields["port"] = state["requested_port"]
    node = resource(panel, "/api/plugins/sing-box/nodes", state, "node", fields, args.state)
    ensure("port" not in state or node["port"] == state["port"],
           "验收节点的端口已变化；保留原 state 并检查面板")
    user = resource(panel, "/api/plugins/sing-box/users", state, "user", {}, args.state)
    existing = panel.request(f"/api/plugins/sing-box/users/{user['id']}/accesses")
    ensure(all(item["node_id"] == node["id"] for item in existing),
           "专用验收用户出现其他授权；拒绝修改")
    access = panel.request(f"/api/plugins/sing-box/users/{user['id']}/accesses", {"node_id": node["id"]})
    state["port"] = node["port"]
    state["stat_name"] = access["stat_name"]
    save(args.state, state)
    install(panel, state, args.state)
    print(f"专用对象就绪：server={server['id']} node={node['id']} user={user['id']} port={node['port']}")


def usage_totals(view):
    values = []
    for field in ("uplink", "downlink", "total"):
        value = view[field]
        ensure(isinstance(value, str) and re.fullmatch(r"0|[1-9][0-9]*", value),
               "面板用量必须是精确非负十进制字符串")
        values.append(int(value))
    ensure(values[0] + values[1] == values[2], "面板上下行与总量不一致")
    return tuple(values)


def snapshot(panel, state, command=None):
    server = panel.request(f"/api/servers/{state['server_id']}")
    ensure(server["name"] == state["prefix"] + "-server", "服务器不再属于本次验收")
    plugin = panel.request(f"/api/plugins/sing-box/servers/{state['server_id']}")
    deployment_view = panel.request(f"/api/plugins/sing-box/servers/{state['server_id']}/deployments")
    deployment = deployment_view["status"]
    query = urllib.parse.urlencode({"user_id": state["user_id"], "node_id": state["node_id"]})
    usage = panel.request("/api/plugins/sing-box/usage?" + query)
    usage_totals(usage)
    result = {
        "observed_at": int(time.time()), "usage": usage, "deployment": deployment,
        "installation": plugin.get("installation"), "publication_pending": deployment_view.get("pending"),
        "server": {"id": server["id"], "online": server["online"],
                   "device_public_key": server["device_public_key"],
                   "agent_version": server["static_info"].get("agent_version"),
                   "runtime_version": server["static_info"].get("runtime_version")},
    }
    if command:
        try:
            process = subprocess.run(shlex.split(command), check=True, capture_output=True,
                                     text=True, timeout=20)
        except (subprocess.SubprocessError, OSError):
            raise AcceptanceError("只读 Agent status 命令失败；请在设备检查服务") from None
        ensure(len(process.stdout) <= 65536, "Agent status 响应过大")
        result["agent"] = json.loads(process.stdout)
    return result


def healthy(view):
    status = view["deployment"]
    return (view["server"]["online"] and status is not None and status["target_rev"] > 0
            and status["target_rev"] == status["applied_rev"] and status["healthy"]
            and not status["last_error"] and view.get("publication_pending") is not True
            and (view.get("installation") is None or view["installation"].get("state") == "ready"))


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
    installation = view.get("installation")
    if isinstance(installation, dict):
        stage = installation.get("state")
        if isinstance(stage, str) and stage in ("not_enabled", "queued", "waiting_agent", "offline", "pending", "ready", "failed"):
            result["installation_state"] = stage
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


RUNTIME_PREFLIGHT_CAPABILITY = "fleet:runtime-preflight:v1"


def blocking_checks(view):
    return sorted({check.get("key", "?") for check in view.get("checks") or []
                   if isinstance(check, dict) and check.get("blocking") is True})


def collect_preflight(panel, base, deadline):
    request = panel.request(base + "/preflight", {})
    ensure(isinstance(request.get("id"), str), "预检请求未返回编号")
    while True:
        view = panel.request(base).get("preflight") or {}
        if view.get("id") == request["id"] and view.get("device_checks_pending") is False:
            return view
        if time.monotonic() >= deadline:
            states = [item.get("status") for item in view.get("operations") or [] if isinstance(item, dict)]
            raise AcceptanceError("超时：设备未完成只读运行时预检；匹配请求=" + str(view.get("id") == request["id"])
                                  + " 操作状态=" + ",".join(map(str, states)))
        time.sleep(2)


def reauthenticate(panel, args):
    # Bootstrap and confirmation require a proof younger than five minutes;
    # device sampling and the first runtime install can outlast the login one.
    panel.request("/api/control-center/reauth", {"password": password(), "totp_code": totp_code(args.totp)})


def preflight(panel, state, args):
    """Run the operator flow the panel requires before publishing business nodes.

    Enable read-only runtime inspection for this server, collect the device
    preflight, install the empty runtime first when no deployment exists yet,
    then collect again and confirm. Only check keys are reported, never values.
    """
    deadline = time.monotonic() + args.timeout
    server = state["server_id"]
    profile = panel.request(f"/api/servers/{server}/fleet")
    if profile["policy"].get("runtime_inspection") is not True:
        panel.request(f"/api/servers/{server}/fleet", {
            "asset": profile["asset"], "policy": {**profile["policy"], "runtime_inspection": True},
            "expected_digest": profile["digest"]}, method="PUT")
    # The Agent advertises the capability only after it reloads the local
    # policy; requesting earlier is refused by the panel.
    while RUNTIME_PREFLIGHT_CAPABILITY not in (panel.request(f"/api/servers/{server}/fleet").get("capabilities") or []):
        ensure(time.monotonic() < deadline, "超时：Agent 未声明只读运行时预检能力，请检查本机 fleet-policy.json")
        time.sleep(2)
    base = f"/api/plugins/sing-box/servers/{server}/operations-view"
    view = collect_preflight(panel, base, deadline)
    bootstrap = view.get("bootstrap") or {}
    if bootstrap.get("available") is True:
        ensure(bootstrap.get("ready") is True,
               "首次空运行时安装预检未通过；阻塞项=" + ",".join(blocking_checks(bootstrap)))
        reauthenticate(panel, args)
        panel.request(base + "/preflight/bootstrap", {"id": view["id"], "confirm": True})
        while True:
            status = snapshot(panel, state)["deployment"]
            if (status and (status.get("target_rev") or 0) > 0
                    and status.get("applied_rev") == status.get("target_rev") and status.get("healthy") is True):
                break
            ensure(time.monotonic() < deadline, "超时：空运行时首次安装未完成；状态=" + json.dumps(
                {key: (status or {}).get(key) for key in ("target_rev", "applied_rev", "healthy")}))
            time.sleep(2)
        view = collect_preflight(panel, base, deadline)
    ensure(view.get("ready") is True, "部署预检未通过；阻塞项=" + ",".join(blocking_checks(view)))
    reauthenticate(panel, args)
    confirmed = panel.request(base + "/preflight/confirm", {"id": view["id"], "confirm": True})
    ensure(confirmed.get("confirmed") is True, "部署预检确认未生效")
    state.setdefault("checkpoints", {})["preflight"] = {"id": view["id"], "confirmed_at": int(time.time())}
    save(args.state, state)
    print("部署预检已确认：" + str(len(view.get("checks") or [])) + " 项检查")


def ready(panel, state, args):
    last_snapshot = args.state.with_name("ready-timeout.json")
    ensure(not last_snapshot.is_symlink(), "私有输出不能是符号链接")
    last_snapshot.unlink(missing_ok=True)
    deadline = time.monotonic() + args.timeout
    while True:
        view = snapshot(panel, state)
        if isinstance(view.get("installation"), dict) and view["installation"].get("state") == "failed":
            save(last_snapshot, {"snapshot": view, "expected_agent_version": args.agent_version})
            raise AcceptanceError("安装已明确失败；请在私有面板检查缺制品或应用原因；状态="
                                  + json.dumps(readiness_status(view, args.agent_version), ensure_ascii=False, separators=(",", ":")))
        if healthy(view) and (not args.agent_version
                             or view["server"]["agent_version"] == args.agent_version):
            break
        if time.monotonic() >= deadline:
            save(last_snapshot, {"snapshot": view, "expected_agent_version": args.agent_version})
            status = readiness_status(view, args.agent_version)
            raise AcceptanceError("超时：设备未在线、配置未健康应用或版本不符；状态="
                                  + json.dumps(status, ensure_ascii=False, separators=(",", ":")))
        time.sleep(min(2, args.timeout))
    user = panel.request(f"/api/plugins/sing-box/users/{state['user_id']}")
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
            ensure(view["server"].get("runtime_version") == baseline["server"].get("runtime_version"),
                   "独立运行时版本发生变化")
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
    installation = stages.add_parser("install", help="取得私有安装描述，不在本机执行")
    installation.add_argument("--agent-version", help="选择已导入且协议兼容的 Agent 版本")
    installation.add_argument("--refresh", action="store_true", help="签发新令牌，用于过期重试或升级")
    checks = stages.add_parser("preflight", help="开启只读运行时预检，完成首次空安装并确认部署预检")
    checks.add_argument("--timeout", type=int, default=600)
    active = stages.add_parser("ready", help="等待健康应用，保存独立客户端 JSON")
    active.add_argument("--agent-version", help="同时检查面板上报的 Agent 版本")
    active.add_argument("--client-port", type=int, help="本次容器端口映射的客户端目标端口")
    active.add_argument("--timeout", type=int, default=600)
    growth = stages.add_parser("traffic", help="等待本用户节点的上下行均增长")
    growth.add_argument("--label", required=True)
    growth.add_argument("--after", default="ready", help="用量基线标签")
    growth.add_argument("--min-uplink", type=int, default=1)
    growth.add_argument("--min-downlink", type=int, default=1)
    growth.add_argument("--timeout", type=int, default=180)
    stable = stages.add_parser("verify", help="暂停流量后检查稳定、outbox 与历史用量")
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
        if args.stage != "prepare":
            ensure(all(key in state for key in ("server_id", "node_id", "user_id")),
                   "prepare 尚未完成；请使用相同参数重试")
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
