"""Gate percentile-score submission while retaining every local benchmark."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
SOURCES = {
    'hardware.sh': {
        'source_sha256': 'eb81ac33d47422290e1df815ef049ef9ac26a213c0bb8353087c4b37e7bd7a6b',
        'patched_sha256': '13112a65c60a8919d90e4f4b8484b4a525cf5121cf94a0bf505faa3350950ca7',
    },
}
REPLACEMENTS = [
    (b'get_mark(){\n',
     b"get_mark(){\nunset 'markinfo[cpu_pct]' 'markinfo[gpu_pct]' 'markinfo[mem_pct]' 'markinfo[disk_pct]' 'markinfo[total_pct]' # Apsp: no stale percentiles.\n"),
    (b'markinfo[count]="$count"\n',
     b'markinfo[count]="$count"\n[[ $apsp_public_report_upload == true ]] || return 0 # Apsp: keep local scores; decline percentile upload.\n'),
    (b'echo -ne "\\r${smark[title]}\\n"\n',
     b'echo -ne "\\r${smark[title]}\\n"\n[[ $apsp_public_report_upload == true ]] || printf \'%s\\n\' \'\xe7\x99\xbe\xe5\x88\x86\xe4\xbd\x8d\xe6\x9c\xaa\xe7\x9f\xa5\xef\xbc\x9a\xe6\x9c\xaa\xe5\x85\x81\xe8\xae\xb8\xe4\xb8\x8a\xe4\xbc\xa0\xe6\x9c\xac\xe6\x9c\xba\xe8\xaf\x84\xe5\x88\x86\xef\xbc\x9b\xe6\x9c\xac\xe5\x9c\xb0\xe8\xaf\x84\xe5\x88\x86\xe5\xb7\xb2\xe4\xbf\x9d\xe7\x95\x99\xe3\x80\x82\'\n'),
    (b'--arg disk_pct "${markinfo[disk_pct]:-}" \\\n',
     b'--arg disk_pct "${markinfo[disk_pct]:-}" \\\n--arg percentile_upload "$apsp_public_report_upload" \\\n'),
    (b'      disk_pct:     num($disk_pct)\n',
     b'      disk_pct:     num($disk_pct),\n      percentile_upload_allowed: ($percentile_upload == "true")\n'),
]


def patch(content):
    for before, after in REPLACEMENTS:
        if content.count(before) != 1:
            raise ValueError('ranking policy requires unique anchors')
        content = content.replace(before, after, 1)
    return content


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown ranking role or source byte limit exceeded')
    identity = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != identity['source_sha256']:
        raise ValueError('ranking policy input SHA256 mismatch')
    result = patch(content)
    if len(result) > MAX_SOURCE + 4096 or hashlib.sha256(result).hexdigest() != identity['patched_sha256']:
        raise ValueError('ranking policy output SHA256 or byte limit mismatch')
    return result
