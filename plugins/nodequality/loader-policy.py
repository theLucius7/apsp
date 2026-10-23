"""Wait for complete verified source delivery before entering the chroot."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
SOURCES = {
    'NodeQuality.sh': {
        'source_sha256': '34be83427844aba67e4677f1dead79812d5699fbbab08be0a4f074c332efb6fa',
        'patched_sha256': '728d15d923ae030b9caf83cfe38b9bdb690800c9e11feb57c75ce99a7623a182',
    },
}
REQUESTS = (
    ('header', '"$raw_file_prefix/part/header.sh"'),
    ('hardware', 'https://Hardware.Check.Place'),
    ('ip', 'https://IP.Check.Place'),
    ('net', 'https://Net.Check.Place'),
    ('route', 'https://Net.Check.Place'),
)
CALLS = (
    'chroot_run bash <(curl -Ls "$raw_file_prefix/part/header.sh")',
    'curl -Ls https://Hardware.Check.Place | chroot_run "env NQENV=$(printf \'%q\' "$payload") bash -s -- $opt_lang $params -y -o /result/$hardware_quality_json_filename" # HQ预处理',
    'chroot_run bash <(curl -Ls https://IP.Check.Place) $opt_ipv $opt_lang -y -o /result/$ip_quality_json_filename',
    'chroot_run bash <(curl -Ls https://Net.Check.Place) $opt_ipv $opt_lang $params -y -o /result/$net_quality_json_filename',
    'chroot_run bash <(curl -Ls https://Net.Check.Place) $opt_ipv $opt_lang -R -n -S 123 -o /result/$backroute_trace_json_filename',
)


def guarded_call(role, request, original):
    # The source helper bounds and verifies its complete output before writing.
    # A trailing sentinel preserves every newline across command substitution.
    prefix = ('local apsp_source; apsp_source=$(curl -Ls ' + request
              + ' && printf .) || return $?; '
              + '[[ $apsp_source != . ]] || { printf "%s\\n" "Error: empty diagnostic source: '
              + role + '" >&2; return 70; }; apsp_source=${apsp_source%.}; ')
    call = original.replace('curl -Ls ' + request, 'printf \'%s\' "$apsp_source"', 1)
    return prefix + call


REPLACEMENTS = [
    (('    ' + call + '\n').encode(),
     ('    ' + guarded_call(role, request, call) + '\n').encode())
    for (role, request), call in zip(REQUESTS, CALLS)
]
REPLACEMENTS.append((
    b'    run_header > $result_directory/$header_info_filename\n',
    b'    run_header > $result_directory/$header_info_filename || exit $? # Apsp: stop after source refusal.\n',
))


def patch(content):
    for before, after in REPLACEMENTS:
        if content.count(before) != 1 or before.count(b'\n') != after.count(b'\n'):
            raise ValueError('loader policy requires unique line-preserving anchors')
        content = content.replace(before, after, 1)
    return content


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown loader role or source byte limit exceeded')
    identity = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != identity['source_sha256']:
        raise ValueError('loader policy input SHA256 mismatch')
    result = patch(content)
    if len(result) > MAX_SOURCE + 4096 or hashlib.sha256(result).hexdigest() != identity['patched_sha256']:
        raise ValueError('loader policy output SHA256 or byte limit mismatch')
    return result
