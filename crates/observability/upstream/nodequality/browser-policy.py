"""Use the native curl identity in the two fixed IP/network script shells."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
SOURCES = {
    'ip.sh': {
        'source_sha256': 'd6349c7667235a491de09ce6c949bdc407037788bab826dc6a9a3793ba8dc79f',
        'patched_sha256': '6460df3e915dbfda81091bcea12f42d1912ff87e839bb91d7ddaf2baa52f597d',
    },
    'net.sh': {
        'source_sha256': 'bd365128d871bbee48dcf18ac17d961ca67912e1bcc4c01c6ecb8dbcd9f7bcd4',
        'patched_sha256': 'ca28c78c339c3b2a8753dd638ab662fea2c5d2577daf117c6f4236b5ffc26a01',
    },
}
WRAPPER = r'''# Apsp modification (2026-10-01): keep curl's native identity.
APSP_NATIVE_CURL=$(type -P curl) || { printf '%s\n' 'Apsp: native curl is required.' >&2; exit 70; }
readonly APSP_NATIVE_CURL
export APSP_NATIVE_CURL
apsp_browser_header(){
local name="${1%%[:;]*}"
case "${name,,}" in user-agent|sec-ch-ua|sec-ch-ua-*|sec-fetch-*) return 0;; *) return 1;; esac
}
curl(){
local header
local -a arguments=()
while [[ $# -gt 0 ]];do
case "$1" in
--user-agent|-A)
[[ $# -ge 2 ]] || { printf '%s\n' 'Apsp: curl user-agent value missing.' >&2; return 2; }
shift 2;;
--user-agent=*|-A?*) shift;;
--header|-H)
[[ $# -ge 2 ]] || { printf '%s\n' 'Apsp: curl header value missing.' >&2; return 2; }
header=$2
[[ $header != @* ]] || { printf '%s\n' 'Apsp: curl header files are not allowed.' >&2; return 70; }
apsp_browser_header "$header" || arguments+=("$1" "$header")
shift 2;;
--header=*|-H?*)
if [[ $1 == --header=* ]];then header=${1#*=};else header=${1#-H};fi
[[ $header != @* ]] || { printf '%s\n' 'Apsp: curl header files are not allowed.' >&2; return 70; }
apsp_browser_header "$header" || arguments+=("$1")
shift;;
--config|--config=*|-K|-K?*)
printf '%s\n' 'Apsp: additional curl configuration is not allowed.' >&2; return 70;;
--) arguments+=("$@");break;;
*) arguments+=("$1");shift;;
esac
done
# -q must be first: a curlrc must not reintroduce a browser identity.
"$APSP_NATIVE_CURL" -q "${arguments[@]}"
}
export -f apsp_browser_header curl
'''.encode()
REPLACEMENTS = [
    (b'check_bash\n', b'check_bash\n' + WRAPPER),
    (b'\ngenerate_random_user_agent\n', b"\nUA_Browser='' # Apsp: browser identity generation is disabled.\n"),
]


def patch(content):
    for before, after in REPLACEMENTS:
        if content.count(before) != 1:
            raise ValueError('browser policy requires unique anchors')
        content = content.replace(before, after, 1)
    return content


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown browser policy role or source byte limit exceeded')
    identity = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != identity['source_sha256']:
        raise ValueError('browser policy input SHA256 mismatch')
    result = patch(content)
    if len(result) > MAX_SOURCE + 4096 or hashlib.sha256(result).hexdigest() != identity['patched_sha256']:
        raise ValueError('browser policy output SHA256 or byte limit mismatch')
    return result
