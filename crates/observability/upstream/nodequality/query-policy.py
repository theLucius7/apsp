"""Keep transport failures out of the pinned Netflix availability heuristic."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
SOURCES = {'ip.sh': {'source_sha256': '00d6b1a19c74bcfd720796b56985db10175fc99860447ffc38b301e10246e1cb', 'patched_sha256': '66c7f3c662f24bdccbd142acf25d2a176fbe72ff417848ef977aa755d6d38c8e'}}

HELPERS = r'''# Apsp modification (2026-10-01): a failed request is unknown, never unlocked.
apsp_netflix_unknown(){
netflix[ustatus]='未知'
netflix[uregion]="${smedia[nodata]}"
netflix[utype]="${smedia[nodata]}"
netflix[error_category]=$1
netflix[error]=$2
printf 'Netflix：未知（%s）\n' "$2"
}
apsp_netflix_fetch(){
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
REPLACEMENTS = [
    (r'''function MediaUnlockTest_Netflix(){
'''.encode(), HELPERS + r'''function MediaUnlockTest_Netflix(){
'''.encode()),
    (r'''local result1=$(curl $CurlARG -$1 --user-agent "$UA_Browser" -fsL -X GET --max-time 10 --tlsv1.3 "https://www.netflix.com/title/81280792" 2>&1)
'''.encode(), r'''local result1 result2
apsp_netflix_fetch "$1" 81280792 result1 || return 0
'''.encode()),
    (r'''local result2=$(curl $CurlARG -$1 --user-agent "$UA_Browser" -fsL -X GET --max-time 10 --tlsv1.3 "https://www.netflix.com/title/70143836" 2>&1)
'''.encode(), r'''apsp_netflix_fetch "$1" 70143836 result2 || return 0
'''.encode()),
    (r'''region=$(echo "$result1"|sed -n 's/.*"id":"\([^"]*\)".*"countryName":"[^"]*".*/\1/p'|head -n1)
[[ -n $region ]]&&region=$(echo "$result2"|sed -n 's/.*"id":"\([^"]*\)".*"countryName":"[^"]*".*/\1/p'|head -n1)
'''.encode(), r'''local region1 region2
region1=$(printf '%s' "$result1"|sed -n 's/.*"id":"\([A-Z][A-Z]\)".*"countryName":"[^"]*".*/\1/p'|head -n1)
region2=$(printf '%s' "$result2"|sed -n 's/.*"id":"\([A-Z][A-Z]\)".*"countryName":"[^"]*".*/\1/p'|head -n1)
if [[ -z $region1 || -z $region2 || $region1 != "$region2" ]];then
apsp_netflix_unknown schema_mismatch '页面地区字段缺失或不一致'
return
fi
local region=$region1
'''.encode()),
    (r'''media_updates+=".Media |= . * { Netflix: { Status: \"$(clean_ansi "${netflix[ustatus]:-null}")\" } } | "
'''.encode(), r'''media_updates+=".Media |= . * { Netflix: { Status: \"$(clean_ansi "${netflix[ustatus]:-null}")\" } } | "
media_updates+=".Media.Netflix += $(jq -cn --arg error "${netflix[error]:-}" --arg category "${netflix[error_category]:-}" --argjson attempts "${netflix[attempts]:-[]}" '{Error:(if $error == "" then null else {category:$category,message:$error} end),Attempts:$attempts}') | "
'''.encode()),
]


def patch(content):
    for before, after in REPLACEMENTS:
        if content.count(before) != 1:
            raise ValueError('query policy requires unique anchors')
        content = content.replace(before, after, 1)
    return content


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown query role or source byte limit exceeded')
    identity = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != identity['source_sha256']:
        raise ValueError('query policy input SHA256 mismatch')
    result = patch(content)
    if len(result) > MAX_SOURCE + 8192 or hashlib.sha256(result).hexdigest() != identity['patched_sha256']:
        raise ValueError('query policy output SHA256 or byte limit mismatch')
    return result
