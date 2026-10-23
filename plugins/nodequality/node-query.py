"""Bounded node-origin official IP lookups; credentials never leave local configuration."""
import argparse
import ipaddress
import json
import os
import pathlib
import re
import selectors
import stat
import subprocess
import time
import uuid

CONFIG = pathlib.Path('/etc/apsp/nodequality-providers.json')
CURL = '/usr/bin/curl'
SCHEMA = 'apsp.node-ip-quality.v1'
CONFIG_SCHEMA = 'apsp.node-ip-quality-config.v1'
BODY_LIMIT = 65536
REPORT_LIMIT = 64 * 1024
TOTAL_SECONDS = 75
PROVIDERS = {'ipregistry-node': 'ipregistry-v1', 'dbip-node': 'dbip-v2'}
LABELS = {'ipregistry-node': 'Ipregistry 正式节点接口', 'dbip-node': 'DB-IP 正式节点接口'}
ENDPOINTS = {'ipregistry-node': 'https://api.ipregistry.co', 'dbip-node': 'https://api.db-ip.com/v2'}
PATHS = {
    'ipregistry-node': ('ip', 'type', 'connection.asn', 'connection.organization', 'connection.type',
                        'location.country.code', 'security.is_proxy', 'security.is_tor', 'security.is_vpn',
                        'security.is_abuser', 'security.is_attacker', 'security.is_cloud_provider'),
    'dbip-node': ('ipAddress', 'countryCode', 'countryName', 'asNumber', 'asName', 'isp', 'usageType',
                  'isProxy', 'isCrawler', 'latitude', 'longitude'),
}
BOOLEAN_PATHS = {'security.is_proxy', 'security.is_tor', 'security.is_vpn', 'security.is_abuser',
                 'security.is_attacker', 'security.is_cloud_provider', 'isProxy', 'isCrawler'}
NUMBER_PATHS = {'connection.asn', 'asNumber', 'latitude', 'longitude'}
# These special-use ranges were not classified consistently by the Python
# versions shipped on supported nodes. Keep the new node-query boundary stable
# without changing historical adapters' public-IP interpretation.
SPECIAL_RANGES = tuple(ipaddress.ip_network(value) for value in
                       ('192.88.99.0/24', '2001:2::/48', '3fff::/20'))
REQUEST_ENV_BLOCKED = {'http_proxy', 'https_proxy', 'all_proxy', 'ftp_proxy', 'no_proxy',
                       'curl_ca_bundle', 'ssl_cert_file', 'ssl_cert_dir', 'sslkeylogfile', 'qlogdir'}


class QueryFailure(Exception):
    def __init__(self, kind, message, http_status=None):
        self.kind, self.message, self.http_status = kind, message, http_status
        super().__init__(message)


def decode(content):
    def pairs(items):
        result = {}
        for name, value in items:
            if name in result:
                raise ValueError('重复 JSON 字段')
            result[name] = value
        return result
    def nonfinite(_):
        raise ValueError('非有限 JSON 数值')
    return json.loads(content, object_pairs_hook=pairs, parse_constant=nonfinite)


def public_ip(value):
    address = ipaddress.ip_address(value)
    if (not address.is_global or address.is_multicast or address.is_unspecified
            or any(address in network for network in SPECIAL_RANGES)):
        raise ValueError('查询目标不是公网单播 IP')
    return address


