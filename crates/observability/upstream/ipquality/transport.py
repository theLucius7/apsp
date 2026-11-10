#!/usr/bin/env python3
"""Bound and classify the fixed node profile's requests without credentials."""
import contextlib
import fcntl
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import selectors
import stat
import subprocess
import sys
import tempfile
import time
from urllib.parse import parse_qs, unquote, urljoin, urlsplit

MAX_BODY = 2 * 1024 * 1024
MAX_JSON_BODY = 65536
MAX_ATTEMPTS = 64
MAX_RECEIPTS = 98304
MAX_SNAPSHOT = 24576
MAX_URL = 512
TIMEOUT = 10
ERROR_KINDS = {'dns', 'connect', 'tls', 'timeout', 'http_403', 'http_429',
               'http_other', 'non_json', 'schema_mismatch', 'response_limit',
               'request_error', 'not_attempted'}
PAIRINGS = {
    'check-place-aggregator': {'MaxMind', 'SCAMALYTICS', 'ipapi', 'AbuseIPDB', 'IP2LOCATION', 'ipdata', 'IPQS'},
    'ipinfo-public-widget': {'IPinfo'}, 'netflix-public-pages': {'Netflix'},
    'youtube-public-page': {'Youtube'}, 'tiktok-public-page': {'TikTok'},
    'primevideo-public-page': {'AmazonPrimeVideo'}, 'reddit-public-endpoint': {'Reddit'},
    'egress-discovery': {'egress'}, 'ipregistry-not-configured': {'ipregistry'},
    'dbip-not-configured': {'DBIP'}, 'disney-not-configured': {'DisneyPlus'},
    'openai-not-configured': {'OpenAI'}, 'smtp-disabled': {'SMTP'}, 'dnsbl-disabled': {'DNSBL'},
}
DISABLED = {provider for provider in PAIRINGS if provider.endswith('-not-configured') or provider.endswith('-disabled')}
DB_NAMES = {'scamalytics': 'SCAMALYTICS', 'ipapi': 'ipapi', 'abuseipdb': 'AbuseIPDB',
            'ip2location': 'IP2LOCATION', 'ipdata': 'ipdata', 'ipqualityscore': 'IPQS'}
MARKER = b'\nAPSP_IPQUALITY_TRANSPORT:'
WRITE_OUT = '\nAPSP_IPQUALITY_TRANSPORT:%{http_code}|%{time_total}|%{content_type}|%{redirect_url}\n'


def require(condition, message):
    if not condition:
        raise ValueError(message)


def unique(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, 'duplicate JSON field')
        result[key] = value
    return result


def decode(content):
    return json.loads(content, object_pairs_hook=unique,
                      parse_constant=lambda _value: (_ for _ in ()).throw(ValueError('nonfinite JSON number')))


def canonical_ip(value, family=None):
    require(isinstance(value, str) and len(value) <= 64, 'bounded public IP required')
    address = ipaddress.ip_address(value.strip())
    if address.version == 4:
        a, b, c, _ = address.packed
        require(not (a in (0, 10, 127) or 224 <= a <= 255
                     or (a == 100 and 64 <= b <= 127) or (a == 169 and b == 254)
                     or (a == 172 and 16 <= b <= 31)
                     or (a == 192 and ((b == 0 and c in (0, 2)) or b == 168))
                     or (a == 198 and (b in (18, 19) or (b == 51 and c == 100)))
                     or (a == 203 and b == 0 and c == 113)), 'node egress is private or reserved')
    else:
        require(address in ipaddress.ip_network('2000::/3') and address not in ipaddress.ip_network('2001:db8::/32'),
                'node egress is not global unicast IPv6')
    require(family is None or address.version == int(family), 'IP family mismatch')
    return str(address)


def ordinary(path, limit):
    descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    with os.fdopen(descriptor, 'rb') as stream:
        metadata = os.fstat(stream.fileno())
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.getuid()
                and metadata.st_size <= limit and metadata.st_nlink == 1, 'bounded owned ordinary input required')
        result = stream.read(limit + 1)
    require(len(result) <= limit, 'ordinary input byte limit exceeded')
    return result


