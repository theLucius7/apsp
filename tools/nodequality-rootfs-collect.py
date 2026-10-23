#!/usr/bin/env python3
"""Collect authenticated Debian inputs; never approve or build a rootfs."""

import argparse
import contextlib
import datetime
import email.parser
import email.message
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import shutil
import socket
import ssl
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request


HERE = Path(__file__).resolve()
spec = importlib.util.spec_from_file_location('apsp_rootfs_build', HERE.with_name('nodequality-rootfs-build.py'))
BUILD = importlib.util.module_from_spec(spec)
spec.loader.exec_module(BUILD)

MAX_TOTAL = 64 * 1024**3
MAX_SECONDS = 7200
MAX_IMPORTS = 1024**2
MAX_RECEIPT = 8 * 1024**2
MIN_FREE = 512 * 1024**2
FAILURE_EVIDENCE_SECONDS = 2
REQUEST_FIELDS = {'schema', 'arch', 'source_epoch', 'repositories'}
REPOSITORY_FIELDS = {'id', 'archive', 'timestamp', 'suite'}
COLLECTION_KIND = 'apsp-nodequality-debian-input-collection'
BASE_ENV = {'PATH': '/usr/sbin:/usr/bin:/sbin:/bin', 'LANG': 'C', 'LC_ALL': 'C'}
APTPATH = '/usr/bin/apt-get'


def require(condition, message):
    BUILD.require(condition, message)


def timestamp():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def debian_release(path=Path('/etc/os-release')):
    path = Path(path)
    if path.is_symlink():
        require(path == Path('/etc/os-release') and path.resolve(strict=True) == Path('/usr/lib/os-release'),
                'unsupported operating-system release link')
        path = Path('/usr/lib/os-release')
    fields = {}
    for line in BUILD.read_regular(path, 65536).decode('utf-8').splitlines():
        if '=' in line:
            key, value = line.split('=', 1)
            fields[key] = value.strip('"\'')
    require(fields.get('ID') == 'debian' and fields.get('VERSION_ID') == '12', 'collection requires Debian 12')
    return fields


def native_arch():
    require(sys.platform == 'linux', 'collection requires a native Debian 12 Linux host')
    require(os.geteuid() == 0, 'isolated native collection requires root on its dedicated host')
    debian_release()
    value = {'x86_64': 'amd64', 'aarch64': 'arm64'}.get(platform.machine())
    require(value is not None, 'unsupported native collection architecture')
    return value


def validate_request(request):
    require(isinstance(request, dict) and set(request) == REQUEST_FIELDS, 'invalid collection request fields')
    require(type(request['schema']) is int and request['schema'] == 1 and request['arch'] in BUILD.ARCHES,
            'invalid collection request identity')
    require(type(request['source_epoch']) is int and 0 < request['source_epoch'] < 2**32, 'fixed source epoch required')
    require(isinstance(request['repositories'], list) and 2 <= len(request['repositories']) <= 3,
            'main and security repositories required')
    seen, imports, suites = set(), {}, set()
    for repo in request['repositories']:
        require(isinstance(repo, dict) and set(repo) == REPOSITORY_FIELDS, 'invalid requested repository')
        BUILD.relative(repo['id'])
        require('/' not in repo['id'] and repo['id'] not in seen, 'duplicate requested repository')
        require(repo['archive'] in BUILD.SIGNERS and re.fullmatch(r'[0-9]{8}T[0-9]{6}Z', repo['timestamp'] or ''),
                'exact requested snapshot timestamp required')
        datetime.datetime.strptime(repo['timestamp'], '%Y%m%dT%H%M%SZ')
        permitted = {'bookworm', 'bookworm-updates'} if repo['archive'] == 'debian' else {'bookworm-security'}
        require(repo['suite'] in permitted and repo['suite'] not in suites, 'duplicate or non-bookworm suite')
        require(repo['archive'] not in imports or imports[repo['archive']] == repo['timestamp'],
                'one archive must use one exact import')
        seen.add(repo['id'])
        imports[repo['archive']] = repo['timestamp']
        suites.add(repo['suite'])
    require(set(imports) == set(BUILD.SIGNERS) and 'bookworm' in suites, 'main/security pair is incomplete')
    return imports


def import_url(archive, chosen):
    day = datetime.datetime.strptime(chosen, '%Y%m%dT%H%M%SZ').date()
    query = urllib.parse.urlencode({'archive': archive, 'after': day.strftime('%Y%m%dT000000Z'),
                                   'before': (day + datetime.timedelta(days=1)).strftime('%Y%m%dT000000Z')})
    return 'https://snapshot.debian.org/mr/timestamp/?' + query


def validate_imports(content, archive, chosen):
    value = BUILD.decode(content)
    require(isinstance(value, dict) and isinstance(value.get('result'), dict)
            and set(value['result']) == {archive}, 'wrong snapshot import response')
    rows = value['result'][archive]
    require(isinstance(rows, list) and rows and len(rows) <= 4096, 'invalid snapshot import list')
    require(all(isinstance(row, str) and re.fullmatch(r'[0-9]{8}T[0-9]{6}Z', row) for row in rows)
            and len(rows) == len(set(rows)), 'ambiguous snapshot import list')
    for row in rows:
        datetime.datetime.strptime(row, '%Y%m%dT%H%M%SZ')
    require(chosen in rows, 'requested timestamp is absent; nearest earlier snapshot is not accepted')
    return rows


def allowed_url(value):
    parsed = urllib.parse.urlsplit(value)
    require(parsed.scheme == 'https' and parsed.hostname == 'snapshot.debian.org'
            and parsed.netloc == 'snapshot.debian.org' and not parsed.fragment, 'non-official collection URL')
    require(parsed.path.startswith(('/archive/debian/', '/archive/debian-security/', '/mr/timestamp/', '/file/')),
            'unsupported official collection path')
    return value


class OfficialRedirects(urllib.request.HTTPRedirectHandler):
    def __init__(self, context=None):
        super().__init__()
        self.context = context

    def redirect_request(self, request, stream, code, message, headers, destination):
        if self.context is not None:
            self.context.update(response_received=True, http_status=code, final_url=request.full_url,
                                response_content_length=headers.get('Content-Length'), response_headers=diagnostic_headers(headers),
                                redirect_target=destination, failure_stage='redirect_url_validation')
        allowed_url(destination)
        if self.context is not None:
            self.context['failure_stage'] = 'redirect_followup_open'
        return super().redirect_request(request, stream, code, message, headers, destination)


def free_disk(path, reserve, additional=0):
    require(type(reserve) is int and MIN_FREE <= reserve <= MAX_TOTAL, 'invalid explicit free-disk reserve')
    usage = os.statvfs(path)
    require(usage.f_bavail * usage.f_frsize >= reserve + additional, 'factory free-disk reserve would be crossed')


def publish(path, content, limit):
    require(len(content) <= limit, 'published metadata exceeds its reader limit')
    BUILD.write_new(path, content)


def publish_success(path, content, limit, owned):
    """Register exclusive inode ownership before a publication can fail."""
    require(len(content) <= limit, 'published metadata exceeds its reader limit')
    stream = None
    try:
        with BUILD.deferred_signals():
            stream = path.open('xb')
            metadata = os.fstat(stream.fileno())
            owned.append((path, metadata.st_dev, metadata.st_ino))
        os.fchmod(stream.fileno(), 0o600)
        require(stream.write(content) == len(content), 'short success metadata write')
        stream.flush()
        os.fsync(stream.fileno())
    finally:
        if stream is not None:
            original = sys.exc_info()[1]
            try:
                stream.close()
            except BaseException as error:
                if original is None:
                    raise
                if hasattr(original, 'add_note'):
                    original.add_note('Success metadata close also failed: ' + type(error).__name__)


def rollback_success(owned):
    result = []
    for path, device, inode in reversed(owned):
        try:
            metadata = path.lstat()
            require(stat.S_ISREG(metadata.st_mode) and (metadata.st_dev, metadata.st_ino) == (device, inode),
                    'success publication identity changed; refusing to remove a replacement')
            path.unlink()
            result.append({'path': path.name, 'removed': True})
        except FileNotFoundError:
            result.append({'path': path.name, 'removed': True, 'already_absent': True})
        except Exception as error:
            result.append({'path': path.name, 'removed': False,
                           'error_type': type(error).__name__, 'error': diagnostic_error_message(error)})
    return result


def diagnostic_text(value, limit):
    if value is None:
        return None
    return re.sub(r'[\x00-\x1f\x7f]', ' ', str(value))[:limit]