def frozen_ips(workspace, ips_path, deadline):
    if ips_path != workspace / 'node-ips.json':
        raise ValueError('节点查询必须使用私有工作区的固定输入文件')
    parent = os.open(workspace, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        descriptor = os.open('node-ips.json', os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                             dir_fd=parent)
        try:
            before = os.fstat(descriptor)
            if (not stat.S_ISREG(before.st_mode) or before.st_nlink != 1
                    or not 0 < before.st_size <= 2048):
                raise ValueError('冻结节点 IP 必须为有界普通文件')
            content = bytearray()
            while True:
                if time.monotonic() >= deadline:
                    raise ValueError('冻结节点 IP 读取超过总时间限制')
                chunk = os.read(descriptor, 2049 - len(content))
                if not chunk:
                    break
                content.extend(chunk)
                if len(content) > 2048:
                    raise ValueError('冻结节点 IP 文件超过大小限制')
            after = os.fstat(descriptor)
            identity = ('st_dev', 'st_ino', 'st_size', 'st_mtime_ns', 'st_ctime_ns', 'st_nlink')
            if len(content) != before.st_size or any(getattr(before, name) != getattr(after, name)
                                                     for name in identity):
                raise ValueError('冻结节点 IP 文件读取时发生变化')
            return decode(bytes(content))
        finally:
            os.close(descriptor)
    finally:
        os.close(parent)


def load_configuration():
    try:
        for parent in [CONFIG.parent, *CONFIG.parent.parents]:
            info = parent.lstat()
            if not stat.S_ISDIR(info.st_mode) or info.st_uid != 0 or info.st_mode & 0o022:
                raise ValueError('节点凭证目录必须由 root 管理且不可被其他用户写入')
        descriptor = os.open(CONFIG, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        try:
            info = os.fstat(descriptor)
            if not stat.S_ISREG(info.st_mode) or info.st_uid != 0 or stat.S_IMODE(info.st_mode) not in {0o400, 0o600} or info.st_size > 8192:
                raise ValueError('节点凭证文件必须为 root 的 0600/0400 普通小文件')
            with os.fdopen(descriptor, 'rb', closefd=False) as source:
                content = source.read(8193)
            value = decode(content)
        finally:
            os.close(descriptor)
        if not isinstance(value, dict) or set(value) != {'schema', 'providers'} or value['schema'] != CONFIG_SCHEMA:
            raise ValueError('节点凭证配置版本或字段无效')
        configured = value['providers']
        if not isinstance(configured, dict) or set(configured) - set(PROVIDERS):
            raise ValueError('节点凭证配置包含未登记来源')
        result = {}
        for provider in PROVIDERS:
            item = configured.get(provider)
            if item is None:
                result[provider] = (None, '未配置该来源的正式私有凭证，未执行查询，信息未知')
                continue
            if not isinstance(item, dict) or set(item) != {'api_key', 'authorized'} or item['authorized'] is not True:
                result[provider] = (None, '未确认该来源接口的操作授权，未执行查询，信息未知')
                continue
            key = item['api_key']
            if not isinstance(key, str) or not re.fullmatch(r'[A-Za-z0-9_-]{8,256}', key) or key.lower() in {'tryout', 'free', 'your_api_key'}:
                result[provider] = (None, '该来源私有凭证格式无效，未执行查询，信息未知')
                continue
            result[provider] = (key, None)
        return result
    except FileNotFoundError:
        return {provider: (None, '节点尚未配置正式私有凭证，未使用网页临时 key 或公共授权材料，信息未知') for provider in PROVIDERS}
    except (OSError, ValueError, TypeError, UnicodeError):
        # Never echo configuration content, secret paths in URLs, or an exception containing a key.
        return {provider: (None, '节点正式凭证配置无法安全读取或校验，未执行查询，信息未知') for provider in PROVIDERS}


def quote(value):
    if any(ord(character) < 32 for character in value):
        raise ValueError('请求配置含控制字符')
    return '"' + value.replace('\\', '\\\\').replace('"', '\\"') + '"'


def curl_json(provider, key, target, family, deadline):
    if deadline - time.monotonic() <= 0:
        raise QueryFailure('timeout', '节点查询超过总时间限制')
    if provider == 'ipregistry-node':
        url = ENDPOINTS[provider] + ('/' if target == 'self' else '/' + target)
        headers = ['Accept: application/json', 'Authorization: ApiKey ' + key]
    else:
        url = ENDPOINTS[provider] + '/' + key + '/' + target
        headers = ['Accept: application/json']
    seconds = min(6, max(0.01, deadline - time.monotonic()))
    # All secret-bearing request configuration travels on stdin, never argv/environment/files.
    config = ['url = ' + quote(url), 'silent', 'noproxy = "*"', 'proxy = ""',
              'connect-timeout = 3', 'max-time = ' + str(seconds), 'max-filesize = 65536',
              'max-redirs = 0', 'retry = 0', 'write-out = "\\n%{http_code}"',
              'ipv4' if family == 4 else 'ipv6']
    config += ['header = ' + quote(header) for header in headers]
    environment = {name: value for name, value in os.environ.items()
                   if name.lower() not in REQUEST_ENV_BLOCKED
                   and not name.startswith('APSP_')}
    started = time.monotonic()
    try:
        process = subprocess.Popen([CURL, '--disable', '--config', '-'], stdin=subprocess.PIPE,
                                   stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=environment)
    except OSError:
        raise QueryFailure('not_attempted', '节点缺少可执行的系统 curl，未执行查询，信息未知') from None
    collected = bytearray()
    try:
        process.stdin.write(('\n'.join(config) + '\n').encode())
        process.stdin.close()
        with selectors.DefaultSelector() as selector:
            selector.register(process.stdout, selectors.EVENT_READ)
            local_deadline = min(deadline, started + seconds + 0.5)
            while selector.get_map():
                remaining = local_deadline - time.monotonic()
                if remaining <= 0:
                    raise QueryFailure('timeout', '质量查询超时')
                for selected, _ in selector.select(min(remaining, 0.1)):
                    chunk = os.read(selected.fileobj.fileno(), 8192)
                    if not chunk:
                        selector.unregister(selected.fileobj)
                    else:
                        collected.extend(chunk)
                        if len(collected) > BODY_LIMIT + 4:
                            raise QueryFailure('response_limit', '质量查询响应超过 64 KiB')
        code = process.wait(timeout=max(0.01, local_deadline - time.monotonic()))
    except subprocess.TimeoutExpired:
        raise QueryFailure('timeout', '节点正式接口查询超时，信息未知') from None
    except OSError:
        raise QueryFailure('body_error', '节点正式接口请求或响应读写失败，信息未知') from None
    finally:
        if process.stdin is not None and not process.stdin.closed:
            try:
                process.stdin.close()
            except OSError:
                pass
        if process.poll() is None:
            process.kill()
        process.wait()
        process.stdout.close()
    kinds = {6: ('dns', '查询入口的 DNS 解析失败'), 7: ('connect', '无法连接查询入口'),
             28: ('timeout', '质量查询超时'), 35: ('tls', '查询入口的 TLS 握手或证书验证失败'),
             60: ('tls', '查询入口的 TLS 握手或证书验证失败'), 63: ('response_limit', '质量查询响应超过 64 KiB')}
    if code:
        kind, message = kinds.get(code, ('request_error', '节点正式接口请求失败，信息未知'))
        raise QueryFailure(kind, message)
    if len(collected) < 4 or collected[-4] != ord('\n') or not collected[-3:].isdigit():
        raise QueryFailure('body_error', '节点正式接口响应缺少 HTTP 状态')
    status = int(collected[-3:])
    if not 200 <= status < 300:
        kind = {403: 'http_403', 429: 'http_429'}.get(status, 'http_other')
        raise QueryFailure(kind, f'节点正式接口拒绝或返回非成功状态（HTTP {status}），信息未知', status)
    try:
        value = decode(collected[:-4])
    except (ValueError, UnicodeError):
        raise QueryFailure('non_json', '质量查询返回的内容不是有效 JSON') from None
    valid = isinstance(value, dict)
    if valid:
        valid = (('success' not in value or value['success'] is True)
                 and ('status' not in value or isinstance(value['status'], str) and value['status'].lower() in {'success', 'succeeded', 'ok'})
                 and (value.get('error') is None or value.get('error') is False)
                 and ('errors' not in value or value['errors'] is None or value['errors'] == [] or value['errors'] == {})
                 and 'errorCode' not in value and 'code' not in value)
    if not valid:
        raise QueryFailure('schema_mismatch', '节点正式接口未确认成功，信息未知')
    return value


def identity(provider, value, family):
    name = 'ip' if provider == 'ipregistry-node' else 'ipAddress'
    try:
        address = public_ip(value[name])
    except (KeyError, TypeError, ValueError):
        raise QueryFailure('schema_mismatch', '正式接口没有确认有效公网 IP，信息未知') from None
    if address.version != family or provider == 'ipregistry-node' and value.get('type') != f'IPv{family}':
        raise QueryFailure('schema_mismatch', '正式接口响应 IP 版本与节点连接版本不符，信息未知')
    return address


def whitelisted_data(provider, value, key=None):
    output = {}
    for path in PATHS[provider]:
        parts, cursor = path.split('.'), value
        for part in parts:
            if not isinstance(cursor, dict) or part not in cursor:
                break
            cursor = cursor[part]
        else:
            # Missing/null optional fields remain unknown; provided fields must match official types.
            if cursor is None:
                continue
            correct = (isinstance(cursor, bool) if path in BOOLEAN_PATHS else
                       isinstance(cursor, (int, float)) and not isinstance(cursor, bool) if path in NUMBER_PATHS else
                       isinstance(cursor, str))
            if not correct:
                raise QueryFailure('schema_mismatch', '正式接口已提供字段的类型与登记契约不符，信息未知')
            if isinstance(cursor, (str, bool, int, float)):
                if isinstance(cursor, str) and len(cursor.encode()) > 512:
                    continue
                if key and key in str(cursor):
                    continue
                destination = output
                for part in parts[:-1]:
                    destination = destination.setdefault(part, {})
                destination[parts[-1]] = cursor
    return output


def atomic(path, data):
    if len(data) > REPORT_LIMIT or path.is_symlink():
        raise ValueError('节点报告路径无效或超过大小限制')
    temporary = path.with_name(path.name + '.pending')
    descriptor = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        with os.fdopen(descriptor, 'wb', closefd=False) as output:
            output.write(data); output.flush(); os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        os.close(descriptor)
        temporary.unlink(missing_ok=True)


def encode(value):
    return json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(',', ':')).encode()


def run(workspace, ips_path, version, job_id):
    started_at, deadline = int(time.time()), time.monotonic() + TOTAL_SECONDS
    workspace = workspace.resolve(strict=True)
    if str(uuid.UUID(job_id)) != job_id or version not in {'both', 'ipv4', 'ipv6'}:
        raise ValueError('节点查询任务编号或 IP 版本无效')
    ips = frozen_ips(workspace, ips_path, deadline)
    if not isinstance(ips, list) or not 1 <= len(ips) <= 8 or len(set(ips)) != len(ips):
        raise ValueError('节点查询需要 1–8 个不同的冻结公网 IP')
    addresses = {str(public_ip(value)): public_ip(value) for value in ips}
    if list(addresses) != ips:
        raise ValueError('冻结公网 IP 必须使用标准表示')
    families = [family for family in (4, 6) if version == 'both' or version == f'ipv{family}']
    configuration = load_configuration()
    results = []
    for provider, database in PROVIDERS.items():
        for target in ips:
            _, reason = configuration[provider]
            results.append({'provider': provider, 'database': database, 'target_ip': target, 'execution': 'node',
                            'source': ENDPOINTS[provider], 'available': configuration[provider][0] is not None,
                            'observed_ip': None, 'attempted_at': None, 'elapsed_ms': None, 'data': None,
                            'error': {'kind': 'not_attempted', 'message': reason or '该来源尚未完成节点出口确认，信息未知', 'http_status': None}})
    report = {'schema': SCHEMA, 'job_id': job_id, 'execution': 'node', 'ip_version': version,
              'started_at': started_at, 'finished_at': started_at, 'ips': ips, 'results': results,
              'streaming': {'execution': 'node', 'status': 'unknown', 'reason': 'Disney+、YouTube Premium、ChatGPT 没有配置可验证的正式授权认证适配；未使用公共 cookies 或固定授权材料，未执行这些检查。'}}
    revision = 0

    def publish(complete):
        nonlocal revision
        revision += 1
        report['finished_at'] = int(time.time())
        atomic(workspace / 'section-ip_quality.json', encode({'name': 'ip_quality', 'text': encode(report).decode(),
               'complete': complete, 'revision': revision, 'collected_at': report['finished_at']}))

    publish(False)
    for provider in PROVIDERS:
        key, reason = configuration[provider]
        if not key:
            continue
        for family in families:
            selected = [row for row in results if row['provider'] == provider and addresses[row['target_ip']].version == family]
            if not selected:
                continue
            attempt, monotonic = int(time.time()), time.monotonic()
            try:
                origin = identity(provider, curl_json(provider, key, 'self', family, deadline), family)
                for row in selected:
                    row['observed_ip'] = str(origin)
                    if addresses[row['target_ip']] != origin:
                        row['error'] = {'kind': 'schema_mismatch', 'message': '冻结目标不是本次接口观察到的节点真实出口，未查询该目标，信息未知', 'http_status': None}
                        continue
                    value = curl_json(provider, key, str(origin), family, deadline)
                    if identity(provider, value, family) != origin:
                        raise QueryFailure('schema_mismatch', '正式查询结果 IP 与当次节点真实出口不一致，信息未知')
                    row['data'], row['error'] = whitelisted_data(provider, value, key), None
            except QueryFailure as failure:
                for row in selected:
                    row['data'] = None
                    row['error'] = {'kind': failure.kind, 'message': failure.message, 'http_status': failure.http_status}
            for row in selected:
                row['attempted_at'], row['elapsed_ms'] = attempt, min(75000, int((time.monotonic() - monotonic) * 1000))
            publish(False)
    publish(True)
    lines = ['节点正式 IP 查询 · 实际执行位置：Agent 节点', '凭证仅从节点 root 私有配置读取；未使用网页 key、公共 cookies 或公共备用凭证。']
    for row in results:
        lines += [f"{LABELS[row['provider']]} · 目标 {row['target_ip']} · 节点观察出口 {row['observed_ip'] or '未知'}",
                  row['error']['message'] if row['error'] else '正式接口已返回当前节点出口的数据，字段由面板按声明类型确认。']
    lines += [report['streaming']['reason']]
    atomic(workspace / 'result.txt', ('\n'.join(lines) + '\n').encode())


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--workspace', type=pathlib.Path, required=True)
    parser.add_argument('--ips-file', type=pathlib.Path, required=True)
    parser.add_argument('--ip-version', choices=['both', 'ipv4', 'ipv6'], required=True)
    parser.add_argument('--job-id', required=True)
    arguments = parser.parse_args()
    try:
        run(arguments.workspace, arguments.ips_file, arguments.ip_version, arguments.job_id)
    except (ValueError, OSError, TypeError):
        print('节点正式查询未完成：私有输入或报告写入失败；已保存章节保留。', file=__import__('sys').stderr)
        raise SystemExit(1)