def atomic(path, content, limit):
    require(len(content) <= limit, 'atomic output byte limit exceeded')
    parent = path.parent
    require(parent.is_dir() and not parent.is_symlink(), 'private ordinary workspace required')
    metadata = parent.stat()
    require(metadata.st_uid == os.getuid() and metadata.st_mode & 0o022 == 0,
            'workspace must be owned and not writable by other accounts')
    if path.exists() or path.is_symlink():
        ordinary(path, limit)
    descriptor, name = tempfile.mkstemp(prefix='.' + path.name + '-', dir=parent)
    try:
        with os.fdopen(descriptor, 'wb') as stream:
            os.fchmod(stream.fileno(), 0o600)
            stream.write(content)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
        directory = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        with contextlib.suppress(FileNotFoundError):
            os.unlink(name)


def receipts_path():
    value = os.environ.get('APSP_IPQUALITY_ATTEMPTS', '')
    require(value.startswith('/') and len(value) <= 4096, 'absolute receipt workspace path required')
    return Path(value)


def load_receipts(path):
    if not path.exists() and not path.is_symlink():
        return []
    content = ordinary(path, MAX_RECEIPTS)
    records = [decode(line) for line in content.splitlines() if line]
    require(len(records) <= MAX_ATTEMPTS and all(isinstance(row, dict) and row.get('seq') == index + 1
            for index, row in enumerate(records)), 'receipt sequence or count is invalid')
    return records


def write_receipts(path, records):
    content = b''.join((json.dumps(record, sort_keys=True, ensure_ascii=False, separators=(',', ':')) + '\n').encode()
                       for record in records)
    atomic(path, content, MAX_RECEIPTS)


@contextlib.contextmanager
def locked_receipts():
    path = receipts_path()
    lock = path.with_name(path.name + '.lock')
    descriptor = os.open(lock, os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW | os.O_NONBLOCK, 0o600)
    try:
        metadata = os.fstat(descriptor)
        require(stat.S_ISREG(metadata.st_mode) and metadata.st_uid == os.getuid() and metadata.st_nlink == 1,
                'ordinary owned receipt lock required')
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield path, load_receipts(path)
    finally:
        os.close(descriptor)


def append_receipt(row):
    require(row['provider'] in PAIRINGS and row['dataset'] in PAIRINGS[row['provider']],
            'unknown fixed provider or dataset')
    with locked_receipts() as (path, records):
        require(len(records) < MAX_ATTEMPTS, 'per-job request limit reached')
        row = dict(row, seq=len(records) + 1)
        records.append(row)
        write_receipts(path, records)


def target_ip():
    value = os.environ.get('APSP_IPQUALITY_TARGET_IP', '')
    return canonical_ip(value, os.environ['APSP_IPQUALITY_FAMILY']) if value else None


