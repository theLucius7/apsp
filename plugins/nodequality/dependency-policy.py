#!/usr/bin/env python3
"""Replace runtime dependency installers in exact pinned scripts with checks."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
CHECK_CALL = b'[[ mode_no -eq 0 ]]&&install_dependencies 1>&2\n'
REQUIRED_CALL = b'install_dependencies 1>&2 # Apsp: -n cannot bypass prerequisites.\n'
ENTRY_REPLACEMENTS = [
    (b'    chroot_run wget https://github.com/nxtrace/NTrace-core/releases/download/v1.3.7/nexttrace_linux_amd64 -qO /usr/local/bin/nexttrace\n',
     b'    chroot_run test -x /usr/local/bin/nexttrace || { printf "%s\\n" "Error: missing offline dependency: nexttrace; runtime installation is disabled" >&2; return 70; }\n'),
    (b'    chroot_run chmod u+x /usr/local/bin/nexttrace\n',
     b'    : # Apsp: the prepared trace executable must already be executable.\n'),
    (b'    load_3rd_program\n', b'    load_3rd_program || exit $? # Apsp: reject missing offline tools.\n'),
]
for function, filename in (
    ('run_ip_quality', 'ip_quality_filename'),
    ('run_net_quality', 'net_quality_filename'),
    ('run_net_trace', 'backroute_trace_filename'),
):
    command = function + ' | tee $result_directory/$' + filename
    ENTRY_REPLACEMENTS.append((('        ' + command + '\n').encode(),
        ('        (set -o pipefail; ' + command + ') || exit $? # Apsp: preserve chapter failure.\n').encode()))

END_MARKERS = {
    'hardware.sh': b'adaptoslocale(){\n',
    'ip.sh': b'declare -A browsers=(\n',
    'net.sh': b'declare -A browsers=(\n',
}
INSTALLERS = {
    'hardware.sh': ('install_packages', 'install_geekbench5', 'install_curl_impersonate'),
    'ip.sh': ('install_packages',),
    'net.sh': ('install_packages', 'install_speedtest', 'install_stun'),
}
REQUIREMENTS = {
    'hardware.sh': ('tar', 'jq', 'curl', 'bc', 'dmidecode', 'sensors', 'lspci', 'lscpu', 'smartctl', 'fio'),
    'ip.sh': ('jq', 'curl', 'bc', 'nc', 'dig'),
    'net.sh': ('jq', 'curl', 'bc', 'convert', 'mtr', 'iperf3', 'stun', 'free', 'nexttrace', 'speedtest'),
}
SOURCES = {
    'NodeQuality.sh': {'source_sha256': 'f7355a4ca5f4042a535e7a73ac337dc427e8f6ca5bdd7e9fb96181fc3162d91b', 'patched_sha256': '4ead63717f9860999017197e5b4e7e4a066d4f8b54f34d15895690e22c7ec15a'},
    'hardware.sh': {'source_sha256': 'e1f90cb9eaf80098a48beb008187b422cb664fc32cf2a4ee306b398ee473567c', 'patched_sha256': '82b18e8eef943a4acfcfb3724fb6b0526ac1b769d8c3591839e89d50f82f562c'},
    'ip.sh': {'source_sha256': '176295d6bc9803d19c1794b73f5c801fe8325f076c60f2114cc943a4b62e68dc', 'patched_sha256': 'f7d7bb8ebce65795f0d54848d9897b5c887cb5eda61a65f5a1df2bf3c43deff9'},
    'net.sh': {'source_sha256': 'f35836caa8e598f443c7b718daa71211cee393534b03f2f3b3d3062a6a3a90b2', 'patched_sha256': 'b0596e02786b1ae6d1dbedd4af10df6b120135715481dec2a9e75d4eca55cfda'},
}


def replace_once(content, before, after):
    if content.count(before) != 1:
        raise ValueError('exact dependency policy anchor must occur exactly once')
    return content.replace(before, after, 1)


def installer_span(role, content):
    begin = b'install_dependencies(){\n'
    end = END_MARKERS[role]
    if content.count(begin) != 1 or content.count(end) != 1:
        raise ValueError('dependency installer boundaries must occur exactly once')
    start, stop = content.index(begin), content.index(end)
    if stop <= start:
        raise ValueError('dependency installer boundaries are reversed')
    return start, stop


def checks(role):
    text = '''# Apsp modification (2026-10-01): dependency installation is forbidden.
# Presence checks do not prove a tool's source, version, license or integrity.
install_dependencies(){
local missing=() tool
local required=(''' + ' '.join(REQUIREMENTS[role]) + ''')
'''
    if role == 'hardware.sh':
        text += '''[[ ${mode_fast:-0} -ne 0 ]] || required+=(sysbench)
if [[ ${mode_fast:-0} -eq 0 && ${mode_privacy:-0} -eq 0 ]];then
required+=(geekbench5)
[[ ${mode_verbose:-0} -ne 1 ]] || required+=(curl-impersonate)
fi
[[ ${mode_verbose:-0} -ne 1 ]] || required+=(update-ca-certificates)
'''
    if role == 'net.sh':
        text += 'usesudo="" # The Apsp Linux wrapper requires root.\n'
    text += '''for tool in "${required[@]}";do
command -v "$tool" >/dev/null 2>&1 || missing+=("$tool")
done
if (( ${#missing[@]} ));then
printf 'Error: missing offline dependencies: %s; runtime installation is disabled\\n' "${missing[*]}" >&2
exit 70
fi
return 0
}
'''
    for installer in INSTALLERS[role]:
        text += installer + '''(){
printf '%s\\n' 'Error: runtime dependency installation is disabled' >&2
exit 70
}
'''
    return text.encode()


def patch(role, content):
    if role == 'NodeQuality.sh':
        for before, after in ENTRY_REPLACEMENTS:
            content = replace_once(content, before, after)
        return content
    if role not in END_MARKERS:
        raise ValueError('unknown dependency policy role')
    start, stop = installer_span(role, content)
    content = content[:start] + checks(role) + content[stop:]
    return replace_once(content, CHECK_CALL, REQUIRED_CALL)


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown dependency role or source byte limit exceeded')
    identity = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != identity['source_sha256']:
        raise ValueError('dependency policy input SHA256 mismatch')
    result = patch(role, content)
    if len(result) > MAX_SOURCE + 4096 or hashlib.sha256(result).hexdigest() != identity['patched_sha256']:
        raise ValueError('dependency policy output SHA256 or byte limit mismatch')
    return result
