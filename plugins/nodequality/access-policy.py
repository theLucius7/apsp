"""Remove borrowed provider credentials from the exact fixed IP script."""
import hashlib
import re

MAX_SOURCE = 2 * 1024 * 1024
SOURCES = {
    'ip.sh': {
        'source_sha256': '66c7f3c662f24bdccbd142acf25d2a176fbe72ff417848ef977aa755d6d38c8e',
        'patched_sha256': 'ec5162dcc8dda76a6251aa6b8940e7973732d71c9fa50def515bb3c92552274b',
    },
}

HELPERS = r'''# Apsp modification (2026-10-01): do not borrow provider credentials.
apsp_access_not_attempted(){
jq -cn --arg provider "$1" --arg target_ip "$IP" --arg checked_at "$(date +%s)" --arg message "$2" '{provider:$provider,status:"unknown",target_ip:$target_ip,checked_at:($checked_at|tonumber),Attempted:false,last_attempt_at:null,elapsed_seconds:null,Error:{category:"credential_not_configured",phase:"not_attempted",message:$message},Attempts:[]}'
}
apsp_access_youtube_metadata(){
jq -cn --arg target_ip "$IP" --arg checked_at "$(date +%s)" --arg status "$1" --arg category "$2" --arg message "$3" --argjson attempts "${youtube[attempts]:-[]}" '{provider:"youtube-public-page",status:$status,target_ip:$target_ip,checked_at:($checked_at|tonumber),Attempted:($attempts|length>0),last_attempt_at:($attempts[-1].attempted_at//null),elapsed_seconds:($attempts[-1].elapsed_seconds//null),Error:(if $category=="" then null else {category:$category,message:$message} end),Attempts:$attempts}'
}
apsp_access_youtube_unknown(){
youtube[ustatus]='未知'
youtube[uregion]="${smedia[nodata]}"
youtube[utype]="${smedia[nodata]}"
youtube[attempts]=$(jq -cn --argjson attempts "${youtube[attempts]:-[]}" --arg category "$1" '$attempts|map(.error=$category)')
youtube[access]=$(apsp_access_youtube_metadata unknown "$1" "$2")
printf 'YouTube：未知（%s）\n' "$2"
}
apsp_access_youtube_fetch(){
local capture encoded reader response code='' http='' elapsed='' body='' category='' reason='' attempted url size nonzero body_size
local producer_status head_status encoding_status exit_line http_line time_line
url='https://www.youtube.com/premium'
attempted=$(date +%s)
# Bound stdout before command substitution, independently of Content-Length
# and curl version. Base64 preserves raw byte length, including NUL. The
# pipeline waits for all producers; a paused peer retains the ten-second curl
# deadline. Reaching the cap never invents a completed curl result.
capture=$(
if {
if curl $CurlARG -$1 -fsSL --max-time 10 --max-filesize 2097152 -H 'Accept-Language: en' --write-out $'\nAPSP_HTTP:%{http_code}\nAPSP_TIME:%{time_total}' "$url" 2>/dev/null;then producer_code=0;else producer_code=$?;fi
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
35|51|58|60|77) category=tls;reason='TLS 校验或握手失败';;
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
youtube[attempts]=$(jq -cn --arg target_ip "$IP" --arg url "$url" --arg attempted_at "$attempted" --arg elapsed_seconds "$elapsed" --arg http_status "$http" --arg captured_bytes "${size:-}" --argjson curl_exit "${code:-null}" --arg error "$category" '[{target_ip:$target_ip,url:$url,attempted_at:($attempted_at|tonumber),elapsed_seconds:(if $elapsed_seconds=="" then null else ($elapsed_seconds|tonumber) end),http_status:(if $http_status=="" or $http_status=="000" then null else ($http_status|tonumber) end),curl_exit:$curl_exit,captured_bytes:(if $captured_bytes=="" then null else ($captured_bytes|tonumber) end),reader_limit_bytes:2097280,producer_deadline_seconds:10,error:(if $error=="" then null else $error end)}]')
if [[ -n $category ]];then apsp_access_youtube_unknown "$category" "$reason";return 1;fi
printf -v "$2" '%s' "$body"
}
'''.encode()

