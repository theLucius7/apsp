"""Reject failed transfers and unrecognized pages in the pinned Netflix check."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
SOURCES = {
    'ip.sh': {
        'source_sha256': 'f434c87f920cc9594aca3978b58e28dcaf786c074d70d64475f567068d093c4d',
        'patched_sha256': '1ef0b1174230d3497f6bc880263356f0481427bc5b4e1e6b9fea818e81903d83',
    },
}

HELPERS = r'''# Apsp modification (2026-10-01): a transport error is not an unlock result.
apsp_netflix_response_error(){
local code="$1" response="$2" title="$3" status body LC_ALL=C
status=${response##*$'\n'}
body=${response%$'\n'*}
if [[ $code != 0 ]];then
    if [[ $status =~ ^[45][0-9]{2}$ ]];then
        printf 'HTTP %s' "$status"
    else
        case "$code" in
            6) printf 'DNS 解析失败' ;;
            7) printf '连接失败' ;;
            18) printf '响应未传输完整' ;;
            28) printf '请求超时' ;;
            35|51|58|60|77|83|90|91) printf 'TLS 验证或握手失败' ;;
            63) printf '响应超过大小限制' ;;
            *) printf '请求失败（curl %s）' "$code" ;;
        esac
    fi
    return 0
fi
if [[ $response != *$'\n'* || ! $status =~ ^[0-9]{3}$ ]];then
    printf '未取得有效 HTTP 状态'
elif [[ $status != 200 ]];then
    printf 'HTTP %s' "$status"
elif [[ -z ${body//[[:space:]]/} ]];then
    printf '响应正文为空'
elif [[ ${#body} -gt 2097152 ]];then
    printf '响应超过大小限制'
elif ! printf '%s' "$body" | grep -Ei '<html([[:space:]]|>)' >/dev/null \
    || ! printf '%s' "$body" | grep -Ei '</html[[:space:]]*>' >/dev/null \
    || ! printf '%s' "$body" | grep -Ei '<title[^>]*>[^<]*Netflix[^<]*</title>' >/dev/null ;then
    printf '未识别的 Netflix 页面'
elif [[ $body == *'Oh no!'* ]];then
    # Preserve the pinned unavailable-title interpretation, not arbitrary errors.
    if ! printf '%s' "$body" | grep -E '<h1[^>]*>[[:space:]]*Oh no![[:space:]]*</h1>' >/dev/null;then
        printf '未识别的 Netflix 页面'
    fi
elif ! printf '%s' "$body" | grep -E 'https://www\.netflix\.com/([[:alpha:]]{2}(-[[:alpha:]]{2})?/)?title/'"$title"'([^0-9]|$)' >/dev/null \
    || ! printf '%s' "$body" | grep -E '"@type"[[:space:]]*:[[:space:]]*"(Movie|TVSeries|TVSeason|TVEpisode|VideoObject)"' >/dev/null;then
    # New/challenge/login/error page shapes stay unknown instead of succeeding.
    printf '未识别的 Netflix 影片页面'
fi
return 0
}
'''.encode()

REQUESTS = b'''local result1=$(curl $CurlARG -$1 --user-agent "$UA_Browser" -fsL -X GET --max-time 10 --tlsv1.3 "https://www.netflix.com/title/81280792" 2>&1)
local result2=$(curl $CurlARG -$1 --user-agent "$UA_Browser" -fsL -X GET --max-time 10 --tlsv1.3 "https://www.netflix.com/title/70143836" 2>&1)
'''
CHECKED_REQUESTS = r'''local result1 result2 code1=0 code2=0 reason1 reason2
# Keep declarations separate: `local value=$(curl ...)` masks the exit status.
# These are two distinct title checks, not retries. Never parse stderr as HTML.
result1=$(curl $CurlARG -$1 --user-agent "$UA_Browser" -fsL -X GET --max-time 10 --tlsv1.3 --max-filesize 2097152 --write-out $'\n%{http_code}' "https://www.netflix.com/title/81280792" 2>/dev/null) || code1=$?
result2=$(curl $CurlARG -$1 --user-agent "$UA_Browser" -fsL -X GET --max-time 10 --tlsv1.3 --max-filesize 2097152 --write-out $'\n%{http_code}' "https://www.netflix.com/title/70143836" 2>/dev/null) || code2=$?
reason1=$(apsp_netflix_response_error "$code1" "$result1" 81280792)
reason2=$(apsp_netflix_response_error "$code2" "$result2" 70143836)
if [[ -n $reason1 || -n $reason2 ]];then
netflix[ustatus]="${smedia[bad]}"
netflix[uregion]="${smedia[nodata]}"
netflix[utype]="${smedia[nodata]}"
[[ -z $reason1 ]] || netflix[reason]="影片 81280792：$reason1"
if [[ -n $reason2 ]];then
    [[ -z ${netflix[reason]} ]] || netflix[reason]+='；'
    netflix[reason]+="影片 70143836：$reason2"
fi
return 0
fi
result1=${result1%$'\n'*}
result2=${result2%$'\n'*}
'''.encode()

REGION = rb'''[[ -n $region ]]&&region=$(echo "$result2"|sed -n 's/.*"id":"\([^"]*\)".*"countryName":"[^"]*".*/\1/p'|head -n1)
'''
DISPLAY = b'''echo -ne "\\r$Font_Cyan${smedia[type]}${tiktok[utype]}${disney[utype]}${netflix[utype]}${youtube[utype]}${amazon[utype]}${reddit[utype]}${chatgpt[utype]}$Font_Suffix\\n"
'''
JSON_TYPE = b'''media_updates+=".Media |= . * { Netflix: { Type: \\"$(clean_ansi "${netflix[utype]:-null}")\\" } } | "
'''
REPLACEMENTS = [
    (b'function MediaUnlockTest_Netflix(){\n', HELPERS + b'function MediaUnlockTest_Netflix(){\n'),
    (REQUESTS, CHECKED_REQUESTS),
    (REGION, REGION + b'[[ $region =~ ^[A-Za-z]{2}$ ]] || region="" # Apsp: do not serialize unchecked page data.\n'),
    (DISPLAY, DISPLAY + '[[ -z ${netflix[reason]} ]] || printf \'Netflix：%s\\n\' "${netflix[reason]}"\n'.encode()),
    (JSON_TYPE, JSON_TYPE + b'''media_updates+=".Media |= . * { Netflix: { Reason: $(jq -cn --arg value "${netflix[reason]:-}" '$value') } } | "
'''),
]


def patch(content):
    for before, after in REPLACEMENTS:
        if content.count(before) != 1:
            raise ValueError('Netflix policy requires unique anchors')
        content = content.replace(before, after, 1)
    return content


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown Netflix role or source byte limit exceeded')
    identity = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != identity['source_sha256']:
        raise ValueError('Netflix policy input SHA256 mismatch')
    result = patch(content)
    if len(result) > MAX_SOURCE + 8192 or hashlib.sha256(result).hexdigest() != identity['patched_sha256']:
        raise ValueError('Netflix policy output SHA256 or byte limit mismatch')
    return result
