"""Bound the exact r16 Netflix transfers and reject unrecognized title pages."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
SOURCES = {
    'ip.sh': {
        'source_sha256': 'ec5162dcc8dda76a6251aa6b8940e7973732d71c9fa50def515bb3c92552274b',
        'patched_sha256': 'd3828e31ea16d8a4422519c050a8a976bb353467952a5a196c331963fc20ec3d',
    },
}

HELPERS = r'''# Apsp modification (2026-10-01): recognize only complete Netflix title pages.
apsp_netflix_page_error(){
local body="$1" title="$2" lower LC_ALL=C
lower=${body,,}
# Read all bounded input instead of grep -q: its early exit can cause SIGPIPE
# in the printf producer under pipefail.
if ! printf '%s' "$body" | grep -Ei '<html([[:space:]]|>)' >/dev/null \
    || ! printf '%s' "$body" | grep -Ei '</html[[:space:]]*>' >/dev/null \
    || ! printf '%s' "$body" | grep -Ei '<title[^>]*>[^<]*Netflix[^<]*</title>' >/dev/null ;then
    printf '未识别的 Netflix 页面'
elif [[ ( $lower == *'<form'* && ( $lower == *'action="/login'* || $lower == *'action="https://www.netflix.com/login'* || $lower == *'action="/signin'* ) ) ||
        $lower == *'id="challenge-form"'* || $lower == *'id="captcha-form"'* || $lower == *'class="g-recaptcha"'* ]];then
    printf '登录或挑战页面，影片可用性未知'
elif [[ $body == *'Oh no!'* ]];then
    # Keep the fixed unavailable-title interpretation, not arbitrary errors.
    if ! printf '%s' "$body" | grep -E '<h1[^>]*>[[:space:]]*Oh no![[:space:]]*</h1>' >/dev/null;then
        printf '未识别的 Netflix 页面'
    fi
elif ! printf '%s' "$body" | grep -E 'https://www\.netflix\.com/([[:alpha:]]{2}(-[[:alpha:]]{2})?/)?title/'"$title""([/?#\"'[:space:]<>]|\$)" >/dev/null \
    || ! printf '%s' "$body" | grep -E '"@type"[[:space:]]*:[[:space:]]*"(Movie|TVSeries|TVSeason|TVEpisode|VideoObject)"' >/dev/null;then
    # A valid transfer alone is insufficient: require this title and a movie
    # schema, while preserving the existing availability heuristic.
    printf '未识别的 Netflix 影片页面'
fi
return 0
}
'''.encode()

FETCH = r'''apsp_netflix_fetch(){
local response code http elapsed body category='' reason='' attempted url
url="https://www.netflix.com/title/$2"
attempted=$(date +%s)
if response=$(curl $CurlARG -$1 -fsL -X GET --max-time 10 --tlsv1.3 --write-out $'\n%{http_code}\n%{time_total}' "$url" 2>/dev/null);then code=0;else code=$?;fi
elapsed=${response##*$'\n'}
response=${response%$'\n'*}
http=${response##*$'\n'}
body=${response%$'\n'*}
[[ $http =~ ^[0-9]{3}$ ]] || http=000
[[ $elapsed =~ ^[0-9]+([.][0-9]+)?$ ]] || elapsed=''
case "$http" in
403) category=http_403;reason='HTTP 403：来源拒绝请求';;
429) category=http_429;reason='HTTP 429：来源限流';;
esac
if [[ -z $category ]];then
case "$code" in
6) category=dns;reason='DNS 解析失败';;
7) category=connection;reason='连接失败';;
28) category=timeout;reason='请求超时';;
35|51|58|60|77) category=tls;reason='TLS 校验或握手失败';;
0) if [[ $http != 200 ]];then category=http_status;reason="非预期 HTTP $http";
   elif [[ -z $body ]];then category=empty_response;reason='响应为空';fi;;
*) category=transport;reason="请求失败（curl $code）";;
esac
fi
netflix[attempts]=$(jq -cn --argjson prior "${netflix[attempts]:-[]}" --arg target_ip "$IP" --arg url "$url" --arg attempted_at "$attempted" --arg elapsed_seconds "$elapsed" --arg http_status "$http" --argjson curl_exit "$code" --arg error "$category" '$prior + [{target_ip:$target_ip,url:$url,attempted_at:($attempted_at|tonumber),elapsed_seconds:(if $elapsed_seconds == "" then null else ($elapsed_seconds|tonumber) end),http_status:(if $http_status == "000" then null else ($http_status|tonumber) end),curl_exit:$curl_exit,error:(if $error == "" then null else $error end)}]')
if [[ -n $category ]];then apsp_netflix_unknown "$category" "$reason";return 1;fi
printf -v "$3" '%s' "$body"
}
'''.encode()

CHECKED_FETCH = r'''apsp_netflix_fetch(){
local capture encoded reader response code='' http='' elapsed='' body='' category='' reason='' attempted url size nonzero body_size
local producer_status head_status encoding_status exit_line http_line time_line
url="https://www.netflix.com/title/$2"
attempted=$(date +%s)
# Bound stdout before command substitution, independently of Content-Length
# and curl version. Base64 preserves raw byte length, including NUL. The
# pipeline waits for all producers; a paused peer retains the ten-second curl
# deadline. Reaching the cap never invents a completed curl result.
capture=$(
if {
if curl $CurlARG -$1 -fsL -X GET --max-time 10 --tlsv1.3 --max-filesize 2097152 --write-out $'\nAPSP_HTTP:%{http_code}\nAPSP_TIME:%{time_total}' "$url" 2>/dev/null;then producer_code=0;else producer_code=$?;fi
printf '\nAPSP_CURL_EXIT:%s\n' "$producer_code" 2>/dev/null
} 2>/dev/null | head -c 2097280 | base64;then
pipeline_status=("${PIPESTATUS[@]}")
else
pipeline_status=("${PIPESTATUS[@]}")
fi
printf '\nAPSP_READER:%s:%s:%s' "${pipeline_status[0]}" "${pipeline_status[1]}" "${pipeline_status[2]}"
)
reader=${capture##*$'\n'}
encoded=${capture%$'\n'*}
if [[ $reader =~ ^APSP_READER:([0-9]+):([0-9]+):([0-9]+)$ ]];then
producer_status=${BASH_REMATCH[1]}
head_status=${BASH_REMATCH[2]}
encoding_status=${BASH_REMATCH[3]}
else
head_status=1;encoding_status=1;producer_status=1
fi
if [[ $head_status != 0 || $encoding_status != 0 ]];then
category=reader_error;reason='有界响应读取失败'
else
if size=$(set -o pipefail;printf '%s' "$encoded"|base64 --decode|wc -c) && nonzero=$(set -o pipefail;printf '%s' "$encoded"|base64 --decode|tr -d '\000'|wc -c);then
if [[ ! $size =~ ^[[:space:]]*[0-9]+[[:space:]]*$ || ! $nonzero =~ ^[[:space:]]*[0-9]+[[:space:]]*$ ]];then
size=''
category=reader_error;reason='有界响应字节计数失败'
elif [[ $size -ge 2097280 ]];then
category=response_too_large;reason='响应超过 2 MiB 限制；传输未确认完成'
elif [[ $size -ne $nonzero ]];then
category=invalid_response;reason='响应包含二进制数据，信息未知'
elif [[ $producer_status != 0 ]];then
category=incomplete_response;reason='响应未取得完整的传输确认'
else
if response=$(set -o pipefail;printf '%s' "$encoded"|base64 --decode);then
exit_line=${response##*$'\n'}
response=${response%$'\n'*}
time_line=${response##*$'\n'}
response=${response%$'\n'*}
http_line=${response##*$'\n'}
body=${response%$'\n'*}
if [[ $exit_line =~ ^APSP_CURL_EXIT:([0-9]+)$ ]];then code=${BASH_REMATCH[1]};fi
if [[ $time_line =~ ^APSP_TIME:([0-9]+([.][0-9]+)?)$ ]];then elapsed=${BASH_REMATCH[1]};fi
if [[ $http_line =~ ^APSP_HTTP:([0-9]{3})$ ]];then http=${BASH_REMATCH[1]};fi
if [[ -z $code || -z $http || -z $elapsed || $code -gt 255 ]];then
code='';http='';elapsed='';category=incomplete_response;reason='响应未取得完整的传输确认'
fi
else
size='';category=reader_error;reason='有界响应解码失败'
fi
fi
else
size='';category=reader_error;reason='有界响应字节计数失败'
fi
fi
if [[ -z $category ]];then
case "$http" in
403) category=http_403;reason='HTTP 403：来源拒绝请求';;
429) category=http_429;reason='HTTP 429：来源限流';;
esac
fi
if [[ -z $category ]];then
case "$code" in
6) category=dns;reason='DNS 解析失败';;
7) category=connection;reason='连接失败';;
28) category=timeout;reason='请求超时';;
35|51|58|60|77|83|90|91) category=tls;reason='TLS 校验或握手失败';;
18) category=incomplete_response;reason='响应未传输完整';;
63) category=response_too_large;reason='响应超过 2 MiB 限制';;
0) if [[ $http != 200 ]];then category=http_status;reason="非预期 HTTP $http";
   elif [[ -z ${body//[[:space:]]/} ]];then category=empty_response;reason='响应为空或仅有空白';
   elif body_size=$(set -o pipefail;LC_ALL=C printf '%s' "$body"|wc -c);then
     if [[ ! $body_size =~ ^[[:space:]]*[0-9]+[[:space:]]*$ ]];then category=reader_error;reason='有界响应字节计数失败';
     elif [[ $body_size -gt 2097152 ]];then category=response_too_large;reason='响应超过 2 MiB 限制';fi
   else category=reader_error;reason='有界响应字节计数失败';fi;;
*) category=transport;reason="请求失败（curl $code）";;
esac
fi
if [[ -z $category ]];then
if reason=$(apsp_netflix_page_error "$body" "$2");then
[[ -z $reason ]] || category=schema_mismatch
else
category=reader_error;reason='有界页面判别失败'
fi
fi
netflix[attempts]=$(jq -cn --argjson prior "${netflix[attempts]:-[]}" --arg target_ip "$IP" --arg url "$url" --arg attempted_at "$attempted" --arg elapsed_seconds "$elapsed" --arg http_status "$http" --arg captured_bytes "${size:-}" --argjson curl_exit "${code:-null}" --arg error "$category" '$prior + [{target_ip:$target_ip,url:$url,attempted_at:($attempted_at|tonumber),elapsed_seconds:(if $elapsed_seconds=="" then null else ($elapsed_seconds|tonumber) end),http_status:(if $http_status=="" or $http_status=="000" then null else ($http_status|tonumber) end),curl_exit:$curl_exit,captured_bytes:(if $captured_bytes=="" then null else ($captured_bytes|tonumber) end),reader_limit_bytes:2097280,producer_deadline_seconds:10,error:(if $error=="" then null else $error end)}]')
if [[ -n $category ]];then apsp_netflix_unknown "$category" "$reason";return 1;fi
printf -v "$3" '%s' "$body"
}
'''.encode()

CLASSIFIER = r'''result1=$(echo $result1|grep 'Oh no!')
result2=$(echo $result2|grep 'Oh no!')
'''.encode()

CHECKED_CLASSIFIER = r'''# Apsp: a missing unavailable-title marker is expected, even under errexit.
if result1=$(echo $result1|grep 'Oh no!');then :;else result1='';fi
if result2=$(echo $result2|grep 'Oh no!');then :;else result2='';fi
'''.encode()

REPLACEMENTS = [(FETCH, HELPERS + CHECKED_FETCH), (CLASSIFIER, CHECKED_CLASSIFIER)]


def patch(content):
    for before, after in REPLACEMENTS:
        if content.count(before) != 1:
            raise ValueError('Netflix policy requires unique complete fetch boundaries')
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