def identify(url, expected_ip, *, redirect_from=None):
    require(isinstance(url, str) and 0 < len(url) <= MAX_URL and not any(ord(c) < 32 or ord(c) == 127 for c in url),
            'bounded HTTPS URL required')
    parsed = urlsplit(url)
    require(parsed.scheme == 'https' and parsed.hostname and parsed.username is None and parsed.password is None
            and parsed.port in (None, 443) and not parsed.fragment, 'HTTPS without credentials is required')
    host, path = parsed.hostname.lower(), unquote(parsed.path)
    query = parse_qs(parsed.query, strict_parsing=True) if parsed.query else {}
    if redirect_from is not None:
        original = urlsplit(redirect_from)
        require(host == original.hostname and not query, 'redirect outside the same fixed service is forbidden')
        if host == 'www.netflix.com':
            require(re.fullmatch(r'(?:/[a-z]{2}(?:-[a-z]{2})?)?/title/(81280792|70143836)', path),
                    'redirect outside fixed Netflix titles is forbidden')
            return 'netflix-public-pages', 'Netflix'
        require(path == original.path, 'redirect outside fixed public endpoint is forbidden')
    if host in ('api64.ipify.org', 'ident.me'):
        require(path in ('', '/') and not query, 'discovery URL differs from fixed endpoint')
        return 'egress-discovery', 'egress'
    if host == 'ipinfo.check.place':
        require(expected_ip is not None and path == '/' + expected_ip, 'query target differs from node egress')
        if query == {'lang': ['en']}:
            return 'check-place-aggregator', 'MaxMind'
        require(set(query) == {'db'} and len(query['db']) == 1 and query['db'][0] in DB_NAMES,
                'unknown aggregated dataset')
        return 'check-place-aggregator', DB_NAMES[query['db'][0]]
    if host == 'ipinfo.io':
        require(expected_ip is not None and path == '/widget/demo/' + expected_ip and not query,
                'public widget target differs from node egress')
        return 'ipinfo-public-widget', 'IPinfo'
    fixed = {
        ('www.tiktok.com', '/'): ('tiktok-public-page', 'TikTok'),
        ('www.primevideo.com', '/'): ('primevideo-public-page', 'AmazonPrimeVideo'),
        ('www.youtube.com', '/premium'): ('youtube-public-page', 'Youtube'),
        ('www.reddit.com', '/svc/shreddit/reddit-chat'): ('reddit-public-endpoint', 'Reddit'),
    }
    if host == 'www.netflix.com' and re.fullmatch(r'(?:/[a-z]{2}(?:-[a-z]{2})?)?/title/(81280792|70143836)', path) and not query:
        require(expected_ip is not None, 'node egress must be observed before media checks')
        return 'netflix-public-pages', 'Netflix'
    require((host, path) in fixed and not query and expected_ip is not None, 'unknown fixed public endpoint')
    return fixed[(host, path)]


def parse_arguments(arguments):
    url = None
    write_out = ''
    output = None
    follow = False
    family = os.environ.get('APSP_IPQUALITY_FAMILY')
    require(family in ('4', '6'), 'one fixed IP family is required')
    native = ['-' + family]
    index = 0
    while index < len(arguments):
        value = arguments[index]
        if value.startswith('https://'):
            require(url is None, 'one URL per source request is required')
            url = value
        elif value in ('-H', '--header'):
            index += 1
            require(index < len(arguments), 'header value is missing')
            header = arguments[index]
            name = header.split(':', 1)[0].lower()
            require(name in ('accept', 'accept-language') and not any(ord(c) < 32 or ord(c) == 127 for c in header)
                    and len(header) <= 256, 'credentials, cookies and identity headers are forbidden')
            native.extend(['--header', header])
        elif value in ('-w', '--write-out'):
            index += 1
            require(index < len(arguments) and len(arguments[index]) <= 256, 'bounded write-out required')
            write_out = arguments[index]
            require(re.sub(r'%\{(?:http_code|time_total|url_effective)\}', '', write_out).find('%') < 0,
                    'unknown write-out variable')
        elif value in ('-o', '--output'):
            index += 1
            require(index < len(arguments) and arguments[index] == '/dev/null', 'external output files are forbidden')
            output = '/dev/null'
        elif value in ('-m', '--max-time', '--max-filesize', '--connect-timeout', '-X', '--request'):
            index += 1
            require(index < len(arguments), 'option value is missing')
            argument = arguments[index]
            if value in ('-X', '--request'):
                require(argument == 'GET', 'only public GET requests are allowed')
            else:
                require(re.fullmatch(r'[0-9]+(?:\.[0-9]+)?', argument), 'numeric request bound required')
                limit = MAX_BODY if value == '--max-filesize' else TIMEOUT
                require(0 < float(argument) <= limit, 'request bound exceeds fixed maximum')
        elif value in ('--compressed', '--tlsv1.3', '--tlsv1.2'):
            native.append(value)
        elif value in ('-q', '--disable', '--silent', '--show-error', '--fail'):
            pass
        elif value == '--location':
            follow = True
        elif re.fullmatch(r'-[sSfL46]+', value):
            require(not any(digit in value for digit in ('4', '6') if digit != family), 'request IP family mismatch')
            follow = follow or 'L' in value
        else:
            raise ValueError('unknown curl option; retry, insecure, proxy and identity overrides are forbidden')
        index += 1
    require(url is not None, 'fixed HTTPS request URL required')
    return url, native, write_out, output, follow


