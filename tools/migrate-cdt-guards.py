#!/usr/bin/env python3
"""Preview or import the two legacy CDT profiles into paused APSP policies."""
from __future__ import annotations

import argparse
from contextlib import ExitStack
from dataclasses import dataclass, field
from datetime import datetime
from decimal import Decimal, InvalidOperation
import fcntl
import hashlib
import http.cookies
import json
import os
from pathlib import Path
import re
import shlex
import tempfile
import urllib.error
import urllib.parse
import urllib.request

GIB = 1024 ** 3
MAX_BYTES = 2 ** 53 - 1
PROFILES = [
    ("config.env", "traffic-guard.lock", "CDT 守护 · 原账号"),
    ("config-hk-cdt-200.env", "hk-cdt-200.lock", "CDT 守护 · 第二账号"),
]


class MigrationError(Exception):
    pass


def env_file(path: Path) -> dict[str, str]:
    """Read literal assignments without executing the legacy shell config."""
    result = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        match = re.fullmatch(r"(?:export\s+)?([A-Z][A-Z0-9_]*)=(.*)", line)
        if not match or "$" in match[2] or "`" in match[2]:
            raise MigrationError("配置必须使用字面量赋值，不能包含 shell 表达式")
        values = shlex.split(match[2], comments=True)
        if len(values) != 1 or match[1] in result:
            raise MigrationError("配置包含空值、多值或重复字段")
        result[match[1]] = values[0]
    return result


def gib_bytes(value: str) -> int:
    try:
        amount = Decimal(value) * GIB
    except InvalidOperation:
        raise MigrationError("GiB 阈值无效") from None
    if not amount.is_finite() or not 0 < amount <= MAX_BYTES or amount != amount.to_integral_value():
        raise MigrationError("GiB 阈值必须能精确表示为正整数字节数")
    return int(amount)


@dataclass
class Profile:
    filename: str
    name: str
    region: str
    instance: str
    key: str = field(repr=False)
    secret: str = field(repr=False)
    threshold: int
    jump: int
    state: dict

    def fingerprint(self):
        return hashlib.sha256((self.key + ":" + self.region + ":" + self.instance).encode()).hexdigest()

    def summary(self):
        return {"profile": self.filename, "name": self.name, "region": self.region,
                "threshold_gib": self.threshold / GIB, "jump_gib": self.jump / GIB,
                "billing_cycle": self.state["billing_cycle"],
                "max_usage_gib": int(self.state["max_usage_bytes"]) / GIB,
                "max_jump_gib": int(self.state["max_jump_bytes"]) / GIB}


def read_profiles(source: Path) -> list[Profile]:
    profiles = []
    for filename, _, name in PROFILES:
        config = env_file(source / filename)
        required = ["ALIYUN_ACCESS_KEY_ID", "ALIYUN_ACCESS_KEY_SECRET", "ALIYUN_REGION_ID", "ALIYUN_ECS_INSTANCE_ID"]
        if any(not config.get(k) for k in required):
            raise MigrationError("配置缺少账号、地域或实例字段")
        region, instance = config[required[2]], config[required[3]]
        if not re.fullmatch(r"[a-z][a-z0-9-]+", region) or not re.fullmatch(r"i-[A-Za-z0-9]+", instance):
            raise MigrationError("地域或实例格式无效")
        state_dir = Path(config.get("TRAFFIC_STATE_DIR", str(source / "state")))
        if not state_dir.is_absolute():
            raise MigrationError("状态目录必须使用绝对路径")
        state = json.loads((state_dir / f"{instance}.json").read_text())
        pool = "mainland" if region.startswith("cn-") and region != "cn-hongkong" else "non-mainland"
        if state.get("version") != 1 or state.get("identity") != {"instance_id": instance, "region_id": region, "quota_pool": pool}:
            raise MigrationError("历史状态身份或版本不匹配")
        state = {k: state[k] for k in ["version", "identity", "billing_cycle", "max_usage_bytes", "observed_at"]} | {"max_jump_bytes": state.get("max_jump_bytes", "0")}
        observed = datetime.fromisoformat(state["observed_at"])
        if observed.tzinfo is None:
            raise MigrationError("历史状态时间必须包含时区")
        state["observed_at"] = int(observed.timestamp())
        for key in ["max_usage_bytes", "max_jump_bytes"]:
            if not re.fullmatch(r"[0-9]+", str(state[key])) or not 0 <= int(state[key]) <= MAX_BYTES:
                raise MigrationError("历史用量必须为整数字节数")
            state[key] = str(state[key])
        if int(state["max_jump_bytes"]) > int(state["max_usage_bytes"]):
            raise MigrationError("最大突增不能超过月度最高用量")
        profiles.append(Profile(filename, name, region, instance, config[required[0]], config[required[1]],
                                gib_bytes(config.get("TRAFFIC_THRESHOLD_GIB") or config.get("TRAFFIC_THRESHOLD_GB", "")),
                                gib_bytes(config.get("TRAFFIC_JUMP_GIB") or "20"), state))
    if len({p.fingerprint() for p in profiles}) != len(profiles):
        raise MigrationError("不能重复迁移同一账号与实例")
    return profiles


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        raise MigrationError("面板发生重定向，请使用最终 HTTPS 地址或本机回环地址")