def diagnostic_url(value):
    if not isinstance(value, str):
        return None
    try:
        parsed = urllib.parse.urlsplit(value)
        if parsed.scheme not in ('http', 'https') or not parsed.hostname:
            return None
        # Credentials, query arguments and fragments do not belong in receipts.
        host = parsed.hostname
        if ':' in host:
            host = '[' + host + ']'
        if parsed.port is not None:
            host += ':' + str(parsed.port)
        return diagnostic_text(urllib.parse.urlunsplit((parsed.scheme, host, parsed.path, '', '')), 2048)
    except ValueError:
        return None


def diagnostic_headers(headers):
    result = {}
    for name in ('Content-Length', 'Content-Type', 'Content-Range', 'Content-Encoding',
                 'Transfer-Encoding', 'Retry-After', 'Location'):
        values = headers.get_all(name) if hasattr(headers, 'get_all') else [headers.get(name)]
        if values:
            safe = [diagnostic_url(value) if name == 'Location' else diagnostic_text(value, 256)
                    for value in values[:4] if value is not None]
            if safe:
                result[name] = safe
    return result


def retained_headers(content):
    """Sanitize either an old raw header record or the current JSON record."""
    if not content.lstrip().startswith(b'{'):
        return diagnostic_headers(email.parser.BytesParser().parsebytes(content, headersonly=True))
    value = BUILD.decode(content)
    require(isinstance(value, dict) and value.get('representation') == 'selected-parsed-headers'
            and isinstance(value.get('headers'), dict), 'invalid selected response headers')
    selected = email.message.Message()
    for name, values in value['headers'].items():
        require(isinstance(name, str) and isinstance(values, list) and len(values) <= 4
                and all(isinstance(item, str) for item in values), 'invalid selected response header values')
        for item in values:
            selected[name] = item
    return diagnostic_headers(selected)


def diagnostic_error_message(error):
    reason = error.reason if isinstance(error, urllib.error.URLError) else error
    message = 'HTTP Error ' + str(error.code) if isinstance(error, urllib.error.HTTPError) else str(reason)
    message = re.sub(r'https?://[^\s]+', lambda match: diagnostic_url(match[0]) or '[redacted URL]', message)
    return diagnostic_text(message, 1024)


def parent_failure(error):
    if isinstance(error, (KeyboardInterrupt, SystemExit)):
        category = 'cancelled'
    elif isinstance(error, (TimeoutError, subprocess.TimeoutExpired)) or (
            isinstance(error, ValueError) and str(error) == 'overall operation deadline exceeded'):
        category = 'timeout'
    else:
        category = 'producer_failed_or_deadline'
    command = getattr(error, 'factory_command', {})
    return {'category': category, 'error_type': diagnostic_text(type(error).__name__, 64),
            'error_message': diagnostic_error_message(error),
            'returncode': command.get('returncode'), 'cleanup_returncode': command.get('cleanup_returncode'),
            'output_truncated': command.get('output_truncated')}


def stream_response(response, output, limit, reserve=MIN_FREE, context=None):
    """The limit applies before every write, including chunked responses."""
    length = 0
    with open(output, 'xb') as target:
        os.fchmod(target.fileno(), 0o600)
        while True:
            if context is not None:
                context['failure_stage'] = 'body_read'
            chunk = response.read(min(65536, limit - length + 1))
            if not chunk:
                break
            if context is not None:
                context['response_bytes_read'] += len(chunk)
                context['failure_stage'] = 'body_byte_limit'
            require(length + len(chunk) <= limit, 'download exceeds its byte budget')
            if context is not None:
                context['failure_stage'] = 'body_disk_reserve'
            free_disk(Path(output).parent, reserve, len(chunk))
            if context is not None:
                context['failure_stage'] = 'body_write'
            target.write(chunk)
            length += len(chunk)
            if context is not None:
                context['response_bytes_written'] = length
    if context is not None:
        context['failure_stage'] = 'body_nonempty_validation'
    require(length > 0, 'empty official download')
    return length


def fetch_worker(url, output, limit, header_output, reserve=MIN_FREE, context=None):
    context = context if context is not None else {}
    context.update(failure_stage='url_validation', response_received=False, http_status=None,
                   final_url=None, response_content_length=None, response_bytes_read=0, response_bytes_written=0,
                   redirect_target=None, response_headers={}, content_length_validation_issue=None)
    allowed_url(url)
    context['failure_stage'] = 'worker_preflight'
    require(type(limit) is int and 0 < limit <= BUILD.MAX_SOURCE, 'invalid worker byte limit')
    output = Path(output).absolute()
    BUILD.private_directory(output.parent)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), OfficialRedirects(context))
    # The parent supervises DNS, TLS and every body read with an absolute deadline.
    context['failure_stage'] = 'request_open'
    with opener.open(url, timeout=30) as response:
        context.update(response_received=True, http_status=response.status, final_url=response.geturl(),
                       response_content_length=response.headers.get('Content-Length'), response_headers=diagnostic_headers(response.headers),
                       failure_stage='response_status_validation')
        require(response.status == 200, 'official download did not return HTTP 200')
        context['failure_stage'] = 'response_url_validation'
        allowed_url(response.geturl())
        advertised = response.headers.get('Content-Length')
        context['failure_stage'] = 'content_length_validation'
        declared_lengths = response.headers.get_all('Content-Length') if hasattr(response.headers, 'get_all') else (
            [advertised] if advertised is not None else [])
        if len(declared_lengths or []) > 1 or (advertised is not None and response.headers.get('Transfer-Encoding') is not None):
            context['content_length_validation_issue'] = 'ambiguous_framing'
            raise ValueError('ambiguous response Content-Length framing')
        if advertised is not None:
            if not re.fullmatch(r'[0-9]{1,20}', advertised):
                context['content_length_validation_issue'] = 'not_decimal'
            elif int(advertised) > limit:
                context['content_length_validation_issue'] = 'exceeds_worker_byte_limit'
            require(context['content_length_validation_issue'] is None, 'advertised download exceeds budget')
        context['failure_stage'] = 'response_header_validation'
        require(len(response.headers.as_bytes()) <= 65536, 'oversized official response headers')
        # Keep only the same bounded allowlist used for failure diagnostics.
        # Parsed cookies, authentication challenges and arbitrary server fields
        # never become retained evidence, even on successful requests.
        headers = BUILD.canonical({'schema': 1, 'representation': 'selected-parsed-headers',
                                   'headers': context['response_headers']}) + b'\n'
        context['failure_stage'] = 'response_header_publication'
        BUILD.write_new(header_output, headers, 0o600)
        size = stream_response(response, output, limit, reserve, context)
        context['failure_stage'] = 'content_length_verification'
        if advertised is not None and size != int(advertised):
            context['content_length_validation_issue'] = 'declared_length_mismatch'
        require(advertised is None or size == int(advertised), 'download differs from Content-Length')
        return {'schema': 1, 'url': diagnostic_url(url), 'final_url': diagnostic_url(response.geturl()), 'http_status': 200,
                'received_at': timestamp(), 'size': size, 'header_representation': 'selected-parsed-headers',
                'response_received': True, 'response_headers': context['response_headers'],
                'response_content_length': diagnostic_text(advertised, 128),
                'response_content_length_truncated': isinstance(advertised, str) and len(advertised) > 128,
                'response_bytes_read': context['response_bytes_read'], 'response_bytes_written': context['response_bytes_written']}


def fetch_error(error, url, started, context=None):
    context = dict(context or {})
    http_error = isinstance(error, urllib.error.HTTPError)
    if http_error:
        context.update(response_received=True, http_status=error.code, final_url=error.geturl(),
                       response_content_length=error.headers.get('Content-Length') if error.headers is not None else None,
                       response_headers=diagnostic_headers(error.headers) if error.headers is not None else {})
    observed = context.get('http_status')
    status = observed if type(observed) is int and 100 <= observed <= 599 else None
    reason = error.reason if isinstance(error, urllib.error.URLError) else error
    if isinstance(reason, (KeyboardInterrupt, SystemExit)):
        category = 'cancelled'
    elif http_error and status is not None:
        category = 'http_' + str(status)
    elif isinstance(reason, socket.gaierror):
        category = 'dns'
    elif isinstance(reason, ssl.SSLError):
        category = 'tls'
    elif isinstance(reason, (TimeoutError, socket.timeout)):
        category = 'timeout'
    elif isinstance(reason, ValueError) and 'budget' in str(reason) and context.get('content_length_validation_issue') != 'not_decimal':
        category = 'response_too_large'
    elif isinstance(reason, (ValueError, http.client.IncompleteRead)):
        category = 'response_invalid'
    else:
        category = 'connection'
    message = diagnostic_error_message(error)
    advertised = context.get('response_content_length')
    counters = {key: context.get(key, 0) for key in ('response_bytes_read', 'response_bytes_written')}
    counters = {key: value if type(value) is int and 0 <= value <= BUILD.MAX_SOURCE + 1 else None
                for key, value in counters.items()}
    return {'schema': 1, 'url': diagnostic_url(url), 'http_status': status, 'category': category,
            'complete': False, 'received_at': timestamp(), 'elapsed_seconds': time.monotonic() - started,
            'error_type': diagnostic_text(type(error).__name__, 64), 'error_message': diagnostic_text(message, 1024),
            'failure_stage': diagnostic_text(context.get('failure_stage', 'request_open'), 64),
            'error_origin': 'download_worker',
            'response_received': context.get('response_received', False) is True,
            'final_url': diagnostic_url(context.get('final_url')),
            'redirect_target': diagnostic_url(context.get('redirect_target')),
            'http_status_observation_scope': 'latest_received_response' if context.get('response_received') is True else None,
            'response_headers': context.get('response_headers', {}),
            'response_content_length': diagnostic_text(advertised, 128),
            'response_content_length_truncated': isinstance(advertised, str) and len(advertised) > 128,
            'content_length_validation_issue': diagnostic_text(context.get('content_length_validation_issue'), 64),
            **counters}


