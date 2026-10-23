#!/usr/bin/env python3
"""Read-only recovery gate for independently signed Agent versions before 0.3.1."""

import errno
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import sys
import time
import urllib.parse
import uuid

SEGMENT = re.compile(r"[0-9A-Za-z][0-9A-Za-z.+_-]{0,127}\Z")
LEGACY_NODEQUALITY_VERSION = "a92fca6c0067df29ddd03fdc2fee6f3000f64545-r2"


def ensure(condition, message):
    if not condition:
        raise ValueError(message)


def protected(path):
    for part in (path,) + tuple(path.parents):
        properties = part.lstat()
        ensure(not part.is_symlink() and properties.st_uid == 0 and properties.st_mode & 0o022 == 0,
               "旧 Agent 状态路径必须由 root 保护，安装未切换")


def regular(path, limit):
    protected(path)
    ensure(path.is_file() and not path.is_symlink() and 0 < path.stat().st_size <= limit,
           "旧 Agent 状态必须是有界普通文件，安装未切换")
    return path.read_bytes()


def quiescent(saved, allow_missing=False):
    """An old process must not create Preparing after the read-only preflight."""
    environment = os.environ.copy()
    environment["LC_ALL"] = "C"
    try:
        if Path("/run/systemd/system").is_dir():
            result = subprocess.run(["systemctl", "show", "apsp-agent.service", "--property=LoadState",
                                     "--property=ActiveState", "--property=SubState", "--property=MainPID"],
                                    capture_output=True, env=environment, check=False, timeout=3)
            ensure(result.returncode == 0 and len(result.stdout) <= 4096,
                   "无法确认旧 Agent 服务停止，请先执行受控维护，安装未切换")
            properties = {}
            for line in result.stdout.decode("ascii").splitlines():
                key, separator, value = line.partition("=")
                ensure(separator and key not in properties, "旧 Agent 服务状态无效，安装未切换")
                properties[key] = value
            ensure(set(properties) == {"LoadState", "ActiveState", "SubState", "MainPID"}
                   and properties["LoadState"] in (("loaded", "not-found") if allow_missing else ("loaded",))
                   and properties["ActiveState"] == "inactive" and properties["SubState"] == "dead"
                   and properties["MainPID"] == "0", "旧 Agent 仍可能运行，请先执行受控维护，安装未切换")
        elif Path("/run/openrc/softlevel").is_file():
            exists = subprocess.run(["rc-service", "--exists", "apsp-agent"], capture_output=True,
                                    env=environment, check=False, timeout=3)
            status = subprocess.run(["rc-service", "apsp-agent", "status"], capture_output=True,
                                    env=environment, check=False, timeout=3)
            ensure((exists.returncode == 0 and status.returncode == 3)
                   or (allow_missing and exists.returncode == 1 and status.returncode == 1),
                   "无法确认旧 Agent 已停止，请先执行受控维护，安装未切换")
        else:
            raise ValueError("无法确认旧 Agent 服务状态，请先执行受控维护，安装未切换")
    except (OSError, subprocess.TimeoutExpired, UnicodeError):
        raise ValueError("无法确认旧 Agent 服务停止，请先执行受控维护，安装未切换") from None
    endpoint = saved.get("status_socket", "/run/apsp/agent.sock")
    ensure(isinstance(endpoint, str) and Path(endpoint).is_absolute()
           and ".." not in Path(endpoint).parts and not any(ord(character) < 32 for character in endpoint),
           "旧 Agent 状态端点无效，安装未切换")
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connection.settimeout(0.5)
    try:
        connection.connect(endpoint)
    except OSError as error:
        ensure(error.errno in (errno.ENOENT, errno.ECONNREFUSED),
               "无法确认旧 Agent 状态端点停止，安装未切换")
    else:
        raise ValueError("旧 Agent 状态端点仍在运行，请先执行受控维护，安装未切换")
    finally:
        connection.close()


def unique_object(pairs):
    value = {}
    for key, entry in pairs:
        ensure(key not in value, "旧诊断状态字段重复，安装未切换")
        value[key] = entry
    return value


def options_valid(value):
    return (isinstance(value, dict) and len(value) <= 32
            and all(isinstance(key, str) and len(key) <= 128 and isinstance(entry, str)
                    and len(entry) <= 1024 for key, entry in value.items()))


def optional_fields_valid(value):
    return ((value.get("expires_at") is None or type(value["expires_at"]) is int
             and -(2 ** 63) <= value["expires_at"] < 2 ** 63)
            and all(value.get(key) is None or isinstance(value[key], str) and len(value[key]) <= 4096
                    for key in ("start_error", "protection_stop_reason")))