def capture(command, deadline):
    process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                               close_fds=True, env={'PATH': '/usr/bin:/bin', 'LANG': 'C.UTF-8', 'HOME': '/nonexistent'})
    output, errors = bytearray(), bytearray()
    selector = selectors.DefaultSelector()
    selector.register(process.stdout, selectors.EVENT_READ, output)
    selector.register(process.stderr, selectors.EVENT_READ, errors)
    category = None
    try:
        while selector.get_map():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                category = 'timeout'
                break
            for key, _ in selector.select(min(remaining, 0.25)):
                content = os.read(key.fd, 65536)
                if not content:
                    selector.unregister(key.fileobj)
                    continue
                destination = key.data
                limit = MAX_BODY if destination is output else 16384
                if len(destination) + len(content) > limit:
                    # Preserve only bytes actually retained within the reader
                    # budget. A truncated prefix never proves a complete body.
                    destination.extend(content[:max(0, limit - len(destination))])
                    category = 'response_limit' if destination is output else 'request_error'
                    break
                destination.extend(content)
            if category:
                break
        if category:
            process.kill()
        try:
            code = process.wait(timeout=max(0.1, deadline - time.monotonic()))
        except subprocess.TimeoutExpired:
            process.kill()
            code = process.wait(timeout=1)
            category = category or 'timeout'
    finally:
        selector.close()
        process.stdout.close()
        process.stderr.close()
        if process.poll() is None:
            process.kill()
            process.wait(timeout=1)
    return bytes(output), code, category


def classify_exit(code, status, category):
    if status == 403:
        return 'http_403'
    if status == 429:
        return 'http_429'
    if category:
        return category
    if code == 6:
        return 'dns'
    if code == 7:
        return 'connect'
    if code == 28:
        return 'timeout'
    if code in (35, 51, 58, 60, 77, 83, 90, 91):
        return 'tls'
    if code == 63:
        return 'response_limit'
    if code:
        return 'request_error'
    if status != 200:
        return 'http_other'
    return None


def getpath(data, path):
    for part in path.split('.'):
        if not isinstance(data, dict) or part not in data:
            return None
        data = data[part]
    return data


def finite_integer(value):
    return type(value) is int and 0 <= value <= 100


def coordinates_safe(data, dataset):
    if dataset == 'MaxMind':
        for path, limit in (('City.Latitude', 90), ('City.Longitude', 180), ('City.AccuracyRadius', 40000)):
            value = getpath(data, path)
            if value is not None and (type(value) not in (int, float) or not math.isfinite(value) or abs(value) > limit):
                return False
    if dataset == 'IPinfo':
        value = getpath(data, 'data.loc')
        if value is not None:
            if not isinstance(value, str) or not re.fullmatch(r'-?[0-9]+(?:\.[0-9]+)?,-?[0-9]+(?:\.[0-9]+)?', value):
                return False
            latitude, longitude = map(float, value.split(','))
            if abs(latitude) > 90 or abs(longitude) > 180:
                return False
    return True


def strings_bounded(value, depth=0):
    if depth > 64:
        return False
    if isinstance(value, str):
        return len(value.encode()) <= 512 and not any(ord(c) < 32 or ord(c) == 127 for c in value)
    if isinstance(value, list):
        return len(value) <= 256 and all(strings_bounded(item, depth + 1) for item in value)
    if isinstance(value, dict):
        return len(value) <= 256 and all(isinstance(key, str) and len(key.encode()) <= 256
                                         and not any(ord(c) < 32 or ord(c) == 127 for c in key)
                                         and strings_bounded(item, depth + 1) for key, item in value.items())
    return value is None or type(value) in (int, bool) or (type(value) is float and math.isfinite(value))


