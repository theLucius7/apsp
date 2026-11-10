#!/usr/bin/env python3
"""Query operator-configured official APIs from the node, without borrowed keys."""
import http.client
import ipaddress
import json
import multiprocessing
import os
from pathlib import Path
import re
import socket
import ssl
import stat
import time
import urllib.parse

CONFIG = Path('/etc/apsp/node-ip-providers.json')
MAX_CONFIG = 8192
MAX_RESPONSE = 256 * 1024
QUERY_SECONDS = 3
TOTAL_SECONDS = 14
PROVIDERS = {
    'ipregistry': ('ipregistry-official-node', 'Ipregistry 正式接口', 'api.ipregistry.co'),
    'dbip': ('dbip-official-node', 'DB-IP 正式接口', 'api.db-ip.com'),
}
KEY = re.compile(r'[A-Za-z0-9_-]{8,512}\Z')
MEDIA = ('Disney+', 'YouTube Premium', 'ChatGPT')


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError('duplicate response field')
        result[key] = value
    return result


def decode(content):
    return json.loads(content, object_pairs_hook=unique_object,
                      parse_constant=lambda _: (_ for _ in ()).throw(ValueError('non-finite JSON')))


def read_config(path=CONFIG, owner=0):
    """Inspect the actual open FD, not a path later reopened for its contents."""
    directory_fd = None
    try:
        directory_fd = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        directory = os.fstat(directory_fd)
        if directory.st_uid != owner or directory.st_mode & 0o022:
            raise ValueError('untrusted credential directory')
        descriptor = os.open(path.name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK,
                             dir_fd=directory_fd)
        with os.fdopen(descriptor, 'rb') as stream:
            metadata = os.fstat(stream.fileno())
            if (not stat.S_ISREG(metadata.st_mode) or metadata.st_uid != owner
                    or stat.S_IMODE(metadata.st_mode) != 0o600
                    or metadata.st_nlink != 1 or metadata.st_size > MAX_CONFIG):
                raise ValueError('untrusted credential file')
            content = stream.read(MAX_CONFIG + 1)
        if len(content) > MAX_CONFIG:
            raise ValueError('credential file too large')
        config = decode(content)
        if (not isinstance(config, dict) or set(config) != {'schema', 'providers'}
                or type(config['schema']) is not int or config['schema'] != 1
                or not isinstance(config['providers'], dict)
                or not set(config['providers']) <= set(PROVIDERS)):
            raise ValueError('invalid credential configuration')
        return config['providers']
    finally:
        if directory_fd is not None:
            os.close(directory_fd)


def credential(config, provider):
    row = config.get(provider)
    if row is None or (isinstance(row, dict) and row.get('enabled') is False):
        return None
    if (not isinstance(row, dict)
            or set(row) != {'enabled', 'operator_owned_credentials', 'api_key'}
            or row['enabled'] is not True or row['operator_owned_credentials'] is not True
            or not isinstance(row['api_key'], str) or not KEY.fullmatch(row['api_key'])
            or row['api_key'].lower() == 'free'):
        raise ValueError('invalid provider credential configuration')
    return row['api_key']


def result(provider, family, kind, attempted, fields=None, address=None, status=None):
    reasons = {
        'credential_not_configured': '未配置节点操作者的正式凭据，未查询，信息未知',
        'credential_invalid': '节点凭据配置或私有文件权限无效，未查询，信息未知',
        'forbidden': '正式接口拒绝访问，信息未知；未重试或使用备用授权材料',
        'rate_limited': '正式接口限流，信息未知；未重试',
        'timeout': '节点正式接口查询超过固定截止，信息未知',
        'transport_error': '节点正式接口连接或 TLS 失败，信息未知',
        'http_error': '正式接口返回非成功状态，信息未知',
        'redirect_rejected': '正式接口重定向被拒绝，凭据未转发',
        'response_too_large': '正式接口响应超过字节限制，信息未知',
        'schema_mismatch': '正式接口未确认公网地址、IP 版本或有效字段，信息未知',
        'success': '由此节点直接调用正式接口，保留上游字段；不代表流媒体解锁',
    }
    identity, label, host = PROVIDERS[provider]
    return {
        'provider': identity, 'database': provider + '-official', 'label': label,
        'origin': 'https://' + host, 'execution': 'node_self', 'ip_version': family,
        'status': 'success' if kind == 'success' else ('failed' if attempted else 'not_attempted'),
        'error_kind': None if kind == 'success' else kind, 'attempted': attempted,
        'reason': reasons[kind], 'address': address, 'fields': fields or {},
        'http_status': status,
    }