def owned_size(root, deadline):
    total, members = 0, 0
    stack = [BUILD.private_directory(root)]
    while stack:
        directory = stack.pop()
        for path in directory.iterdir():
            deadline.check()
            info = path.lstat()
            members += 1
            require(members <= BUILD.MAX_MEMBERS, 'factory directory has too many entries')
            if stat.S_ISDIR(info.st_mode):
                require(not path.is_symlink(), 'factory directory has a link parent')
                stack.append(path)
            else:
                require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1, 'factory directory contains a non-private ordinary file')
                total += info.st_size
    return total


class Collector:
    def __init__(self, output, max_total, deadline, reserve=MIN_FREE):
        require(type(max_total) is int and 0 < max_total <= MAX_TOTAL, 'invalid explicit factory byte budget')
        self.output, self.max_total, self.deadline = BUILD.private_directory(output), max_total, deadline
        self.output_identity = BUILD.FactoryCapacity.identity(self.output)
        self.parent_identity = BUILD.FactoryCapacity.identity(self.output.parent)
        self.reserve = reserve
        self.cache = self.output / 'input-cache'
        self.cache.mkdir(mode=0o700)
        self.downloads, self.signatures = [], []

    def budget(self, additional=0):
        self.ensure_identity()
        require(owned_size(self.output, self.deadline) + additional <= self.max_total, 'factory byte budget exceeded')
        free_disk(self.output, self.reserve, additional)

    def ensure_identity(self):
        require(BUILD.FactoryCapacity.identity(self.output) == self.output_identity
                and BUILD.FactoryCapacity.identity(self.output.parent) == self.parent_identity,
                'collection output directory identity changed')

    def failure_budget(self, content, deadline):
        self.ensure_identity()
        deadline.check()
        require(len(content) <= MAX_RECEIPT, 'failure receipt exceeds its reader limit')
        require(owned_size(self.output, deadline) + len(content) <= self.max_total,
                'factory byte budget exceeded while retaining failure metadata')
        free_disk(self.output, self.reserve, len(content))
        deadline.check()

    def failed_download(self, work, url, error, phase, expected=None, actual=None, worker=None):
        parent = parent_failure(error)
        raw_receipt = None
        receipt_error = None
        if (work / 'receipt.json').exists():
            try:
                raw_receipt = BUILD.read_regular(work / 'receipt.json', 65536)
                if worker is None:
                    candidate = BUILD.decode(raw_receipt)
                    if isinstance(candidate, dict) and candidate.get('complete') is False:
                        worker = candidate
            except (OSError, ValueError) as invalid:
                receipt_error = diagnostic_text(str(invalid), 1024)
        if worker is None:
            report = {'schema': 1, 'url': diagnostic_url(url), 'complete': False,
                'category': parent['category'], 'http_status': None, 'elapsed_seconds': None,
                'response_received': None, 'failure_stage': 'worker_receipt_unavailable',
                'error_type': None, 'error_message': None, 'error_origin': None,
                'final_url': None, 'response_content_length': None, 'response_headers': {},
                'redirect_target': None, 'http_status_observation_scope': None,
                'response_bytes_read': None, 'response_bytes_written': None}
        else:
            report = dict(worker)
            if worker.get('complete') is not False:
                report.update(complete=False, category='signed_identity_mismatch' if phase == 'signed_payload_validation' else 'response_invalid',
                    failure_stage=phase, error_type=diagnostic_text(type(error).__name__, 64),
                    error_message=diagnostic_text(str(error), 1024), error_origin='collector')
        report.update(parent_failure_stage=phase, parent_failure=parent,
                      expected_identity=expected, actual_identity=actual)
        if receipt_error is not None:
            report['worker_receipt_read_error'] = receipt_error
        self.downloads.append(report)
        evidence = {'complete': False, 'body_retained': False, 'worker_receipt': None, 'response_headers': None}
        report['failure_evidence'] = evidence
        try:
            self.ensure_identity()
            # Unauthenticated payloads never enter the cache or failure evidence.
            # Removing only this worker's body also prevents its discarded bytes
            # from consuming the metadata retention budget.
            with contextlib.suppress(FileNotFoundError):
                (work / 'body').unlink()
            headers = None
            if (work / 'headers').exists():
                original = BUILD.read_regular(work / 'headers', 65536)
                selected = retained_headers(original)
                headers = BUILD.canonical({'schema': 1, 'representation': 'selected-parsed-headers', 'headers': selected}) + b'\n'
            elif report.get('response_headers'):
                headers = BUILD.canonical({'schema': 1, 'representation': 'selected-parsed-headers',
                                           'headers': report['response_headers']}) + b'\n'
            require(headers is None or len(headers) <= 65536, 'failure headers exceed their reader limit')
            # This bounded cleanup phase runs after a request timeout/cancel. It
            # does not grant the download another deadline, retry or byte budget.
            cleanup_deadline = BUILD.Deadline(FAILURE_EVIDENCE_SECONDS)
            metadata_bytes = (len(raw_receipt) if raw_receipt is not None else 0) + (len(headers) if headers is not None else 0)
            require(owned_size(self.output, cleanup_deadline) + metadata_bytes <= self.max_total,
                    'factory byte budget exceeded while retaining failure metadata')
            free_disk(self.output, self.reserve, metadata_bytes)
            with BUILD.deferred_signals():
                directory = BUILD.reserve_output(self.output / ('failed-download-' + str(len(self.downloads) - 1).zfill(6)))
            evidence['directory'] = directory.name
            for name, content, field in (('worker-receipt.json', raw_receipt, 'worker_receipt'),
                                         ('response-headers.json', headers, 'response_headers')):
                if content is not None:
                    cleanup_deadline.check()
                    publish(directory / name, content, 65536)
                    evidence[field] = {'path': directory.name + '/' + name, 'sha256': hashlib.sha256(content).hexdigest(), 'size': len(content)}
            evidence['complete'] = True
        except Exception as retention_error:
            evidence['error'] = diagnostic_text(str(retention_error), 1024)
        if hasattr(error, 'add_note'):
            error.add_note('Official download failed: ' + str(report.get('category')) + '; stage=' + str(report.get('failure_stage')))
        return report

    def obtain(self, url, limit, expected=None, suffix=''):
        self.budget()
        available = self.max_total - owned_size(self.output, self.deadline)
        bound = min(limit, available)
        require(bound > 0 and (expected is None or expected['size'] <= bound), 'insufficient factory download budget')
        with tempfile.TemporaryDirectory(prefix='download-', dir=self.output) as name:
            work = Path(name)
            body, headers, receipt = work / 'body', work / 'headers', work / 'receipt.json'
            argv = [sys.executable, str(HERE), '_fetch', '--url', allowed_url(url), '--output', str(body),
                    '--headers', str(headers), '--receipt', str(receipt), '--limit', str(bound),
                    '--reserve-free-bytes', str(self.reserve)]
            try:
                raw = BUILD.run_bounded(argv, BUILD.Deadline(min(180, self.deadline.remaining()), capacity=self.deadline.capacity), 65536)
            except BaseException as error:
                self.failed_download(work, url, error, 'producer_exit', expected)
                raise
            report, actual, phase = None, None, 'worker_receipt_validation'
            try:
                candidate = BUILD.decode(BUILD.read_regular(receipt, 65536, self.deadline))
                require(isinstance(candidate, dict) and BUILD.decode(raw) == candidate, 'worker output and receipt differ')
                report = candidate
                phase = 'payload_identity_validation'
                actual = BUILD.file_identity(body, bound, self.deadline)
                require(report.get('size') == actual['size'] and report.get('http_status') == 200, 'download worker identity mismatch')
                if expected is not None:
                    phase = 'signed_payload_validation'
                    require(actual == expected, 'download differs from authenticated SHA256/size')
            except BaseException as error:
                self.failed_download(work, url, error, phase, expected, actual, report)
                raise
            blob = actual['sha256'] + suffix
            destination = self.cache / blob
            if destination.exists():
                require(BUILD.file_identity(destination, limit, self.deadline) == actual, 'cache identity conflict')
            else:
                os.rename(body, destination)
            header_blob = 'http-' + str(len(self.downloads)) + '.headers'
            destination_header = self.output / header_blob
            os.rename(headers, destination_header)
            report.update(actual, blob=blob, headers=header_blob)
            self.downloads.append(report)
            self.budget()
            return dict(actual, blob=blob)