def checkpoint_safe(encoded):
    try:
        checkpoint = json.loads(encoded, object_pairs_hook=unique_object,
                                parse_constant=lambda _value: (_ for _ in ()).throw(ValueError()))
    except (ValueError, RecursionError):
        raise ValueError("旧诊断状态无法安全解析，安装未切换") from None
    if checkpoint is None:
        return
    ensure(isinstance(checkpoint, dict) and len(checkpoint) == 1
           and next(iter(checkpoint)) in ("Preparing", "Started"), "旧诊断检查点未知，安装未切换")
    phase = next(iter(checkpoint))
    value = checkpoint[phase]
    ensure(isinstance(value, dict), "旧诊断检查点损坏，安装未切换")
    if phase == "Started":
        ensure({"spec", "service", "plugin", "started_at"} <= set(value)
               and set(value) <= {"spec", "service", "plugin", "started_at", "start_error",
                                  "expires_at", "protection_stop_reason", "environment"}
               and isinstance(value["spec"], dict) and isinstance(value["service"], dict)
               and value["plugin"] == "nodequality" and type(value["started_at"]) is int
               and 0 <= value["started_at"] < 2 ** 64 and optional_fields_valid(value)
               and (value.get("environment") is None or isinstance(value["environment"], dict)),
               "旧已启动检查点损坏，安装未切换")
        spec, service = value["spec"], value["service"]
        ensure(set(spec) == {"id", "version", "binary_path", "job_dir", "timeout_secs", "options"}
               and {"unit", "program", "args", "working_directory", "timeout_secs"} <= set(service)
               and set(service) <= {"unit", "program", "args", "working_directory", "timeout_secs",
                                    "memory_max", "tasks_max", "cpu_weight", "io_weight", "oom_score_adjust"}
               and isinstance(spec.get("version"), str) and SEGMENT.fullmatch(spec["version"])
               and isinstance(spec.get("id"), str) and len(spec["id"]) == 36
               and isinstance(spec.get("binary_path"), str) and Path(spec["binary_path"]).is_absolute()
               and isinstance(spec.get("job_dir"), str) and Path(spec["job_dir"]).is_absolute()
               and type(spec.get("timeout_secs")) is int and 1 <= spec["timeout_secs"] <= 3600
               and type(service.get("timeout_secs")) is int and service["timeout_secs"] == spec["timeout_secs"]
               and options_valid(spec.get("options")) and isinstance(service.get("args"), list)
               and len(service["args"]) <= 64
               and all(isinstance(argument, str) and len(argument) <= 4096 for argument in service["args"])
               and service.get("working_directory") == spec["job_dir"]
               and service.get("program") == spec["binary_path"]
               and service.get("unit") == "apsp-diagnostic-" + spec["id"] + ".service",
               "旧已启动检查点版本或执行身份无效，安装未切换")
        ensure(all(type(service[key]) is int for key in
                   {"memory_max", "tasks_max", "cpu_weight", "io_weight", "oom_score_adjust"} & set(service)),
               "旧已启动检查点资源形状无效，安装未切换")
        ensure(spec["version"] == LEGACY_NODEQUALITY_VERSION
               and set(spec["options"]) <= {"ip_version", "network_mode", "upload_report"}
               and spec["options"].get("ip_version", "both") in ("both", "ipv4", "ipv6")
               and spec["options"].get("network_mode", "low") in ("low", "normal")
               and spec["options"].get("upload_report", "false") in ("true", "false"),
               "旧 Agent 无法按原版本回收此已启动任务；请保留状态并使用兼容的新签名 Agent，安装未切换")
        try:
            uuid.UUID(spec["id"])
        except (ValueError, TypeError, AttributeError):
            raise ValueError("旧已启动检查点身份无效，安装未切换") from None
        # Existing compiled-root verify-cache validates the exact saved executable proof;
        # the old Agent only observes Started instead of executing it again.
        return
    ensure({"id", "version", "artifact", "timeout_secs"} <= set(value)
           and set(value) <= {"id", "version", "artifact", "timeout_secs", "plugin", "options",
                              "expires_at", "resource_budget"}
           and isinstance(value["version"], str) and SEGMENT.fullmatch(value["version"])
           and type(value["timeout_secs"]) is int and 1 <= value["timeout_secs"] <= 3600
           and isinstance(value["artifact"], dict) and optional_fields_valid(value),
           "旧排队检查点损坏，安装未切换")
    try:
        ensure(isinstance(value["id"], str) and len(value["id"]) == 36, "invalid checkpoint UUID")
        uuid.UUID(value["id"])
    except (ValueError, TypeError, AttributeError):
        raise ValueError("旧排队检查点身份无效，安装未切换") from None
    artifact = value["artifact"]
    proof = artifact.get("proof")
    ensure(set(artifact) == {"url", "sha256", "proof"} and isinstance(artifact["url"], str)
           and 0 < len(artifact["url"]) <= 8192 and isinstance(artifact["sha256"], str)
           and re.fullmatch(r"[0-9a-f]{64}", artifact["sha256"])
           and isinstance(proof, dict) and set(proof) == {"metadata_json", "checksums", "signature"}
           and all(isinstance(proof[key], str) and 0 < len(proof[key]) <= limit
                   for key, limit in (("metadata_json", 32768), ("checksums", 8192), ("signature", 16384))),
           "旧排队检查点签名证明形状无效，安装未切换")
    options = value.get("options", {})
    budget = value.get("resource_budget")
    ensure(budget is None or isinstance(budget, dict)
           and set(budget) == {"memory_max", "tasks_max", "cpu_weight", "io_weight", "oom_score_adjust"}
           and all(type(entry) is int for entry in budget.values()),
           "旧排队检查点资源形状无效，安装未切换")
    ensure(options_valid(options) and value.get("plugin", "nodequality") == "nodequality",
           "旧排队检查点插件或参数未知，安装未切换")
    mode = options.get("mode", "full")
    ensure(mode in ("full", "daily"), "旧排队检查点模式未知，安装未切换")
    ensure(mode != "full", "旧 Agent 存在尚未启动的完整验机任务；恢复门禁拒绝，原状态保留，安装未切换")