FUNCTION_REPLACEMENTS = {
    'db_ipregistry': r'''db_ipregistry(){
ipregistry=()
ipregistry[susetype]='未知'
ipregistry[scomtype]='未知'
ipregistry[access]=$(apsp_access_not_attempted ipregistry '未配置经授权的正式接口；未查询该来源，信息未知')
printf '%s\n' 'ipregistry：未知（未配置经授权的正式接口，未查询该来源）'
}
'''.encode(),
    'db_dbip': r'''db_dbip(){
dbip=()
dbip[risk]='未知'
dbip[score]=''
dbip[access]=$(apsp_access_not_attempted DBIP '未配置经授权的正式接口；未查询该来源，信息未知')
printf '%s\n' 'DB-IP：未知（未配置经授权的正式接口，未查询该来源）'
}
'''.encode(),
    'MediaUnlockTest_DisneyPlus': r'''function MediaUnlockTest_DisneyPlus(){
disney=()
disney[ustatus]='未知'
disney[uregion]="${smedia[nodata]}"
disney[utype]="${smedia[nodata]}"
disney[access]=$(apsp_access_not_attempted disney-plus '未配置经授权的查询方式；未查询该来源，流媒体可用性未知')
printf '%s\n' 'Disney+：未知（未配置经授权的查询方式，未查询该来源）'
}
'''.encode(),
    'MediaUnlockTest_YouTube_Premium': r'''function MediaUnlockTest_YouTube_Premium(){
youtube=()
local result region lower title
apsp_access_youtube_fetch "$1" result || return 0
lower=${result,,}
if title=$(printf '%s' "$lower"|grep -oE '<title>[^<]*</title>');then :;else title='';fi
if [[ $lower != *'<html'* || $lower != *'</html>'* || $lower != *'<head'* || $lower != *'</head>'* || $lower != *'<body'* || $lower != *'</body>'* ||
      ( $title != '<title>youtube premium</title>' && $title != '<title>youtube premium - youtube</title>' && $title != '<title>premium - youtube</title>' ) ||
      ( $lower == *'<form'* && ( $lower == *'accounts.google.com'* || $lower == *'consent.google.com'* || $lower == *'consent.youtube.com'* ) ) ||
      $lower == *'id="challenge-form"'* || $lower == *'id="captcha-form"'* || $lower == *'class="g-recaptcha"'* ]];then
apsp_access_youtube_unknown schema_mismatch '响应不是可确认的 Premium 页面，登录或挑战页面信息未知'
return 0
fi
if region=$(printf '%s' "$result"|grep -oE '"contentRegion"[[:space:]]*:[[:space:]]*"[A-Z][A-Z]"'|sed -E 's/.*"([A-Z][A-Z])"/\1/'|sort -u);then :;else region='';fi
if [[ ! $region =~ ^[A-Z][A-Z]$ ]];then
apsp_access_youtube_unknown schema_mismatch '页面地区字段缺失或不一致'
return 0
fi
if [[ $region == CN && $result == *'www.google.cn'* ]];then
youtube[ustatus]="${smedia[cn]}"
youtube[uregion]="  [CN]   "
youtube[utype]="${smedia[nodata]}"
elif [[ $result == *'Premium is not available in your country'* ]];then
youtube[ustatus]="${smedia[noprem]}"
youtube[uregion]="${smedia[nodata]}"
youtube[utype]="${smedia[nodata]}"
elif [[ $result == *'ad-free'* ]];then
local result1 result3 resultunlocktype
result1=$(Check_DNS_1 www.youtube.com)
result3=$(Check_DNS_3 www.youtube.com)
resultunlocktype=$(Get_Unlock_Type "$result1" "$result3")
youtube[ustatus]="${smedia[yes]}"
youtube[uregion]="  [$region]   "
youtube[utype]="$resultunlocktype"
else
apsp_access_youtube_unknown schema_mismatch '页面未确认 Premium 可用性'
return 0
fi
# This preserves the public-page heuristic; it is not a playback proof.
youtube[access]=$(apsp_access_youtube_metadata succeeded '' '')
}
'''.encode(),
    'read_ref': r'''read_ref(){
Media_Cookie='' # Apsp: public cookies are not fetched or used.
IATA_Database="${rawgithub}main/ref/iata-icao.csv"
}
'''.encode(),
}
FUNCTIONS = tuple(FUNCTION_REPLACEMENTS)
JSON_ANCHOR = b'ipjson=$(echo "$ipjson"|jq "$head_updates$basic_updates$type_updates$score_updates$factor_updates$media_updates$mail_updates.")\n'
JSON_ADDITION = r'''ipjson=$(printf '%s' "$ipjson"|jq --argjson registry "${ipregistry[access]:-null}" --argjson dbip "${dbip[access]:-null}" --argjson disney "${disney[access]:-null}" --argjson youtube "${youtube[access]:-null}" '.Sources=(.Sources//{}) | .Sources.ipregistry=$registry | .Sources.DBIP=$dbip | .Media.DisneyPlus += $disney | .Media.Youtube += $youtube')
'''.encode()
METADATA_REPLACEMENTS = [(JSON_ANCHOR, JSON_ANCHOR + JSON_ADDITION)]


def function_span(name, content):
    if name not in FUNCTION_REPLACEMENTS or not isinstance(content, bytes):
        raise ValueError('unknown access policy function or source type')
    pattern = rb'(?m)^(?:function )?' + name.encode() + rb'\(\)\{\n[^\0]*?^\}\n'
    matches = list(re.finditer(pattern, content))
    if len(matches) != 1:
        raise ValueError('access policy requires unique complete function boundaries')
    return matches[0].span()


def replace_function(name, content, replacement):
    start, stop = function_span(name, content)
    return content[:start] + replacement + content[stop:]


def patch(content):
    for name, replacement in FUNCTION_REPLACEMENTS.items():
        content = replace_function(name, content, replacement)
    anchor = FUNCTION_REPLACEMENTS['db_ipregistry']
    if content.count(anchor) != 1:
        raise ValueError('access policy requires a unique helper insertion anchor')
    content = content.replace(anchor, HELPERS + anchor, 1)
    for before, after in METADATA_REPLACEMENTS:
        if content.count(before) != 1:
            raise ValueError('access policy requires unique JSON anchors')
        content = content.replace(before, after, 1)
    return content


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown access policy role or source byte limit exceeded')
    identity = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != identity['source_sha256']:
        raise ValueError('access policy input SHA256 mismatch')
    result = patch(content)
    if len(result) > MAX_SOURCE + 8192 or hashlib.sha256(result).hexdigest() != identity['patched_sha256']:
        raise ValueError('access policy output SHA256 or byte limit mismatch')
    return result