def verify_candidate(candidate, arch, deadline):
    require(isinstance(candidate, dict) and set(candidate) == {'image_sha256', 'arch', 'tools'}, 'invalid candidate builder record')
    require(candidate['arch'] == arch and BUILD.SHA256.fullmatch(candidate['image_sha256'] or ''), 'invalid candidate image identity')
    require(isinstance(candidate['tools'], list) and len(candidate['tools']) == len(BUILD.TOOL_PATHS), 'complete candidate tool record required')
    found = set()
    for row in candidate['tools']:
        require(isinstance(row, dict) and set(row) == {'name', 'path', 'version', 'sha256', 'size'}, 'invalid candidate tool')
        require(row['name'] in BUILD.TOOL_PATHS and row['name'] not in found
                and row['path'] == BUILD.TOOL_PATHS[row['name']], 'wrong candidate tool path')
        require(isinstance(row['version'], str) and 0 < len(row['version']) <= 128, 'candidate tool version required')
        require(BUILD.file_identity(row['path'], BUILD.MAX_ARCHIVE, deadline) ==
                {key: row[key] for key in ('sha256', 'size')}, 'candidate tool bytes differ from this host')
        found.add(row['name'])
    return candidate


def record_collection_tools(deadline):
    result = []
    paths = {'/usr/bin/gpgv', '/usr/bin/unshare', APTPATH, '/usr/bin/env', '/usr/bin/dpkg-query',
            '/usr/lib/apt/methods/file', '/usr/lib/apt/methods/gpgv', str(Path(sys.executable).resolve())}
    for requested in sorted(paths):
        resolved = Path(requested).resolve(strict=True)
        identity = BUILD.file_identity(resolved, BUILD.MAX_ARCHIVE, deadline)
        ownership = BUILD.run_bounded(['/usr/bin/dpkg-query', '--search', str(resolved)],
                                      BUILD.Deadline(min(15, deadline.remaining()), capacity=deadline.capacity), 65536)
        require(ownership.strip() and b'\0' not in ownership, 'collection tool lacks dpkg ownership evidence')
        result.append(dict(identity, requested_path=requested, resolved_path=str(resolved),
                           dpkg_ownership=ownership.decode('utf-8').strip()))
    versions = BUILD.run_bounded(['/usr/bin/dpkg-query', '--show',
            '--showformat=${binary:Package}\t${Version}\t${Architecture}\t${source:Package}\t${source:Version}\n',
            'apt', 'gpgv', 'util-linux', 'python3-minimal'],
            BUILD.Deadline(min(15, deadline.remaining()), capacity=deadline.capacity), 65536).decode('utf-8')
    actual_versions = {}
    for path in (APTPATH, '/usr/bin/gpgv', '/usr/bin/unshare', str(Path(sys.executable).resolve())):
        actual_versions[path] = BUILD.run_bounded([path, '--version'],
                BUILD.Deadline(min(15, deadline.remaining()), capacity=deadline.capacity), 65536).decode('utf-8').strip()
    return {'executables': result, 'dpkg_report': versions, 'actual_version_output': actual_versions,
            'evidence_scope': 'actual bytes and local dpkg claims; not independent builder approval'}


def repository_url(repo, path):
    BUILD.relative(path)
    return BUILD.repository_url(repo, path)


def pool_path(repo, value):
    path = BUILD.relative(value)
    prefix = {'debian': 'pool/main/', 'debian-security': 'pool/updates/main/'}[repo['archive']]
    require(path.startswith(prefix), 'package/source is outside this signed Debian main archive')
    return path


def authenticate_metadata(collector, requested, keyring):
    repo = dict(requested)
    repo['inrelease'] = collector.obtain(repository_url(repo, 'dists/' + repo['suite'] + '/InRelease'), BUILD.MAX_LOCK)
    with tempfile.TemporaryDirectory(prefix='signature-', dir=collector.output) as name:
        home = Path(name)
        release = home / 'Release'
        status = BUILD.run_bounded(['/usr/bin/gpgv', '--homedir', str(home), '--keyring', str(keyring),
                '--status-fd', '1', '--output', str(release), str(collector.cache / repo['inrelease']['blob'])],
                BUILD.Deadline(min(30, collector.deadline.remaining()), capacity=collector.deadline.capacity), 65536, stderr=subprocess.DEVNULL)
        signers = BUILD.valid_signers(status, repo['archive'], repo['timestamp'])
        publish(collector.output / (repo['id'] + '.gpg-status'), status, 65536)
        records = list(BUILD.control_records(BUILD.read_regular(release, BUILD.MAX_LOCK, collector.deadline)))
        require(len(records) == 1 and records[0].get('Origin') == 'Debian'
                and records[0].get('Codename') == repo['suite'], 'wrong authenticated Release identity')
        sums = BUILD.checksum_rows(records[0].get('SHA256', ''))
        collector.signatures.append({'repository': repo['id'], 'primary_fingerprints': signers,
                                      'inrelease_sha256': repo['inrelease']['sha256'],
                                      'gpg_status_file': repo['id'] + '.gpg-status'})
    repo['indices'], indices = [], {}
    for kind, base in [('Packages', 'main/binary-' + collector.arch + '/Packages'), ('Sources', 'main/source/Sources')]:
        path = next((base + extension for extension in ('.xz', '.gz') if base + extension in sums), None)
        require(path is not None, 'authenticated Release lacks the required native/source index')
        descriptor = collector.obtain(repository_url(repo, 'dists/' + repo['suite'] + '/' + path),
                                      BUILD.MAX_INDEX, sums[path], Path(path).suffix)
        repo['indices'].append(dict(descriptor, kind=kind, path=path))
        indices[kind] = collector.cache / descriptor['blob']
    return repo, indices


def iter_rows(value, deadline):
    # Lists are owned small test fixtures. Production retains only cache paths.
    if isinstance(value, list):
        yield from value
    else:
        yield from BUILD.index_records(value, BUILD.MAX_INDEX, deadline)


def essential_seeds(repositories, indices, deadline=None):
    deadline = deadline or BUILD.Deadline(60)
    main = next(repo for repo in repositories if repo['suite'] == 'bookworm')
    rows = iter_rows(indices[main['id']]['Packages'], deadline)
    names = {row['Package'] for row in rows if row.get('Essential') == 'yes'}
    require(names and all(BUILD.NAME.fullmatch(name) for name in names), 'authenticated main essential set is missing')
    # APT, rather than string ordering, chooses versions across the same signed mirrors.
    return sorted(names | {'apt'} | set(BUILD.TOOL_PACKAGES.values())), sorted(names)