def text(value):
    return (isinstance(value, str) and 0 < len(value.encode()) <= 256
            and not any(ord(char) < 32 or ord(char) == 127 for char in value))


def confirmed(value):
    if not isinstance(value, dict):
        return False
    if 'success' in value and value['success'] is not True:
        return False
    if ('status' in value and (not isinstance(value['status'], str)
                              or value['status'].lower() not in ('success', 'succeeded', 'ok'))):
        return False
    for key in ('error', 'errorCode', 'errors'):
        if key in value and value[key] is not None and value[key] is not False and value[key] not in ('', [], {}):
            return False
    return True


def parse_response(provider, value, family):
    if not confirmed(value):
        raise ValueError('response did not confirm success')
    raw_address = value.get('ip' if provider == 'ipregistry' else 'ipAddress')
    if not isinstance(raw_address, str):
        raise ValueError('address absent')
    address = ipaddress.ip_address(raw_address)
    if (address.version != family or not address.is_global or address.is_multicast
            or address.is_unspecified or (address.version == 6 and address.ipv4_mapped)):
        raise ValueError('address or family mismatch')
    fields = {}
    if provider == 'ipregistry':
        if value.get('type') != 'IPv' + str(family):
            raise ValueError('provider address type mismatch')
        mappings = (
            ('connection', 'type', '连接用途'), ('company', 'type', '组织类型'),
            ('company', 'name', '组织'), ('connection', 'organization', '网络组织'),
        )
        for container, key, label in mappings:
            group = value.get(container)
            if group is not None and not isinstance(group, dict):
                raise ValueError('invalid object')
            item = (group or {}).get(key)
            if item is not None:
                if not text(item):
                    raise ValueError('invalid text field')
                if key == 'type':
                    kinds = ('business', 'education', 'government', 'hosting', 'isp')
                    if item not in kinds and not (container == 'connection' and item == 'inactive'):
                        raise ValueError('invalid usage type')
                fields[label] = item
        security = value.get('security')
        if security is not None and not isinstance(security, dict):
            raise ValueError('invalid security object')
        for key, label in (('is_proxy', '代理'), ('is_tor', 'Tor'), ('is_vpn', 'VPN'),
                           ('is_abuser', '滥用'), ('is_threat', '威胁')):
            item = (security or {}).get(key)
            if item is not None:
                if type(item) is not bool:
                    raise ValueError('invalid security boolean')
                fields[label] = item
    else:
        for key, label in (('countryCode', '国家代码'), ('countryName', '国家或地区'),
                           ('stateProv', '地区'), ('city', '城市'), ('usageType', '用途类型'),
                           ('organization', '组织'), ('threatLevel', '威胁等级')):
            item = value.get(key)
            if item is not None:
                if not text(item):
                    raise ValueError('invalid text field')
                if key == 'countryCode' and not re.fullmatch(r'[A-Z]{2}', item):
                    raise ValueError('invalid country code')
                if key == 'usageType' and item not in ('hosting', 'corporate', 'consumer', 'reserved'):
                    raise ValueError('invalid usage type')
                if key == 'threatLevel' and item not in ('low', 'medium', 'high'):
                    raise ValueError('invalid threat level')
                fields[label] = item
        for key, label in (('isProxy', '代理'), ('isCrawler', '爬虫')):
            item = value.get(key)
            if item is not None:
                if type(item) is not bool:
                    raise ValueError('invalid boolean')
                fields[label] = item
        number = value.get('asNumber')
        if number is not None:
            if type(number) is not int or not 1 <= number <= 4294967295:
                raise ValueError('invalid ASN')
            fields['ASN'] = number
    if not fields:
        raise ValueError('no trustworthy provider fields')
    return str(address), fields


def response_failure(provider, value):
    # DB-IP documents an error object separately from successful field objects.
    # Never expose its arbitrary message, which may include the request URL/key.
    if provider == 'dbip' and isinstance(value, dict):
        code = value.get('errorCode')
        if code in ('INVALID_KEY', 'EXPIRED', 'RESTRICTED', 'TEMPORARY_BLOCKED'):
            return 'forbidden'
        if code == 'OVER_QUERY_LIMIT':
            return 'rate_limited'
    return None