class Panel:
    def __init__(self, origin: str, password: str):
        url = urllib.parse.urlsplit(origin)
        if url.username or url.password or url.path not in ("", "/") or url.query or url.fragment or not url.hostname:
            raise MigrationError("面板地址必须是无路径、无凭据的源地址")
        if url.scheme != "https" and not (url.scheme == "http" and url.hostname in ("127.0.0.1", "::1", "localhost")):
            raise MigrationError("只允许 HTTPS 或本机回环 HTTP")
        self.origin, self.cookie = origin.rstrip("/"), ""
        self.opener = urllib.request.build_opener(NoRedirect())
        self.request("/api/login", "POST", {"password": password})

    def request(self, path, method="GET", body=None):
        headers = {"Content-Type": "application/json"}
        if self.cookie:
            headers["Cookie"] = self.cookie
        request = urllib.request.Request(self.origin + path, method=method, headers=headers,
                                         data=None if body is None else json.dumps(body).encode())
        try:
            with self.opener.open(request, timeout=90) as response:
                if response.headers.get("Set-Cookie"):
                    cookies = http.cookies.SimpleCookie(response.headers["Set-Cookie"])
                    self.cookie = "; ".join(f"{k}={v.value}" for k, v in cookies.items())
                data = response.read()
                return json.loads(data) if data else None
        except urllib.error.HTTPError as error:
            # Do not echo remote bodies, credentials, cookies or request payloads.
            raise MigrationError(f"面板请求 {method} {path} 返回 HTTP {error.code}") from None


def save(path: Path, value: dict):
    path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", dir=path.parent, delete=False) as output:
        temp = Path(output.name)
        try:
            json.dump(value, output, ensure_ascii=False, indent=2)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
            temp.replace(path)
        finally:
            temp.unlink(missing_ok=True)


def policy(profile: Profile) -> dict:
    return {"enabled": False, "stop_mode": "StopCharging", "threshold_action": "off", "limit_gb": 100,
            "threshold_percent": 95, "schedule_enabled": False, "start_time": "08:00", "stop_time": "23:00",
            "utc_offset_minutes": 480, "keepalive": False,
            "cdt_guard": {"threshold_bytes": profile.threshold, "jump_bytes": profile.jump, "auto_start": True}}