def apt_config(work, mirrors, arch):
    def quote(path):
        value = str(path)
        require('"' not in value and '\\' not in value and '\n' not in value, 'unsafe APT directory')
        return '"' + value + '"'
    for name in ('confparts', 'sourceparts', 'prefparts', 'trustparts', 'lists/partial', 'archives/partial', 'log'):
        (work / name).mkdir(parents=True, mode=0o700)
    for name in ('empty-main', 'status', 'extended_states', 'preferences', 'empty-trusted'):
        BUILD.write_new(work / name, b'')
    lines = []
    for repo, mirror in mirrors:
        signers = ','.join(sorted(BUILD.SIGNERS[repo['archive']]))
        lines.append('deb [arch=' + arch + ' check-valid-until=no signed-by=' + str(mirror / 'archive-keyring.gpg')
                     + ',' + signers + '] file://' + str(mirror) + ' ' + repo['suite'] + ' main')
    BUILD.write_new(work / 'sources.list', ('\n'.join(lines) + '\n').encode('ascii'))
    paths = {'Dir::Etc': '.', 'Dir::State': '.', 'Dir::Cache': '.', 'Dir::Log': 'log',
             'Dir::Etc::main': 'empty-main', 'Dir::Etc::parts': 'confparts',
             'Dir::Etc::sourcelist': 'sources.list', 'Dir::Etc::sourceparts': 'sourceparts',
             'Dir::Etc::preferences': 'preferences', 'Dir::Etc::preferencesparts': 'prefparts',
             'Dir::Etc::trusted': 'empty-trusted', 'Dir::Etc::trustedparts': 'trustparts',
             'Dir::State::status': 'status', 'Dir::State::extended_states': 'extended_states', 'Dir::State::lists': 'lists',
             'Dir::Log::History': 'log/history.log', 'Dir::Log::Terminal': 'log/term.log',
             'Dir::Cache::archives': 'archives'}
    config = [key + ' ' + quote(work / path) + ';' for key, path in sorted(paths.items())]
    config.extend(['#clear APT::Architectures;', 'APT::Architecture "' + arch + '";',
        'APT::Architectures { "' + arch + '"; };', 'Dir::Cache::pkgcache "";', 'Dir::Cache::srcpkgcache "";',
        'APT::Install-Recommends "false";', 'APT::Install-Suggests "false";',
        'APT::Sandbox::User "root";',
        'Acquire::Retries "0";', 'Acquire::Languages "none";', 'Acquire::PDiffs "false";',
        'Acquire::By-Hash "false";', 'Acquire::AllowInsecureRepositories "false";',
        'Acquire::AllowWeakRepositories "false";', 'Acquire::AllowDowngradeToInsecureRepositories "false";',
        '#clear DPkg::Pre-Invoke;', '#clear DPkg::Post-Invoke;', '#clear DPkg::Pre-Install-Pkgs;',
        '#clear APT::Update::Pre-Invoke;', '#clear APT::Update::Post-Invoke;',
        '#clear APT::Update::Post-Invoke-Success;'])
    BUILD.write_new(work / 'apt.conf', ('\n'.join(config) + '\n').encode('ascii'))
    return work / 'apt.conf'


def solver_worker(config, evidence, baseline, operation, seeds):
    require(native_arch() in BUILD.ARCHES, 'native solver required')
    inode = os.stat('/proc/self/ns/net').st_ino
    require(inode != baseline and all(name == 'lo' for _, name in socket.if_nameindex()), 'solver network isolation failed')
    evidence = Path(evidence).absolute()
    BUILD.private_directory(evidence.parent)
    require(operation in ('update', 'plan'), 'invalid internal solver operation')
    require(all(BUILD.NAME.fullmatch(seed) for seed in seeds), 'invalid internal solver seed')
    require(operation != 'update' or not seeds, 'update must not take package seeds')
    receipt = {'schema': 1, 'host_namespace_inode': baseline, 'solver_namespace_inode': inode,
               'different_namespace': True, 'interfaces': ['lo'], 'operation': operation,
               'configuration': str(config), 'installation': False}
    BUILD.write_new(evidence, BUILD.canonical(receipt) + b'\n')
    argv = [APTPATH, '--quiet', '--yes']
    argv += ['update'] if operation == 'update' else ['--print-uris', '--download-only', '--no-remove', 'install', *seeds]
    os.execve(APTPATH, argv, dict(BASE_ENV, APT_CONFIG=str(config), HOME=str(evidence.parent)))


def map_print_uris(content, mirrors, indices, arch, deadline=None):
    deadline = deadline or BUILD.Deadline(60)
    wanted, selections = {}, []
    for line in content.decode('utf-8').splitlines():
        if not line.startswith("'"):
            continue
        # APT may omit the display checksum when Packages has no MD5Sum.
        # Only the signed Packages SHA256 and size below authenticate a file.
        match = re.fullmatch(r"'([^']+)'\s+\S+\s+([0-9]+)(?:\s+\S+)?\s*", line)
        require(match is not None, 'unrecognized APT URI record')
        url = urllib.parse.urlsplit(match[1])
        require(url.scheme == 'file' and not url.netloc and not url.query and not url.fragment, 'solver selected a network URI')
        decoded = urllib.parse.unquote(url.path, errors='strict')
        matches = []
        for repo, mirror in mirrors:
            prefix = str(mirror) + '/'
            if decoded.startswith(prefix):
                filename = pool_path(repo, decoded[len(prefix):])
                matches.append((repo['id'], filename, match[2]))
        require(len(matches) == 1, 'APT selected outside the owned mirrors')
        repository, filename, size = matches[0]
        key = (repository, filename)
        require(key not in wanted, 'APT printed a duplicate package URI')
        wanted[key] = size
    require(wanted and len(wanted) <= 2048, 'APT produced no bounded complete selection')
    found = set()
    for repo, _mirror in mirrors:
        for row in iter_rows(indices[repo['id']]['Packages'], deadline):
            key = (repo['id'], row.get('Filename'))
            if key not in wanted:
                continue
            require(key not in found, 'APT filename has no unique signed package identity')
            require(row.get('Architecture') in (arch, 'all') and row.get('Size') == wanted[key]
                    and BUILD.SHA256.fullmatch(row.get('SHA256', '')), 'APT selection differs from signed native/all package')
            source_name, source_version = BUILD.source_pair(row)
            selections.append({'repository': repo['id'], 'name': row['Package'], 'version': row['Version'],
                'architecture': row['Architecture'], 'filename': key[1], 'sha256': row['SHA256'],
                'size': int(row['Size']), 'source_name': source_name, 'source_version': source_version})
            found.add(key)
    require(found == set(wanted), 'APT URI has no matching authenticated package')
    require(selections and len(selections) <= 2048, 'APT produced no bounded complete selection')
    require(len({row['name'] for row in selections}) == len(selections), 'APT selected duplicate package names')
    return sorted(selections, key=lambda row: row['name'])


def source_closure(packages, repositories, indices, deadline=None):
    deadline = deadline or BUILD.Deadline(60)
    wanted = {(row['source_name'], row['source_version']) for row in packages}
    candidates = {pair: [] for pair in wanted}
    for repo in repositories:
        found = set()
        for row in iter_rows(indices[repo['id']]['Sources'], deadline):
            pair = (row.get('Package'), row.get('Version'))
            if pair in wanted:
                require(pair not in found, 'ambiguous signed source index')
                found.add(pair)
                directory = pool_path(repo, row.get('Directory'))
                files = BUILD.checksum_rows(row.get('Checksums-Sha256', ''))
                require(any(path.endswith('.dsc') for path in files) and len(files) <= 64
                        and all('/' not in path for path in files), 'incomplete or unsafe corresponding source files')
                candidates[pair].append({'repository': repo['id'], 'name': pair[0], 'version': pair[1],
                                        'directory': directory, 'files': files})
    result = []
    for pair, matches in sorted(candidates.items()):
        require(matches, 'corresponding source is absent from every signed repository')
        identities = {BUILD.canonical(row['files']) for row in matches}
        require(len(identities) == 1, 'conflicting corresponding source across signed repositories')
        chosen = matches[0]
        chosen['files'] = [dict(value, name=path) for path, value in sorted(chosen['files'].items())]
        result.append(chosen)
    return result


def payload_capacity(rows, suffix):
    blobs = {}
    total = 0
    for row in rows:
        value = {key: row[key] for key in ('sha256', 'size')}
        blob = row['sha256'] + suffix
        require(blob not in blobs or blobs[blob] == value, 'conflicting planned payload blob identity')
        blobs[blob] = value
        total += row['size']
    return {'file_references': len(rows), 'reference_bytes': total,
            'unique_blobs': len(blobs), 'unique_blob_bytes': sum(value['size'] for value in blobs.values()),
            'cache_suffix': suffix}


def capacity_bytes(plan, deadline):
    chunks, total = [], 1
    encoder = json.JSONEncoder(ensure_ascii=True, sort_keys=True, separators=(',', ':'))
    for text in encoder.iterencode(plan):
        deadline.check()
        chunk = text.encode('ascii')
        total += len(chunk)
        require(total <= MAX_RECEIPT, 'capacity plan exceeds its reader limit')
        chunks.append(chunk)
    return b''.join(chunks) + b'\n'