def request_child(provider, key, family, sender):
    """A disposable process gives DNS, TLS and all reads one hard parent deadline."""
    connection = None
    try:
        real_getaddrinfo = socket.getaddrinfo

        def family_addresses(host, port, *args, **kwargs):
            requested = socket.AF_INET if family == 4 else socket.AF_INET6
            values = real_getaddrinfo(host, port, requested, socket.SOCK_STREAM)
            return [row for row in values if row[0] == requested][:4]

        socket.getaddrinfo = family_addresses
        host = PROVIDERS[provider][2]
        path = ('/?' + urllib.parse.urlencode({'key': key}) if provider == 'ipregistry'
                else '/v2/' + urllib.parse.quote(key, safe='') + '/self')
        # http.client ignores proxy environment variables and never follows redirects.
        connection = http.client.HTTPSConnection(host, timeout=1, context=ssl.create_default_context())
        connection.request('GET', path, headers={'Accept': 'application/json',
                                                'User-Agent': 'ApspNodeIp/1', 'Connection': 'close'})
        response = connection.getresponse()
        status = response.status
        if status != 200:
            kind = ('forbidden' if status in (401, 403) else 'rate_limited' if status == 429
                    else 'redirect_rejected' if 300 <= status < 400 else 'http_error')
            sender.send(result(provider, family, kind, True, status=status))
            return
        content = response.read(MAX_RESPONSE + 1)
        if len(content) > MAX_RESPONSE:
            sender.send(result(provider, family, 'response_too_large', True, status=status))
            return
        value = decode(content)
        kind = response_failure(provider, value)
        if kind is not None:
            sender.send(result(provider, family, kind, True, status=status))
            return
        address, fields = parse_response(provider, value, family)
        # Even documented text fields may be reflected by a broken service.
        if key in json.dumps(fields, ensure_ascii=False):
            raise ValueError('reflected credential')
        sender.send(result(provider, family, 'success', True, fields, address, status))
    except (ValueError, UnicodeError, RecursionError):
        sender.send(result(provider, family, 'schema_mismatch', True))
    except (TimeoutError, socket.timeout):
        sender.send(result(provider, family, 'timeout', True))
    except (OSError, http.client.HTTPException):
        sender.send(result(provider, family, 'transport_error', True))
    finally:
        if connection is not None:
            connection.close()
        sender.close()


def query(provider, key, family, deadline):
    remaining = min(QUERY_SECONDS, deadline - time.monotonic())
    if remaining <= 0:
        return result(provider, family, 'timeout', False)
    context = multiprocessing.get_context('fork')
    receiver, sender = context.Pipe(duplex=False)
    process = context.Process(target=request_child, args=(provider, key, family, sender))
    try:
        process.start()
        sender.close()
        if not receiver.poll(remaining):
            return result(provider, family, 'timeout', True)
        return receiver.recv()
    except (OSError, EOFError):
        return result(provider, family, 'transport_error', True)
    finally:
        sender.close()
        receiver.close()
        if process.pid is not None:
            process.join(timeout=0.05)
            if process.is_alive():
                process.terminate()
                process.join(timeout=0.2)
            if process.is_alive():
                process.kill()
                process.join(timeout=0.2)
            process.close()


def collect(ip_version):
    families = {'both': (4, 6), 'ipv4': (4,), 'ipv6': (6,)}[ip_version]
    deadline = time.monotonic() + TOTAL_SECONDS
    configuration_error = False
    try:
        config = read_config()
    except FileNotFoundError:
        config = {}
    except (OSError, ValueError, UnicodeError, RecursionError):
        config = {}
        configuration_error = True
    results = []
    for provider in PROVIDERS:
        try:
            key = credential(config, provider)
            kind = 'credential_invalid' if configuration_error else 'credential_not_configured'
        except ValueError:
            key, kind = None, 'credential_invalid'
        for family in families:
            results.append(query(provider, key, family, deadline) if key
                           else result(provider, family, kind, False))
    return results


def render(results):
    parts = ['\n节点正式 IP 自查（独立于面板查询缓存）\n',
             '固定官方 HTTPS self 接口；每源每种 IP 版本最多 3 秒，无重试或代理继承。\n']
    for row in results:
        parts.append(f'{row["label"]} · IPv{row["ip_version"]} · {row["origin"]} · 节点出口\n')
        parts.append(row['reason'] + '\n')
        if row['status'] == 'success':
            parts.append('本次接口观察的节点出口：' + row['address'] + '\n')
            for label, item in row['fields'].items():
                parts.append(label + '：' + ('是' if item is True else '否' if item is False else str(item)) + '\n')
    parts.append('最近成功结果保留在各次历史报告中；本次失败不改写旧报告。\n')
    for label in MEDIA:
        parts.append(label + '：未配置经授权的节点认证适配；未使用公共 cookies 或固定材料，未尝试，信息未知。\n')
    return ''.join(parts)