def boolean_fields_safe(data, dataset):
    fields = {
        'IPinfo': ('data.privacy.proxy', 'data.privacy.tor', 'data.privacy.vpn', 'data.privacy.hosting'),
        'SCAMALYTICS': ('external_datasources.firehol.is_proxy', 'external_datasources.x4bnet.is_tor',
                       'scamalytics.scamalytics_proxy.is_vpn', 'scamalytics.scamalytics_proxy.is_datacenter',
                       'scamalytics.is_blacklisted_external', 'external_datasources.x4bnet.is_blacklisted_spambot',
                       'external_datasources.x4bnet.is_bot_operamini', 'external_datasources.x4bnet.is_bot_semrush'),
        'ipapi': ('is_proxy', 'is_tor', 'is_vpn', 'is_datacenter', 'is_abuser', 'is_crawler'),
        'IP2LOCATION': ('is_proxy', 'proxy.is_public_proxy', 'proxy.is_web_proxy', 'proxy.is_tor', 'proxy.is_vpn',
                       'proxy.is_data_center', 'proxy.is_spammer', 'proxy.is_web_crawler', 'proxy.is_scanner', 'proxy.is_botnet'),
        'ipdata': ('threat.is_proxy', 'threat.is_tor', 'threat.is_datacenter', 'threat.is_threat',
                   'threat.is_known_abuser', 'threat.is_known_attacker'),
        'IPQS': ('proxy', 'tor', 'vpn', 'recent_abuse', 'bot_status'),
    }
    return all(getpath(data, path) is None or type(getpath(data, path)) is bool for path in fields.get(dataset, ()))


def country_fields_safe(data, dataset):
    fields = {
        'MaxMind': ('Country.IsoCode', 'Country.RegisteredCountry.IsoCode', 'City.Country.IsoCode', 'City.Continent.Code'),
        'IPinfo': ('data.country', 'data.abuse.country'),
        'SCAMALYTICS': ('external_datasources.maxmind_geolite2.ip_country_code',),
        'ipapi': ('location.country_code',), 'IP2LOCATION': ('country_code',),
        'ipdata': ('country_code',), 'IPQS': ('country_code',),
    }
    return all(getpath(data, path) is None or (isinstance(getpath(data, path), str)
               and re.fullmatch('[A-Z]{2}', getpath(data, path)) is not None)
               for path in fields.get(dataset, ()))


def scalar_fields_safe(data, dataset):
    fields = {
        'MaxMind': ('ASN.AutonomousSystemOrganization', 'City.Name', 'City.PostalCode', 'City.Location.TimeZone',
                    'Country.Name', 'Country.RegisteredCountry.Name', 'City.Continent.Name'),
        'IPinfo': ('data.asn.type', 'data.company.type', 'data.asn.name', 'data.city', 'data.postal', 'data.timezone'),
        'ipapi': ('asn.type', 'company.type', 'company.abuser_score'),
        'AbuseIPDB': ('data.usageType',), 'IP2LOCATION': ('usage_type', 'as_info.as_usage_type'),
    }
    return all(getpath(data, path) is None or isinstance(getpath(data, path), str)
               for path in fields.get(dataset, ()))