def write_capacity_plan(collector, materials, solver):
    """Record signed-index identities before the existing conservative admission."""
    BUILD.validate_materials(materials)
    require(isinstance(solver, dict), 'APT capacity report is missing')
    apt = solver.get('capacity')
    require(isinstance(apt, dict) and set(apt) == {'reserved_expansion_bytes', 'phase_observations', 'derived_directories_removed'},
            'APT capacity observations are missing')
    require(type(apt['reserved_expansion_bytes']) is int and apt['reserved_expansion_bytes'] >= 0
            and apt['derived_directories_removed'] is True, 'invalid APT capacity accounting')
    observations = apt['phase_observations']
    require(isinstance(observations, list) and len(observations) == 2, 'incomplete APT phase observations')
    for operation, row in zip(('update', 'plan'), observations):
        require(isinstance(row, dict) and set(row) == {'operation', 'lists_bytes', 'archives_bytes', 'mirrors_bytes', 'solver_owned_bytes'}
                and row['operation'] == operation
                and all(type(row[key]) is int and row[key] >= 0 for key in row if key != 'operation'),
                'invalid APT phase observation')
        require(sum(row[key] for key in ('lists_bytes', 'archives_bytes', 'mirrors_bytes')) <= row['solver_owned_bytes'],
                'APT phase observation exceeds its owned workspace')
    work = collector.output / 'solver'
    require(all(not (work / name).exists() for name in ('lists', 'archives', 'mirrors')),
            'APT derived files remain before payload admission')
    binary = payload_capacity(materials['packages'], '.deb')
    source_files = [value for row in materials['sources'] for value in row['files']]
    sources = payload_capacity(source_files, '.source')
    conservative = binary['reference_bytes'] + sources['reference_bytes']
    observed_owned = owned_size(collector.output, collector.deadline)
    disk = os.statvfs(collector.output)
    available = disk.f_bavail * disk.f_frsize
    plan = {'schema': 1, 'kind': COLLECTION_KIND + '-capacity-plan', 'arch': materials['arch'],
            'source_epoch': materials['source_epoch'], 'keyring': materials['keyring'], 'repositories': materials['repositories'],
            'observed_at': timestamp(), 'stage': 'before_binary_and_source_payload_download',
            'complete': False, 'payload_authenticated': False, 'builder_approved': False,
            'full_ready': False, 'reproducibility_verified': False,
            'authentication_scope': 'identities selected from authenticated indices; payload bytes not yet authenticated',
            'packages': materials['packages'], 'sources': materials['sources'],
            'binary': binary, 'source': dict(sources, name_version_pairs=len(materials['sources'])),
            'unique_cache_payload_bytes': binary['unique_blob_bytes'] + sources['unique_blob_bytes'],
            'observed_owned_metadata_bytes_before_plan': observed_owned,
            'observed_input_cache_metadata_bytes': owned_size(collector.cache, collector.deadline),
            'observed_retained_solver_bytes': owned_size(work, collector.deadline),
            'apt': apt, 'max_total_bytes': collector.max_total, 'reserve_free_bytes': collector.reserve,
            'observed_available_disk_bytes_before_plan': available,
            'conservative_payload_admission_bytes': conservative,
            'plan_metadata_bytes': 0, 'required_owned_bytes_before_payloads': 0,
            'required_available_disk_bytes_at_observation': 0,
            'admission_at_observation': {'allowed': False, 'rejection_reasons': []},
            'accounting_scope': 'ordinary-file logical bytes; APT observations are phase samples, not a continuous peak',
            'future_overhead_scope': 'future HTTP headers, final inventories, tool records and receipts are excluded; phase budgets still apply'}
    # Include the plan's own bounded bytes in the snapshot admission. Only decimal
    # lengths and decision fields change; refuse a non-converging representation.
    for _ in range(16):
        raw = capacity_bytes(plan, collector.deadline)
        required_owned = observed_owned + len(raw) + conservative
        required_disk = collector.reserve + len(raw) + conservative
        reasons = []
        if required_owned > collector.max_total:
            reasons.append('factory_total_byte_admission')
        if available < required_disk:
            reasons.append('factory_free_disk_reserve')
        updated = dict(plan, plan_metadata_bytes=len(raw), required_owned_bytes_before_payloads=required_owned,
                       required_available_disk_bytes_at_observation=required_disk,
                       admission_at_observation={'allowed': not reasons, 'rejection_reasons': reasons})
        if updated == plan:
            break
        plan = updated
    else:
        raise ValueError('capacity plan byte accounting did not converge')
    require(len(raw) <= MAX_RECEIPT, 'capacity plan exceeds its reader limit')
    # Planning evidence never bypasses the existing total or disk reserve guard.
    collector.budget(len(raw))
    publish(collector.output / 'capacity-plan.json', raw, MAX_RECEIPT)
    return {'path': 'capacity-plan.json', 'sha256': hashlib.sha256(raw).hexdigest(), 'size': len(raw)}


def solve(collector, repositories, indices, keyring, authenticated_expansion=None):
    expansion_plan = None
    if authenticated_expansion is not None:
        # This is an internal, already authenticated repo/path bound map. It
        # is deliberately not accepted as a collection CLI argument or as a
        # replacement for the complete parent package/source authentication.
        capacity_spec = importlib.util.spec_from_file_location('ipquality_solver_expansion',
                HERE.with_name('ipquality-inputs-capacity.py'))
        capacity_helper = importlib.util.module_from_spec(capacity_spec)
        capacity_spec.loader.exec_module(capacity_helper)
        expansion_plan = capacity_helper.expansion_budget(repositories, authenticated_expansion)
    seeds, essentials = essential_seeds(repositories, indices, collector.deadline)
    work = collector.output / 'solver'
    work.mkdir(mode=0o700)
    # Preserve legacy admission exactly unless the offline derivation supplied
    # signed, fully verified uncompressed bounds. The latter reserve covers
    # multiple APT copies and metadata; neither plan is a hard output quota.
    reserved_expansion = (expansion_plan['reserved_expansion_bytes'] if expansion_plan is not None else
            sum(BUILD.MAX_INDEX for repo in repositories for row in repo['indices'] if row['kind'] == 'Packages'))
    collector.budget(reserved_expansion)
    mirrors = []
    for repo in repositories:
        mirror = work / 'mirrors' / repo['id']
        mirror.mkdir(parents=True, mode=0o700)
        files = [(keyring, mirror / 'archive-keyring.gpg'),
                 (collector.cache / repo['inrelease']['blob'], mirror / 'dists' / repo['suite'] / 'InRelease')]
        files += [(collector.cache / row['blob'], mirror / 'dists' / repo['suite'] / row['path'])
                  for row in repo['indices'] if row['kind'] == 'Packages']
        for original, target in files:
            collector.budget(original.stat().st_size + (reserved_expansion if expansion_plan is not None else 0))
            target.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
            shutil.copyfile(original, target)
            os.chmod(target, 0o644)
        mirrors.append((repo, mirror))
    config = apt_config(work, mirrors, collector.arch)
    outputs, commands, space_observations = {}, [], []
    baseline = os.stat('/proc/self/ns/net').st_ino
    for operation in ('update', 'plan'):
        evidence = work / (operation + '-namespace.json')
        argv = ['/usr/bin/unshare', '--net', '--fork', '--kill-child', '/usr/bin/env', '-i',
                *[key + '=' + value for key, value in BASE_ENV.items()], 'APT_CONFIG=' + str(config),
                sys.executable, str(HERE), '_solver', '--config', str(config), '--evidence', str(evidence),
                '--baseline', str(baseline), '--operation', operation]
        if operation == 'plan':
            argv += ['--seeds', *seeds]
        if expansion_plan is not None:
            collector.budget(reserved_expansion)
        outputs[operation] = BUILD.run_bounded(argv,
                BUILD.Deadline(min(180, collector.deadline.remaining()), capacity=collector.deadline.capacity), MAX_RECEIPT)
        BUILD.write_new(work / (operation + '.stdout'), outputs[operation])
        commands.append(argv)
        space_observations.append({'operation': operation,
            'lists_bytes': owned_size(work / 'lists', collector.deadline),
            'archives_bytes': owned_size(work / 'archives', collector.deadline),
            'mirrors_bytes': owned_size(work / 'mirrors', collector.deadline),
            'solver_owned_bytes': owned_size(work, collector.deadline)})
        collector.budget()
    packages = map_print_uris(outputs['plan'], mirrors, indices, collector.arch, collector.deadline)
    require(set(seeds) <= {row['name'] for row in packages}, 'APT selection omitted required or essential packages')
    report = {'schema': 1, 'seeds': seeds, 'main_essential_names': essentials,
              'commands': commands, 'package_selection': packages,
              'installation': False, 'binary_downloads_by_solver': False,
              'network_namespaces': [BUILD.decode(BUILD.read_regular(work / (op + '-namespace.json'), 65536))
                                     for op in ('update', 'plan')],
              'capacity': {'reserved_expansion_bytes': reserved_expansion, 'phase_observations': space_observations,
                           'derived_directories_removed': True},
              'factory_budget_scope': 'owned material checked between phases; external filesystem quota remains separate'}
    if expansion_plan is not None:
        report['authenticated_expansion_admission'] = expansion_plan
        report['factory_budget_scope'] = 'prospective copy admission and polled capacity; no hard output quota or continuous peak assertion'
    # Keep the evidence/configuration, but not derived lists or duplicate mirrors.
    for name in ('lists', 'archives', 'mirrors'):
        shutil.rmtree(work / name)
    publish(work / 'selection.json', BUILD.canonical(report) + b'\n', MAX_RECEIPT)
    collector.budget()
    return packages, report


