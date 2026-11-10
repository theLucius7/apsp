#!/usr/bin/env python3
"""Bounded configured TCP checks and optional official node-IP self queries."""
import json
import importlib.util
import ipaddress
import multiprocessing
import os
import pathlib
import socket
import sys
import time


def resolve_child(target, port, family, sender):
    try:
        values = socket.getaddrinfo(target, port, family, socket.SOCK_STREAM)
        chosen = {}
        for kind, _, _, _, address in values:
            if kind not in (socket.AF_INET, socket.AF_INET6):
                continue
            ip = ipaddress.ip_address(address[0])
            if not ip.is_unspecified and not ip.is_multicast and not (ip.version == 6 and ip.ipv4_mapped):
                chosen.setdefault(kind, address)
        # A long list for one family must not hide the other requested family.
        sender.send(list(chosen.items()))
    except OSError as error:
        sender.send(str(error)[:512])
    finally:
        sender.close()


def resolve(target, port, family):
    context = multiprocessing.get_context("spawn")
    receiver, sender = context.Pipe(duplex=False)
    process = context.Process(target=resolve_child, args=(target, port, family, sender))
    process.start()
    sender.close()
    try:
        if not receiver.poll(2):
            raise TimeoutError("DNS 解析超过 2 秒")
        result = receiver.recv()
        if isinstance(result, str):
            raise OSError(result)
        if not result:
            raise OSError("没有匹配 IP 版本的地址")
        chosen = {}
        for kind, address in result:
            ip = ipaddress.ip_address(address[0])
            if not ip.is_unspecified and not ip.is_multicast and not (ip.version == 6 and ip.ipv4_mapped):
                chosen.setdefault(kind, address)
        if not chosen:
            raise OSError("没有可探测的单播地址")
        return list(chosen.items())
    finally:
        receiver.close()
        process.join(timeout=0.1)
        if process.is_alive():
            process.terminate()
            process.join(timeout=0.5)
        if process.is_alive():
            process.kill()
            process.join(timeout=0.5)
        process.close()


def validate(targets):
    if not isinstance(targets, list) or len(targets) > 4:
        raise ValueError("最多允许 4 个管理员配置的 TCP 目标")
    for target in targets:
        if not isinstance(target, dict) or set(target) != {"name", "target", "port"}:
            raise ValueError("目标字段无效")
        name, host, port = target["name"], target["target"], target["port"]
        if not isinstance(name, str) or not name.strip() or len(name.encode()) > 128 or any(ord(c) < 32 for c in name):
            raise ValueError("目标名称无效")
        if not isinstance(host, str) or not host or len(host) > 253 or host.startswith("-") or any(c not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.:-" for c in host):
            raise ValueError("目标地址无效")
        if type(port) is not int or not 1 <= port <= 65535:
            raise ValueError("目标端口无效")


def measure(target, ip_version):
    family = {"both": socket.AF_UNSPEC, "ipv4": socket.AF_INET, "ipv6": socket.AF_INET6}[ip_version]
    label = f'{target["name"]} · {target["target"]}:{target["port"]}'
    try:
        addresses = resolve(target["target"], target["port"], family)
    except (OSError, EOFError) as error:
        return label + "\nDNS/地址失败：" + str(error)[:512] + "\n"
    parts = [label + "\n"]
    for kind, address in addresses:
        times = []
        for _ in range(4):
            started = time.monotonic()
            try:
                with socket.socket(kind, socket.SOCK_STREAM) as connection:
                    connection.settimeout(1)
                    connection.connect(address)
                times.append((time.monotonic() - started) * 1000)
            except OSError:
                pass
        latency = f'{sum(times)/len(times):.3f} 毫秒' if times else "未知（连接失败或超时）"
        parts.append(f'目标地址：{address[0]}\n成功 {len(times)} / 4 · 丢失 {100-25*len(times)}% · 平均连接延迟 {latency}\n')
    return "".join(parts)


def write_atomic(path, text):
    temporary = path.with_name(path.name + ".tmp")
    with temporary.open("w", encoding="utf-8") as destination:
        destination.write(text)
        destination.flush()
        os.fsync(destination.fileno())
    temporary.chmod(0o600)
    temporary.replace(path)


def run(workspace, targets_path, ip_version):
    if ip_version not in ("both", "ipv4", "ipv6"):
        raise ValueError("IP 版本无效")
    if targets_path.stat().st_size > 8192 or targets_path.is_symlink():
        raise ValueError("目标文件无效或过大")
    targets = json.loads(targets_path.read_text())
    validate(targets)
    parts = [f'日常网络检查 · {ip_version} · {int(time.time())}\n每个目标每种 IP 版本 4 次 TCP 连接，DNS 最多 2 秒，每连接最多 1 秒，顺序执行。\n']
    if not targets:
        parts.append("尚未配置启用的 TCP 拨测目标，网络质量未知。请在拨测中配置自有或已获同意的目标。\n")
    for target in targets:
        parts.append("\n" + measure(target, ip_version))
    helper_path = pathlib.Path(__file__).with_name('official-ip.py')
    helper_spec = importlib.util.spec_from_file_location('apsp_official_node_ip', helper_path)
    helper = importlib.util.module_from_spec(helper_spec)
    helper_spec.loader.exec_module(helper)
    node_sources = helper.collect(ip_version)
    parts.append(helper.render(node_sources))
    text = "".join(parts)
    write_atomic(workspace / "node-ip-sources.json", json.dumps(node_sources, ensure_ascii=False))
    write_atomic(workspace / "result.txt", text)
    write_atomic(workspace / "section-net_quality.json", json.dumps({
        "name": "net_quality", "text": text, "complete": True,
        "revision": 1, "collected_at": int(time.time()),
    }, ensure_ascii=False))


if __name__ == "__main__":
    try:
        root, targets, version = sys.argv[1:]
        run(pathlib.Path(root), pathlib.Path(targets), version)
    except (ValueError, OSError) as error:
        print("日常检查失败：" + str(error), file=sys.stderr)
        raise SystemExit(1)