def unchecked_body(provider, dataset, body, expected_ip, family):
    if len(body) > MAX_BODY:
        return 'response_limit', None
    if not body or b'\0' in body:
        return 'schema_mismatch', None
    try:
        text = body.decode('utf-8')
    except UnicodeDecodeError:
        return 'schema_mismatch', None
    if provider == 'egress-discovery':
        try:
            observed = canonical_ip(text.strip(), family)
            return None, observed
        except ValueError:
            return 'schema_mismatch', None
    if provider in ('check-place-aggregator', 'ipinfo-public-widget'):
        if len(body) > MAX_JSON_BODY:
            return 'response_limit', None
        try:
            data = decode(text)
        except (ValueError, TypeError):
            return 'non_json', None
        if (not isinstance(data, dict) or not strings_bounded(data) or not coordinates_safe(data, dataset)
                or not boolean_fields_safe(data, dataset) or not country_fields_safe(data, dataset)
                or not scalar_fields_safe(data, dataset) or data.get('success') is False
                or data.get('status') in ('fail', 'error')):
            return 'schema_mismatch', None
        if data.get('error') not in (None, False, '') or data.get('errors') not in (None, []):
            return 'schema_mismatch', None
        # Query target binding is not a claim that an aggregated source echoed it.
        # Every provided target field must agree with the independently observed IP.
        for path in ('ip', 'query', 'ipAddress', 'data.ip', 'data.ipAddress'):
            value = getpath(data, path)
            if value is not None:
                try:
                    if canonical_ip(value, family) != expected_ip:
                        return 'schema_mismatch', None
                except (ValueError, TypeError):
                    return 'schema_mismatch', None
        criteria = {
            'MaxMind': lambda: isinstance(getpath(data, 'ASN'), dict) and isinstance(getpath(data, 'Country'), dict)
                      and re.fullmatch('[A-Z]{2}', getpath(data, 'Country.IsoCode') or '') is not None,
            'IPinfo': lambda: isinstance(data.get('data'), dict) and isinstance(getpath(data, 'data.asn'), dict)
                     and re.fullmatch('[A-Z]{2}', getpath(data, 'data.country') or '') is not None,
            'SCAMALYTICS': lambda: finite_integer(getpath(data, 'scamalytics.scamalytics_score'))
                          and isinstance(data.get('external_datasources'), dict),
            'ipapi': lambda: isinstance(data.get('asn'), dict) and isinstance(data.get('company'), dict)
                     and re.fullmatch('[A-Z]{2}', getpath(data, 'location.country_code') or '') is not None,
            'AbuseIPDB': lambda: finite_integer(getpath(data, 'data.abuseConfidenceScore'))
                        and getpath(data, 'data.ipAddress') is not None,
            'IP2LOCATION': lambda: finite_integer(data.get('fraud_score'))
                          and re.fullmatch('[A-Z]{2}', data.get('country_code') or '') is not None,
            'ipdata': lambda: isinstance(data.get('threat'), dict)
                      and re.fullmatch('[A-Z]{2}', data.get('country_code') or '') is not None,
            'IPQS': lambda: finite_integer(data.get('fraud_score'))
                    and re.fullmatch('[A-Z]{2}', data.get('country_code') or '') is not None,
        }
        try:
            return (None, None) if criteria[dataset]() else ('schema_mismatch', None)
        except (TypeError, ValueError):
            return 'schema_mismatch', None
    lower = text.lower()
    if any(token in lower for token in ('please wait...', 'challenge-form', 'captcha-form', 'g-recaptcha', 'cf-chl-')):
        return 'schema_mismatch', None
    if not all(token in lower for token in ('<html', '</html>', '<head', '</head>', '<body', '</body>')):
        return 'schema_mismatch', None
    criteria = {
        'Netflix': lambda: 'netflix' in lower,
        'Youtube': lambda: 'youtube premium' in lower and re.search(r'"contentRegion"\s*:\s*"[A-Z]{2}"', text),
        'TikTok': lambda: 'tiktok' in lower and re.search(r'"region"\s*:\s*"[A-Z]{2}"', text),
        'AmazonPrimeVideo': lambda: ('primevideo' in lower or 'prime video' in lower)
                           and re.search(r'"currentTerritory"\s*:\s*"[A-Z]{2}"', text),
        'Reddit': lambda: 'reddit' in lower and re.search(r'country="[A-Z]{2}"', text),
    }
    return (None, None) if criteria[dataset]() else ('schema_mismatch', None)


def validate_body(provider, dataset, body, expected_ip, family):
    # Untrusted JSON can contain deep nesting, enormous integers or isolated
    # Unicode surrogates even when the HTTP response fits the byte bound.
    # Classification must return to request() so it can persist a real failure.
    try:
        return unchecked_body(provider, dataset, body, expected_ip, family)
    except (RecursionError, OverflowError, UnicodeError, ValueError, TypeError, KeyError):
        return 'schema_mismatch', None