def prepare(panel: Panel, profiles: list[Profile], journal_path: Path):
    journal = json.loads(journal_path.read_text()) if journal_path.exists() else {"panel": panel.origin, "profiles": {}}
    if journal["panel"] != panel.origin:
        raise MigrationError("迁移记录属于其他面板")
    if not panel.request("/api/control-center/credentials")["encryption_ready"]:
        raise MigrationError("请先配置 APSP 凭据加密密钥环，再导入云密钥")
    for profile in profiles:
        record = journal["profiles"].setdefault(profile.filename, {"fingerprint": profile.fingerprint()})
        if record["fingerprint"] != profile.fingerprint():
            raise MigrationError("源账号或实例已变化，请核对迁移记录")
        overview = panel.request("/api/plugins/alicloud")
        if "resource_id" in record:
            resource = next(r for r in overview["resources"] if r["id"] == record["resource_id"])
            account = next(a for a in overview["accounts"] if a["id"] == record["account_id"])
            if resource["account_id"] != account["id"] or resource["region"] != profile.region or resource["cloud_id"] != profile.instance or resource["kind"] != "ecs" or account.get("credential_id") != record["credential_id"]:
                raise MigrationError("目标账号、凭据引用或资源身份与迁移记录不匹配")
            if resource["power_policy"]["enabled"] or account["enabled"]:
                raise MigrationError("目标仍在运行；重导状态前请暂停目标策略与账号")
        elif any(r["region"] == profile.region and r["cloud_id"] == profile.instance for r in overview["resources"]):
            raise MigrationError("实例已登记且不在本次迁移记录中，请先核对现有管理关系")
        if "credential_id" not in record:
            record["credential_id"] = panel.request("/api/control-center/credentials", "POST", {
                "name": profile.name, "kind": "cloud", "secret": {"provider": "alicloud", "access_key_id": profile.key, "access_key_secret": profile.secret}})["id"]
            save(journal_path, journal)
        if "account_id" not in record:
            record["account_id"] = panel.request("/api/plugins/alicloud/accounts", "POST", {
                "name": profile.name, "site": "china", "enabled": False, "auto_enabled": False,
                "limit_gb": 100, "credential_id": record["credential_id"]})["id"]
            save(journal_path, journal)
        if "resource_id" not in record:
            record["resource_id"] = panel.request("/api/plugins/alicloud/resources", "POST", {
                "account_id": record["account_id"], "name": profile.name, "kind": "ecs", "region": profile.region,
                "cloud_id": profile.instance, "auto_enabled": False, "cap_mbps": 1})["id"]
            save(journal_path, journal)
        path = "/api/plugins/alicloud/resources/" + record["resource_id"]
        resource = next(r for r in panel.request("/api/plugins/alicloud")["resources"] if r["id"] == record["resource_id"])
        panel.request(path + "/power-policy", "PATCH", {"revision": resource["revision"], "policy": policy(profile)})
        resource = next(r for r in panel.request("/api/plugins/alicloud")["resources"] if r["id"] == record["resource_id"])
        panel.request(path + "/cdt-state", "POST", {"revision": resource["revision"], "state": profile.state})
        record["prepared"] = True
        save(journal_path, journal)
    return journal


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--apply", action="store_true", help="导入为暂停状态，不停用 cron、不启停云实例")
    parser.add_argument("--panel-url")
    parser.add_argument("--password-file", type=Path)
    parser.add_argument("--journal", type=Path)
    args = parser.parse_args()
    os.umask(0o077)
    source = args.source.resolve()
    try:
        with ExitStack() as stack:
            for _, lock, _ in PROFILES:
                stream = stack.enter_context((source / lock).open("a"))
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
            profiles = read_profiles(source)
        print(json.dumps({"profiles": [p.summary() for p in profiles], "mode": "paused-import" if args.apply else "preview"}, ensure_ascii=False, indent=2))
        if args.apply:
            if not all([args.panel_url, args.password_file, args.journal]):
                raise MigrationError("导入需要面板地址、密码文件和迁移记录路径")
            if args.password_file.stat().st_mode & 0o077:
                raise MigrationError("管理员密码文件权限必须为 0600")
            panel = Panel(args.panel_url, args.password_file.read_text().strip())
            prepare(panel, profiles, args.journal)
            print("两个配置与月度历史已导入为暂停状态；原 cron 尚未变更。")
    except (MigrationError, OSError, ValueError, KeyError, StopIteration) as error:
        print(str(error) if isinstance(error, MigrationError) else "迁移未完成，请检查源文件、锁、状态格式和面板连接；原定时任务未变更。")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