def _preflight(version, configuration):
    ensure(isinstance(version, str) and re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?", version),
           "旧 Agent 版本无法安全确认")
    if tuple(int(part) for part in version.split("-")[0].split("+")[0].split(".")) > (0, 3, 0):
        return
    configuration = Path(configuration)
    database, saved = Path("/var/lib/apsp/core/state.db"), {}
    if configuration.exists() or configuration.is_symlink():
        try:
            import tomllib
        except ImportError:
            raise ValueError("旧 Agent 恢复预检需要 Python 3.11 或更新版本，安装未切换") from None
        try:
            saved = tomllib.loads(regular(configuration, 1048576).decode("utf-8"))
        except (ValueError, UnicodeError):
            raise ValueError("旧 Agent 配置无法安全解析，安装未切换") from None
        state_path = saved.get("state_db", str(database))
        ensure(isinstance(state_path, str) and Path(state_path).is_absolute()
               and not any(ord(character) < 32 for character in state_path)
               and ".." not in Path(state_path).parts, "旧 Agent 状态位置无法安全确认，安装未切换")
        database = Path(state_path)
        quiescent(saved, allow_missing=not database.exists())
    if not database.exists() and not database.is_symlink():
        return
    if not (configuration.exists() or configuration.is_symlink()):
        raise ValueError("旧 Agent 状态存在但配置缺失，安装未切换")
    protected(database)
    ensure(database.is_file(), "旧 Agent 状态必须是普通文件，安装未切换")
    wal, shm = (Path(str(database) + suffix) for suffix in ("-wal", "-shm"))
    for sidecar in (wal, shm):
        if sidecar.exists() or sidecar.is_symlink():
            protected(sidecar)
            ensure(sidecar.is_file(), "旧 Agent 状态附属文件无效，安装未切换")
    ensure(not wal.exists() or wal.stat().st_size == 0 or shm.exists(),
           "旧 Agent WAL 缺少共享内存，拒绝创建恢复文件，安装未切换")
    try:
        import sqlite3
    except ImportError:
        raise ValueError("旧 Agent 状态预检需要 Python 标准库 SQLite 支持，安装未切换") from None
    status = lambda: [(path.exists(), path.stat().st_ino, path.stat().st_size, path.stat().st_mtime_ns)
                      if path.exists() else (False,) for path in (database, wal, shm)]
    before, deadline = status(), time.monotonic() + 1
    connection = None
    try:
        uri = "file:" + urllib.parse.quote(str(database), safe="/") + "?mode=ro"
        connection = sqlite3.connect(uri, uri=True, timeout=1)
        connection.set_progress_handler(lambda: int(time.monotonic() >= deadline), 1000)
        rows = connection.execute(
            "SELECT substr(value,1,1048577),typeof(value) FROM kv WHERE key='diagnostics:active' LIMIT 2").fetchall()
    except sqlite3.Error:
        raise ValueError("旧诊断状态无法只读确认，安装未切换") from None
    finally:
        if connection is not None:
            connection.close()
    ensure(status() == before, "旧诊断状态在预检期间变化，安装未切换")
    ensure(len(rows) <= 1, "旧诊断状态重复，安装未切换")
    if rows:
        encoded, kind = rows[0]
        ensure(kind == "text" and isinstance(encoded, str) and len(encoded.encode("utf-8")) <= 1048576,
               "旧诊断状态不是有界 JSON，安装未切换")
        checkpoint_safe(encoded)


def preflight(version, configuration=Path("/etc/apsp/agent.toml")):
    try:
        _preflight(version, configuration)
    except OSError:
        raise ValueError("旧状态路径无法安全读取，安装未切换") from None


if __name__ == "__main__":
    try:
        ensure(len(sys.argv) == 2, "旧 Agent 预检参数无效")
        preflight(sys.argv[1])
    except ValueError as error:
        raise SystemExit(f"Legacy Agent refused: {error}") from None
    except OSError:
        raise SystemExit("Legacy Agent refused: 旧状态路径无法安全读取，安装未切换") from None