def request(arguments):
    url, native, write_out, output, follow = parse_arguments(arguments)
    expected_ip = target_ip()
    provider, dataset = identify(url, expected_ip)
    with locked_receipts() as (_path, records):
        require(len(records) < MAX_ATTEMPTS, 'per-job request limit reached before network I/O')
    attempted_at = int(time.time())
    started = time.monotonic()
    deadline = started + TIMEOUT
    current_url = url
    body = b''
    status, code, category, observed = None, 70, None, None
    for redirect in range(4):
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            category, code = 'timeout', 28
            break
        command = ['/usr/bin/curl', '-q', '--silent', '--show-error', '--proto', '=https', '--proto-redir', '=https',
                   '--max-time', str(remaining), '--connect-timeout', str(min(3, remaining)),
                   '--max-filesize', str(MAX_BODY), '--max-redirs', '0', '--write-out', WRITE_OUT, *native, current_url]
        capture_bytes, code, category = capture(command, deadline)
        body = capture_bytes
        redirect_url = ''
        if category is None:
            boundary = capture_bytes.rfind(MARKER)
            if boundary < 0:
                category = 'request_error'
            else:
                body = capture_bytes[:boundary]
                metadata = capture_bytes[boundary + len(MARKER):].decode('utf-8', errors='replace').rstrip('\n')
                fields = metadata.split('|', 3)
                if len(fields) != 4 or not re.fullmatch('[0-9]{3}', fields[0]):
                    category = 'request_error'
                else:
                    status = int(fields[0]) or None
                    redirect_url = fields[3]
        if category is None and code == 0 and status in (301, 302, 303, 307, 308) and follow:
            try:
                require(redirect < 3 and redirect_url, 'public endpoint redirect limit reached')
                destination = urljoin(current_url, redirect_url)
                require(identify(destination, expected_ip, redirect_from=url) == (provider, dataset),
                        'redirect changed provider or dataset')
                current_url = destination
                continue
            except ValueError:
                category = 'http_other'
        category = classify_exit(code, status, category)
        if category is None:
            category, observed = validate_body(provider, dataset, body, expected_ip, os.environ['APSP_IPQUALITY_FAMILY'])
        break
    elapsed = max(0, int((time.monotonic() - started) * 1000))
    messages = {'dns': 'DNS 解析失败', 'connect': '连接失败', 'tls': 'TLS 校验或握手失败', 'timeout': '请求超时',
                'http_403': 'HTTP 403：来源拒绝请求', 'http_429': 'HTTP 429：来源限流',
                'http_other': 'HTTP 状态或重定向不符合固定查询契约', 'non_json': '来源返回非 JSON 响应',
                'schema_mismatch': '来源字段、页面身份或目标 IP 不匹配',
                'response_limit': '响应超过有界读取限制', 'request_error': '未取得完整请求确认'}
    row = dict(provider=provider, dataset=dataset, target_ip=observed if provider == 'egress-discovery' else expected_ip,
               url=url, status='failed' if category else 'succeeded', attempted_at=attempted_at,
               elapsed_ms=elapsed, http_status=status, curl_exit=max(0, code) if code >= 0 else 70,
               # On a reader failure this is the retained prefix byte count,
               # not a claimed total length of an incomplete response body.
               response_bytes=len(body), error_kind=category, error_message=messages.get(category))
    append_receipt(row)
    # Failed body bytes must never reach the upstream heuristic or JSON parser.
    if not category and output is None:
        sys.stdout.buffer.write(observed.encode() if provider == 'egress-discovery' else body)
    replacements = {'http_code': f'{status or 0:03}', 'time_total': f'{elapsed / 1000:.3f}', 'url_effective': current_url}
    for name, value in replacements.items():
        write_out = write_out.replace('%{' + name + '}', value)
    sys.stdout.write(write_out.replace('\\n', '\n').replace('\\r', '\r').replace('\\t', '\t'))
    return (63 if category == 'response_limit' else 28 if category == 'timeout' else code if code > 0 else 22) if category else 0