def collect(request_path, keyring_path, provenance_path, candidate_path, output_path, max_total, seconds, reserve=MIN_FREE):
    require(type(seconds) is int and 0 < seconds <= MAX_SECONDS, 'invalid overall collection timeout')
    deadline = BUILD.Deadline(seconds)
    request_bytes = BUILD.read_regular(request_path, BUILD.MAX_LOCK, deadline)
    request = BUILD.decode(request_bytes)
    imports = validate_request(request)
    require(request['arch'] == native_arch(), 'collection must run on the requested native architecture')
    candidate_bytes = BUILD.read_regular(candidate_path, BUILD.MAX_LOCK, deadline) if candidate_path else None
    candidate = verify_candidate(BUILD.decode(candidate_bytes), request['arch'], deadline) if candidate_bytes else None
    provenance_bytes = BUILD.read_regular(provenance_path, MAX_IMPORTS, deadline)
    provenance = BUILD.decode(provenance_bytes)
    keyring_identity = BUILD.file_identity(keyring_path, BUILD.MAX_LOCK, deadline)
    require(isinstance(provenance, dict) and provenance.get('schema') == 1
            and all(isinstance(provenance.get(field), str) and 0 < len(provenance[field]) <= 4096
                    for field in ('source', 'reviewer', 'obtained_at'))
            and {key: provenance.get(key) for key in ('sha256', 'size')} == keyring_identity,
            'independent keyring acquisition/review record is missing or mismatched')
    complete, output, stage, capacity_identity = False, None, 'metadata_collection', None
    owned_publications = []
    try:
        with BUILD.deferred_signals():
            output = BUILD.reserve_output(output_path)
        collector = Collector(output, max_total, deadline, reserve)
        collector.arch = request['arch']
        collector.budget(len(request_bytes))
        BUILD.write_new(output / 'request.json', request_bytes)
        if candidate_bytes is not None:
            collector.budget(len(candidate_bytes))
            BUILD.write_new(output / 'candidate-builder.json', candidate_bytes)
        collector.budget(len(provenance_bytes))
        BUILD.write_new(output / 'keyring-provenance.json', provenance_bytes)
        keyring = dict(keyring_identity, blob=keyring_identity['sha256'] + '.gpg')
        collector.budget(keyring_identity['size'])
        BUILD.copy_locked(Path(keyring_path).parent, dict(keyring_identity, blob=Path(keyring_path).name),
                          collector.cache / keyring['blob'], BUILD.MAX_LOCK, deadline)
        observed = []
        for archive, chosen in sorted(imports.items()):
            descriptor = collector.obtain(import_url(archive, chosen), MAX_IMPORTS, suffix='.json')
            rows = validate_imports(BUILD.read_regular(collector.cache / descriptor['blob'], MAX_IMPORTS, deadline), archive, chosen)
            observed.append({'archive': archive, 'selected_timestamp': chosen, 'response': descriptor,
                             'imports_in_response': rows, 'authentication_scope': 'official HTTPS discovery, not package signature'})
        repositories, indices = [], {}
        for requested in request['repositories']:
            repo, parsed = authenticate_metadata(collector, requested, collector.cache / keyring['blob'])
            repositories.append(repo)
            indices[repo['id']] = parsed
        stage = 'dependency_selection'
        packages, solver = solve(collector, repositories, indices, collector.cache / keyring['blob'])
        stage = 'source_closure_selection'
        sources = source_closure(packages, repositories, indices, deadline)
        stage = 'capacity_plan_publication'
        planned_materials = dict(request, keyring=keyring, repositories=repositories,
            packages=[dict(row, blob=row['sha256'] + '.deb') for row in packages],
            sources=[dict(row, files=[dict(value, blob=value['sha256'] + '.source') for value in row['files']]) for row in sources])
        capacity_identity = write_capacity_plan(collector, planned_materials, solver)
        stage = 'before_binary_and_source_payload_download'
        remaining = sum(row['size'] for row in packages) + sum(value['size'] for row in sources for value in row['files'])
        collector.budget(remaining)
        stage = 'binary_and_source_payload_download'
        repos = {repo['id']: repo for repo in repositories}
        for package in packages:
            downloaded = collector.obtain(repository_url(repos[package['repository']], package['filename']),
                    BUILD.MAX_ARCHIVE, {key: package[key] for key in ('sha256', 'size')}, '.deb')
            package['blob'] = downloaded['blob']
        for source in sources:
            for value in source['files']:
                downloaded = collector.obtain(repository_url(repos[source['repository']], source['directory'] + '/' + value['name']),
                        BUILD.MAX_SOURCE, {key: value[key] for key in ('sha256', 'size')}, '.source')
                value['blob'] = downloaded['blob']
        materials = dict(request, keyring=keyring, repositories=repositories, packages=packages, sources=sources)
        stage = 'final_source_authentication'
        BUILD.validate_materials(materials)
        inventory, final_signatures = BUILD.verify_authenticated_sources(materials, collector.cache, deadline)
        materials_bytes = BUILD.canonical(materials) + b'\n'
        stage = 'success_materials_publication'
        collector.budget(len(materials_bytes))
        publish_success(output / 'materials.json', materials_bytes, BUILD.MAX_LOCK, owned_publications)
        lock_identity = None
        if candidate is not None:
            lock = dict(materials, builder=candidate)
            BUILD.validate_lock(lock)
            lock_bytes = BUILD.canonical(lock) + b'\n'
            collector.budget(len(lock_bytes))
            publish_success(output / 'inputs-lock.json', lock_bytes, BUILD.MAX_LOCK, owned_publications)
            lock_identity = hashlib.sha256(lock_bytes).hexdigest()
        else:
            unbound_bytes = BUILD.canonical({
                'schema': 1, 'kind': COLLECTION_KIND, 'materials_sha256': hashlib.sha256(materials_bytes).hexdigest(),
                'builder': None, 'lock_ready': False, 'builder_approved': False,
                'full_ready': False, 'reproducibility_verified': False}) + b'\n'
            collector.budget(len(unbound_bytes))
            publish_success(output / 'unbound-inputs.json', unbound_bytes, MAX_RECEIPT, owned_publications)
        stage = 'collection_receipt_preparation'
        receipt = {'schema': 1, 'kind': COLLECTION_KIND, 'complete': True, 'arch': request['arch'], 'collected_at': timestamp(),
            'inputs_lock_sha256': lock_identity, 'materials_sha256': hashlib.sha256(materials_bytes).hexdigest(),
            'lock_ready': candidate is not None, 'builder': candidate, 'imports': observed,
            'signatures': final_signatures, 'downloads': collector.downloads,
            'collection_tools': record_collection_tools(deadline),
            'collector_sha256': BUILD.file_identity(HERE, BUILD.MAX_LOCK, deadline)['sha256'],
            'verification_helper_sha256': BUILD.file_identity(HERE.with_name('nodequality-rootfs-build.py'), BUILD.MAX_LOCK, deadline)['sha256'],
            'candidate_builder_image_sha256': candidate['image_sha256'] if candidate is not None else None,
            'builder_approved': False, 'runtime_image_identity_verified': False,
            'keyring_acquisition_review_supplied': True, 'keyring_trust_independently_verified_by_collector': False,
            'source_authenticated': True, 'authentication_scope': 'Debian inputs under supplied independently reviewed keyring',
            'solver_selection_sha256': BUILD.file_identity(output / 'solver' / 'selection.json', MAX_RECEIPT, deadline)['sha256'],
            'capacity_plan': capacity_identity,
            'full_ready': False, 'reproducibility_verified': False, 'max_total_bytes': max_total,
            'overall_timeout_seconds': seconds, 'reserve_free_bytes': reserve}
        raw_receipt = BUILD.canonical(receipt) + b'\n'
        require(len(raw_receipt) <= MAX_RECEIPT, 'collection receipt exceeds its reader limit')
        collector.budget(len(raw_receipt))
        stage = 'collection_receipt_publication'
        with BUILD.deferred_signals():
            publish_success(output / 'collection.json', raw_receipt, MAX_RECEIPT, owned_publications)
            complete = True
        return receipt
    except BaseException as error:
        complete = False
        publication_cleanup = rollback_success(owned_publications)
        if any(row['removed'] is False for row in publication_cleanup) and hasattr(error, 'add_note'):
            error.add_note('Success metadata rollback was incomplete; changed file identities were preserved')
        if output is not None:
            # Retain a bounded failure receipt and already obtained evidence; never mark it complete.
            with contextlib.suppress(BaseException):
                failure = {'schema': 1, 'kind': COLLECTION_KIND, 'complete': False,
                           'error_type': type(error).__name__, 'error': diagnostic_error_message(error),
                           'failure_stage': stage, 'capacity_plan': capacity_identity,
                           'success_publication_cleanup': publication_cleanup,
                           'notes': [diagnostic_error_message(ValueError(str(note))) for note in getattr(error, '__notes__', [])[:4]],
                           'builder_approved': False,
                           'full_ready': False, 'reproducibility_verified': False,
                           'received_objects': getattr(locals().get('collector'), 'downloads', [])[-512:]}
                failure['failed_download'] = next((row for row in reversed(failure['received_objects'])
                                                   if row.get('complete') is False), None)
                raw_failure = BUILD.canonical(failure) + b'\n'
                local_collector = locals().get('collector')
                require(local_collector is not None, 'failure publication lacks owned collection identity')
                local_collector.failure_budget(raw_failure, BUILD.Deadline(FAILURE_EVIDENCE_SECONDS))
                publish(output / 'failure.json', raw_failure, MAX_RECEIPT)
                if hasattr(error, 'add_note'):
                    error.add_note('Incomplete collection evidence retained at ' + str(output))
        raise
    finally:
        if not complete and output is not None and not (output / 'failure.json').exists():
            # Failed receipt publication must not erase already acquired cache
            # or replace evidence failure with apparent successful cleanup.
            error = sys.exc_info()[1]
            if error is not None and hasattr(error, 'add_note'):
                error.add_note('Incomplete collection directory retained without a complete failure receipt: ' + str(output))


def bind(materials_directory, candidate_path, output_path, seconds):
    require(type(seconds) is int and 0 < seconds <= MAX_SECONDS, 'invalid overall binding timeout')
    deadline = BUILD.Deadline(seconds)
    directory = BUILD.private_directory(materials_directory)
    require(not (directory / 'failure.json').exists(), 'binding refuses an incomplete collection')
    raw = BUILD.read_regular(directory / 'materials.json', BUILD.MAX_LOCK, deadline)
    materials = BUILD.decode(raw)
    BUILD.validate_materials(materials)
    require(materials['arch'] == native_arch(), 'binding requires the matching native architecture')
    old_receipt = BUILD.decode(BUILD.read_regular(directory / 'collection.json', MAX_RECEIPT, deadline))
    require(isinstance(old_receipt, dict) and old_receipt.get('kind') == COLLECTION_KIND
            and old_receipt.get('complete') is True and old_receipt.get('source_authenticated') is True
            and old_receipt.get('materials_sha256') == hashlib.sha256(raw).hexdigest()
            and old_receipt.get('builder_approved') is False, 'collection receipt is missing or inconsistent')
    expected_imports = {repo['archive']: repo['timestamp'] for repo in materials['repositories']}
    observations = old_receipt.get('imports')
    require(isinstance(observations, list) and len(observations) == len(expected_imports), 'binding import pair is incomplete')
    seen_imports = set()
    for observation in observations:
        require(isinstance(observation, dict) and observation.get('archive') in expected_imports
                and observation['archive'] not in seen_imports
                and observation.get('selected_timestamp') == expected_imports[observation['archive']],
                'binding import observation differs from exact repository timestamp')
        seen_imports.add(observation['archive'])
    for observation in observations:
        body = BUILD.checked_blob(directory / 'input-cache', observation['response'], MAX_IMPORTS, deadline)
        validate_imports(BUILD.read_regular(body, MAX_IMPORTS, deadline), observation['archive'], observation['selected_timestamp'])
    inventory, signatures = BUILD.verify_authenticated_sources(materials, directory / 'input-cache', deadline)
    candidate_bytes = BUILD.read_regular(candidate_path, BUILD.MAX_LOCK, deadline)
    candidate = verify_candidate(BUILD.decode(candidate_bytes), materials['arch'], deadline)
    lock = dict(materials, builder=candidate)
    BUILD.validate_lock(lock)
    output, complete, owned_identity = None, False, None
    try:
        with BUILD.deferred_signals():
            output = BUILD.reserve_output(output_path)
            owned_identity = (BUILD.FactoryCapacity.identity(output), BUILD.FactoryCapacity.identity(output.parent))
        raw_lock = BUILD.canonical(lock) + b'\n'
        publish(output / 'inputs-lock.json', raw_lock, BUILD.MAX_LOCK)
        BUILD.write_new(output / 'candidate-builder.json', candidate_bytes)
        receipt = {'schema': 1, 'kind': COLLECTION_KIND + '-binding', 'arch': materials['arch'],
                   'materials_directory': str(directory), 'materials_sha256': hashlib.sha256(raw).hexdigest(),
                   'inputs_lock_sha256': hashlib.sha256(raw_lock).hexdigest(), 'signatures': signatures,
                   'cache_directory': str(directory / 'input-cache'), 'lock_ready': True,
                   'builder_approved': False, 'runtime_image_identity_verified': False,
                   'full_ready': False, 'reproducibility_verified': False}
        with BUILD.deferred_signals():
            publish(output / 'binding.json', BUILD.canonical(receipt) + b'\n', MAX_RECEIPT)
            complete = True
        return receipt
    except BaseException:
        complete = False
        raise
    finally:
        if output is not None and not complete:
            BUILD.cleanup_output(output, guard_mounts=sys.platform == 'linux', expected_identity=owned_identity)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    operations = parser.add_subparsers(dest='operation', required=True)
    public = operations.add_parser('collect')
    for name in ('request', 'keyring', 'keyring-provenance', 'output'):
        public.add_argument('--' + name, type=Path, required=True)
    public.add_argument('--candidate-builder', type=Path)
    public.add_argument('--max-total-bytes', type=int, required=True)
    public.add_argument('--timeout-seconds', type=int, required=True)
    public.add_argument('--reserve-free-bytes', type=int, default=MIN_FREE)
    binding = operations.add_parser('bind')
    binding.add_argument('--materials', type=Path, required=True)
    binding.add_argument('--candidate-builder', type=Path, required=True)
    binding.add_argument('--output', type=Path, required=True)
    binding.add_argument('--timeout-seconds', type=int, required=True)
    fetch = operations.add_parser('_fetch', help=argparse.SUPPRESS)
    fetch.add_argument('--url', required=True)
    fetch.add_argument('--output', type=Path, required=True)
    fetch.add_argument('--headers', type=Path, required=True)
    fetch.add_argument('--receipt', type=Path, required=True)
    fetch.add_argument('--limit', type=int, required=True)
    fetch.add_argument('--reserve-free-bytes', type=int, required=True)
    solver = operations.add_parser('_solver', help=argparse.SUPPRESS)
    solver.add_argument('--config', type=Path, required=True)
    solver.add_argument('--evidence', type=Path, required=True)
    solver.add_argument('--baseline', type=int, required=True)
    solver.add_argument('--operation', choices=('update', 'plan'), required=True, dest='solver_operation')
    solver.add_argument('--seeds', nargs='*', default=[])
    args = parser.parse_args()
    if args.operation == '_fetch':
        started = time.monotonic()
        context = {}
        try:
            result = fetch_worker(args.url, args.output, args.limit, args.headers, args.reserve_free_bytes, context)
        except BaseException as error:
            result = fetch_error(error, args.url, started, context)
            BUILD.write_new(args.receipt, BUILD.canonical(result) + b'\n')
            if isinstance(error, urllib.error.HTTPError):
                error.close()
            raise
        result.update(complete=True, elapsed_seconds=time.monotonic() - started)
        BUILD.write_new(args.receipt, BUILD.canonical(result) + b'\n')
    elif args.operation == '_solver':
        solver_worker(args.config, args.evidence, args.baseline, args.solver_operation, args.seeds)
        return
    elif args.operation == 'bind':
        result = bind(args.materials, args.candidate_builder, args.output, args.timeout_seconds)
    else:
        result = collect(args.request, args.keyring, args.keyring_provenance, args.candidate_builder,
                         args.output, args.max_total_bytes, args.timeout_seconds, args.reserve_free_bytes)
    print(json.dumps(result, sort_keys=True))


if __name__ == '__main__':
    try:
        with BUILD.cli_signals():
            main()
    except (OSError, ValueError, subprocess.SubprocessError, urllib.error.URLError) as error:
        raise SystemExit('Collection error: ' + str(error)) from None