def discover(family):
    require(family in ('4', '6') and os.environ.get('APSP_IPQUALITY_FAMILY') == family, 'discovery family mismatch')
    require(not os.environ.get('APSP_IPQUALITY_TARGET_IP'), 'discovery must precede the bound query target')
    # Two distinct public discovery providers; never retry the same endpoint.
    for url in ('https://api64.ipify.org/', 'https://ident.me/'):
        code = request(['-' + family, '--silent', '--max-time', '10', url])
        if code == 0:
            return 0
    return 69


def annotate(dataset, category, message):
    # Older fixed policies use more specific reader errors; keep the common wire enum.
    aliases = {'empty_response': 'schema_mismatch', 'invalid_response': 'schema_mismatch',
               'reader_error': 'request_error', 'incomplete_response': 'request_error',
               'response_too_large': 'response_limit', 'connection': 'connect', 'http_status': 'http_other', 'transport': 'request_error'}
    category = aliases.get(category, category)
    require(category in ERROR_KINDS - {'not_attempted'} and len(message) <= 256, 'bounded common error required')
    with locked_receipts() as (path, records):
        matched = [row for row in records if row['dataset'] == dataset]
        require(matched, 'source classification requires a real attempt')
        row = matched[-1]
        if row['status'] == 'succeeded':
            row.update(status='failed', error_kind=category, error_message=message)
            write_receipts(path, records)


def not_attempted(provider, dataset, message):
    require(provider in DISABLED and dataset in PAIRINGS[provider] and 0 < len(message) <= 256,
            'known disabled source with explanation required')
    append_receipt(dict(provider=provider, dataset=dataset, target_ip=target_ip(), url=None,
                        status='not_attempted', attempted_at=None, elapsed_ms=None, http_status=None,
                        curl_exit=None, response_bytes=None, error_kind='not_attempted', error_message=message))


def source_state(dataset):
    with locked_receipts() as (_path, records):
        rows = [row for row in records if row['dataset'] == dataset]
    return 0 if rows and all(row['status'] == 'succeeded' and row['target_ip'] == target_ip() for row in rows) else 1


def snapshot(stream):
    content = stream.read(MAX_SNAPSHOT + 1)
    require(0 < len(content) <= MAX_SNAPSHOT, 'bounded upstream partial object required')
    data = decode(content)
    require(isinstance(data, dict) and isinstance(data.get('Head'), dict)
            and canonical_ip(data['Head'].get('IP'), os.environ['APSP_IPQUALITY_FAMILY']) == target_ip(),
            'upstream partial object target differs from observed node egress')
    path = Path(os.environ.get('APSP_IPQUALITY_PARTIAL', ''))
    require(path.is_absolute(), 'absolute partial workspace path required')
    atomic(path, content, MAX_SNAPSHOT)


def main(arguments):
    if arguments and arguments[0] == 'discover':
        require(len(arguments) == 2, 'one discovery family required')
        return discover(arguments[1])
    if arguments and arguments[0] == 'not-attempted':
        require(len(arguments) == 4, 'one disabled source explanation required')
        not_attempted(*arguments[1:])
        return 0
    if arguments and arguments[0] == 'annotate':
        require(len(arguments) == 4, 'one source classification required')
        annotate(*arguments[1:])
        return 0
    if arguments and arguments[0] == 'source-state':
        require(len(arguments) == 2, 'one source state required')
        return source_state(arguments[1])
    if arguments == ['snapshot']:
        snapshot(sys.stdin.buffer)
        return 0
    return request(arguments)


if __name__ == '__main__':
    try:
        raise SystemExit(main(sys.argv[1:]))
    except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError) as error:
        # Error text never includes native stderr, credentials or response bodies.
        print('IPQuality request guard: ' + str(error), file=sys.stderr)
        raise SystemExit(70) from None
