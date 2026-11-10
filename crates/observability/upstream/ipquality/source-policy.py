"""Derive one bounded node-egress profile from the exact AGPL IPQuality source."""
import hashlib
import re

SOURCE_COMMIT = '87397e2c3196ec796f5477c83343c2354df601ea'
SOURCE_SHA256 = 'b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf'
VERSION = SOURCE_COMMIT + '-node-r1'
MAX_SOURCE = 2 * 1024 * 1024
POLICY_ORDER = ('report', 'dependency', 'data', 'ip-score', 'browser', 'query',
                'access', 'netflix', 'openai')

HELPERS = r'''# Apsp modification (2026-10-02): independent node-egress profile.
# Original IPQuality copyright and AGPL-3.0 remain applicable.
readonly APSP_IPQUALITY_TRANSPORT=/usr/local/lib/apsp-ipquality/transport.py
apsp_node_receipt(){
/usr/bin/python3 "$APSP_IPQUALITY_TRANSPORT" "$@"
}
apsp_node_snapshot(){
local preserved=$ipjson
save_json || return 70
printf '%s' "$ipjson" | apsp_node_receipt snapshot || return 70
ipjson=$preserved
}
apsp_node_json_text(){
jq -cn --arg value "$1" 'if $value=="" or $value=="null" or $value=="N/A" then null else $value end'
}
apsp_node_unknown(){
local -n result_array=$1
result_array=()
result_array[ustatus]='未知';result_array[uregion]='';result_array[utype]=''
result_array[susetype]='未知';result_array[scomtype]='未知';result_array[risk]='未知'
}
apsp_node_call(){
local dataset=$1 array=$2 function=$3
shift 3
"$function" "$@" >&2 || :
case "$array" in
ipinfo|ipapi|abuseipdb|ip2location)
local -n typed_result=$array
case "${typed_result[usetype]:-}" in ''|null) typed_result[susetype]='未知';; esac
case "${typed_result[comtype]:-}" in ''|null) typed_result[scomtype]='未知';; esac
;;
esac
case "$array" in
netflix) [[ -z ${netflix[error_category]:-} ]] || apsp_node_receipt annotate "$dataset" "${netflix[error_category]}" "${netflix[error]:-页面字段不匹配}";;
youtube) if [[ -n ${youtube[access]:-} ]];then
local category message
category=$(printf '%s' "${youtube[access]}"|jq -r '.Error.category//empty')
message=$(printf '%s' "${youtube[access]}"|jq -r '.Error.message//empty')
[[ -z $category ]] || apsp_node_receipt annotate "$dataset" "$category" "$message"
fi;;
esac
if ! apsp_node_receipt source-state "$dataset";then apsp_node_unknown "$array";fi
apsp_node_snapshot || return 70
}
apsp_node_disabled(){
apsp_node_receipt not-attempted "$1" "$2" "$3" || return 70
}
'''.encode()

SIMPLE_FUNCTIONS = {
    'countRunTimes': 'countRunTimes(){\n: # Public hit counters are never contacted.\n}\n',
    'show_ad': 'show_ad(){\n: # No online advertisements or sponsor downloads.\n}\n',
    'check_connectivity': 'check_connectivity(){\nreturn 0 # No connectivity or mirror discovery request.\n}\n',
    'show_progress_bar': 'show_progress_bar(){\n: # Measurement execution is serial and noninteractive.\n}\n',
    'kill_progress_bar': 'kill_progress_bar(){\n: # No detached progress process is needed.\n}\n',
    'install_dependencies': r'''install_dependencies(){
local tool
for tool in jq curl bc python3;do
command -v "$tool" >/dev/null 2>&1 || { printf 'Missing offline tool: %s\n' "$tool" >&2;exit 70; }
done
}
''',
    'check_mail': r'''check_mail(){
smail=();smailstatus=();services=()
smail[local]=2;smail[remote]=0
apsp_node_disabled smtp-disabled SMTP '此节点查询未授权 SMTP 探测；未执行，信息未知'
}
''',
    'check_dnsbl': r'''check_dnsbl(){
smail[t]=null;smail[c]=null;smail[m]=null;smail[b]=null
apsp_node_disabled dnsbl-disabled DNSBL '此节点查询未授权 DNSBL 批量探测；未执行，信息未知'
}
''',
    'check_dnsbl_parallel': r'''check_dnsbl_parallel(){
printf '%s\n' 'DNSBL bulk requests are disabled' >&2
return 70
}
''',
    'show_mail': 'show_mail(){\nprintf "%s\\n" "SMTP / DNSBL：未知（本次未执行）"\n}\n',
    'Check_DNS_1': 'function Check_DNS_1(){\nprintf "%s" "" # DNS unlock type is unverified.\n}\n',
    'Check_DNS_2': 'function Check_DNS_2(){\nprintf "%s" "" # DNS unlock type is unverified.\n}\n',
    'Check_DNS_3': 'function Check_DNS_3(){\nprintf "%s" "" # DNS unlock type is unverified.\n}\n',
    'Get_Unlock_Type': 'function Get_Unlock_Type(){\nprintf "%s" "" # A public-page check is not proof of DNS/native unlocking.\n}\n',
    'factor_bool': r'''factor_bool(){
[[ $# == 3 && $2 =~ ^[A-Za-z][A-Za-z0-9]*$ && $3 =~ ^[A-Za-z][A-Za-z0-9]*$ ]] || return 70
local literal=null
case "$1" in
true|false) literal=$1;;
*) if [[ $1 =~ ^[A-Z]{2}$ ]];then literal=$(jq -cn --arg value "$1" '$value') || return 70;fi;;
esac
printf '.Factor |= . * { %s: { %s: %s } } | ' "$3" "$2" "$literal"
}
''',
    'get_ipv4': r'''get_ipv4(){
IPV4=$(apsp_node_receipt discover 4) || IPV4=''
}
''',
    'get_ipv6': r'''get_ipv6(){
IPV6=$(apsp_node_receipt discover 6) || IPV6=''
}
''',
    'MediaUnlockTest_TikTok': r'''function MediaUnlockTest_TikTok(){
tiktok=()
local response region
response=$(curl -$1 -sS --max-time 10 --compressed https://www.tiktok.com/) || return 0
region=$(printf '%s' "$response"|grep -oE '"region"[[:space:]]*:[[:space:]]*"[A-Z]{2}"'|sed -E 's/.*"([A-Z]{2})"/\1/'|sort -u)
[[ $region =~ ^[A-Z]{2}$ ]] || { apsp_node_receipt annotate TikTok schema_mismatch '页面地区字段缺失或不一致';return 0; }
tiktok[ustatus]="${smedia[yes]}";tiktok[uregion]="$region";tiktok[utype]=''
}
''',
    'MediaUnlockTest_PrimeVideo_Region': r'''function MediaUnlockTest_PrimeVideo_Region(){
amazon=()
local response region
response=$(curl -$1 -sS --max-time 10 --compressed https://www.primevideo.com/) || return 0
region=$(printf '%s' "$response"|grep -oE '"currentTerritory"[[:space:]]*:[[:space:]]*"[A-Z]{2}"'|sed -E 's/.*"([A-Z]{2})"/\1/'|sort -u)
[[ $region =~ ^[A-Z]{2}$ ]] || { apsp_node_receipt annotate AmazonPrimeVideo schema_mismatch '页面地区字段缺失或不一致';return 0; }
amazon[ustatus]="${smedia[yes]}";amazon[uregion]="$region";amazon[utype]=''
}
''',
    'MediaUnlockTest_Reddit': r'''function MediaUnlockTest_Reddit(){
reddit=()
local response region
response=$(curl -$1 -sS --max-time 10 --compressed https://www.reddit.com/svc/shreddit/reddit-chat) || return 0
region=$(printf '%s' "$response"|grep -oE 'country="[A-Z]{2}"'|sed -E 's/.*"([A-Z]{2})"/\1/'|sort -u)
[[ $region =~ ^[A-Z]{2}$ ]] || { apsp_node_receipt annotate Reddit schema_mismatch '页面地区字段缺失或不一致';return 0; }
reddit[ustatus]="${smedia[yes]}";reddit[uregion]="$region";reddit[utype]=''
}
''',
}

CHECK_IP = r'''check_IP(){
IP=$1
export APSP_IPQUALITY_TARGET_IP="$IP"
mode_lite=0
show_head >&2
ipjson='{"Head":{},"Info":{},"Type":{},"Score":{},"Factor":{},"Media":{},"Mail":{}}'
apsp_node_call MaxMind maxmind db_maxmind "$2" || return $?
apsp_node_call IPinfo ipinfo db_ipinfo || return $?
apsp_node_call SCAMALYTICS scamalytics db_scamalytics "$2" || return $?
db_ipregistry "$2" >&2
apsp_node_disabled ipregistry-not-configured ipregistry '未配置经授权的正式接口；未查询该来源，信息未知' || return $?
apsp_node_call ipapi ipapi db_ipapi "$2" || return $?
apsp_node_call AbuseIPDB abuseipdb db_abuseipdb "$2" || return $?
apsp_node_call IP2LOCATION ip2location db_ip2location "$2" || return $?
db_dbip >&2
apsp_node_disabled dbip-not-configured DBIP '未配置经授权的正式接口；未查询该来源，信息未知' || return $?
apsp_node_call ipdata ipdata db_ipdata "$2" || return $?
apsp_node_call IPQS ipqs db_ipqs "$2" || return $?
apsp_node_call TikTok tiktok MediaUnlockTest_TikTok "$2" || return $?
MediaUnlockTest_DisneyPlus "$2" >&2
apsp_node_disabled disney-not-configured DisneyPlus '未配置经授权的查询方式；未查询该来源，流媒体可用性未知' || return $?
apsp_node_call Netflix netflix MediaUnlockTest_Netflix "$2" || return $?
apsp_node_call Youtube youtube MediaUnlockTest_YouTube_Premium "$2" || return $?
apsp_node_call AmazonPrimeVideo amazon MediaUnlockTest_PrimeVideo_Region "$2" || return $?
apsp_node_call Reddit reddit MediaUnlockTest_Reddit "$2" || return $?
OpenAITest "$2" >&2
apsp_node_disabled openai-not-configured OpenAI '未配置经授权的节点认证适配；未查询该来源，ChatGPT 可用性未知' || return $?
check_mail || return $?
check_dnsbl || return $?
save_json || return 70
printf '%s' "$ipjson" | apsp_node_receipt snapshot || return 70
printf '%s\n' "$ipjson"
}
'''.encode()

MAIN_ANCHOR = b'\nUA_Browser=\'\' # Apsp: browser identity generation is disabled.\nadapt_locale\n'
MAIN = r'''
# The independently signed wrapper supplies exactly one IP-family argument.
[[ $# == 1 && ( $1 == 4 || $1 == 6 ) ]] || { printf '%s\n' 'Expected one IP family: 4 or 6' >&2;exit 2; }
export APSP_IPQUALITY_FAMILY="$1"
[[ -n ${APSP_IPQUALITY_ATTEMPTS:-} && -n ${APSP_IPQUALITY_PARTIAL:-} ]] || exit 70
UA_Browser=''
YY=en
mode_json=1;mode_output=0;mode_privacy=1;mode_lite=0;fullIP=1
install_dependencies
set_language
# JSON still uses upstream field names; status text is explicit Chinese.
smedia[yes]='可用';smedia[no]='不可用';smedia[nodata]='未知'
shead[bash]="/usr/bin/bash /usr/local/lib/apsp-ipquality/patched-ip.sh $1"
shead[git]='https://github.com/xykt/IPQuality/tree/87397e2c3196ec796f5477c83343c2354df601ea'
if [[ $1 == 4 ]];then get_ipv4;IP=$IPV4;else get_ipv6;IP=$IPV6;fi
[[ -n $IP ]] || { printf '%s\n' 'Node egress is unknown; no provider query was attempted.' >&2;exit 69; }
check_IP "$IP" "$1"
'''.encode()

JSON_ANCHOR = b'ipjson=$(echo "$ipjson"|jq "$head_updates$basic_updates$type_updates$score_updates$factor_updates$media_updates$mail_updates.")\n'
JSON_FINAL = r'''ipjson=$(printf '%s' "$ipjson"|jq 'walk(if type=="string" and (.=="null" or .=="" or .=="N/A") then null else . end) | .Mail={Port25:null,DNSBlacklist:{Total:null,Clean:null,Marked:null,Blacklisted:null},Status:"未知",Reason:"SMTP 与 DNSBL 未授权，本次未执行"}') || return 70
'''.encode()


def replace_once(content, before, after):
    if content.count(before) != 1:
        raise ValueError('independent IP policy requires one exact anchor')
    return content.replace(before, after, 1)


# These exact body endings are audited after the nine pinned transforms.
# They include nested group/function closures, and exclude following globals.
FUNCTION_ENDINGS = {
    'countRunTimes': b'stail[total]=$(echo "$RunTimes"|jq \'.total\')\n}\n',
    'show_ad': b'ADLines=$(((adCount+1)*12))\nfi\n}\n',
    'check_connectivity': b'rawgithub="https://testingcf.jsdelivr.net/gh/xykt/IPQuality@"\nreturn 1\nfi\n}\n',
    'show_progress_bar': b'show_progress_bar_ "$@" 1>&2\n}\n',
    'kill_progress_bar': b'kill "$bar_pid" 2>/dev/null&&echo -ne "\\r"\n}\n',
    'install_dependencies': b'exit 70\nfi\nreturn 0\n}\n',
    'check_mail': b'check_email_service $service\nkill_progress_bar\ndone\n}\n',
    'check_dnsbl': b'smail[sdnsbl]="$Font_Cyan${smail[dnsbl]}  ${smail[available]}${smail[t]}   ${smail[clean]}${smail[c]}   ${smail[marked]}${smail[m]}   ${smail[blacklisted]}${smail[b]}$Font_Suffix"\n}\n',
    'check_dnsbl_parallel': b'echo "${smail[t]} ${smail[c]} ${smail[m]} ${smail[b]}"\n}\n}\n',
    'show_mail': b'[[ $1 -eq 4 ]]&&echo -ne "\\r${smail[sdnsbl]}\\n"\n}\n',
    'Check_DNS_1': b'echo $(Check_DNS_IP ${resultinlines[$resultdnsindex]} ${resultinlines[1]})\n}\n',
    'Check_DNS_2': b'else\necho 1\nfi\n}\n',
    'Check_DNS_3': b'else\necho 0\nfi\n}\n',
    'Get_Unlock_Type': b'echo "${smedia[native]}"\n}\n',
    'factor_bool': b'[[ -z $tmp_txt ]]&&tmp_txt="null"\necho "$tmp_txt"\n}\n',
    'get_ipv4': b'IPV4="$response"\nbreak\nfi\ndone\n}\n',
    'get_ipv6': b'IPV6="$response"\nbreak\nfi\ndone\n}\n',
    'MediaUnlockTest_TikTok': b'tiktok[utype]="${smedia[nodata]}"\nreturn\nfi\n}\n',
    'MediaUnlockTest_PrimeVideo_Region': b'amazon[utype]="${smedia[nodata]}"\nreturn\nfi\n}\n',
    'MediaUnlockTest_Reddit': b'reddit[utype]="${smedia[nodata]}"\nesac\n}\n',
    'check_IP': b'*)echo -e "$ip_report"|sed \'s/\\x1b\\[[0-9;]*[mGKHF]//g\' >>"$outputfile" 2>/dev/null\nesac\nfi\n}\n',
    'save_json': b'ipjson=$(printf \'%s\' "$ipjson"|jq --argjson registry "${ipregistry[access]:-null}" --argjson dbip "${dbip[access]:-null}" --argjson disney "${disney[access]:-null}" --argjson youtube "${youtube[access]:-null}" \'.Sources=(.Sources//{}) | .Sources.ipregistry=$registry | .Sources.DBIP=$dbip | .Media.DisneyPlus += $disney | .Media.Youtube += $youtube\')\n}\n',
}


def function_span(content, name):
    ending = FUNCTION_ENDINGS.get(name)
    if ending is None:
        raise ValueError('independent IP policy has no reviewed function ending: ' + name)
    pattern = rb'(?m)^(?:function )?' + re.escape(name.encode()) + rb'\(\)\{\n[^\0]*?' + re.escape(ending)
    matches = list(re.finditer(pattern, content))
    if len(matches) != 1:
        raise ValueError('independent IP policy requires a unique complete function: ' + name)
    start, stop = matches[0].span()
    headers = re.findall(rb'(?m)^(?:function )?([A-Za-z_][A-Za-z0-9_]*)\(\)\{\n', content[start:stop])
    expected = [name.encode()]
    if name == 'show_ad':
        expected += [b'print_pair', b'print_block']
    if headers != expected:
        raise ValueError('independent IP policy function boundary crosses another function: ' + name)
    return start, stop


def replace_function(content, name, replacement):
    start, stop = function_span(content, name)
    return content[:start] + replacement + content[stop:]


def quote_json_values(content):
    start, stop = function_span(content, 'save_json')
    serializer = content[start:stop]
    # Preserve the upstream field mapping, but pass every interpolated string
    # through jq --arg. Provider data must not become jq source code.
    serializer = re.sub(rb'\\"([^\n]*?)\\"',
                        lambda match: b'$(apsp_node_json_text "' + match[1] + b'")', serializer)
    return content[:start] + serializer + content[stop:]


def patch(content):
    for name, replacement in SIMPLE_FUNCTIONS.items():
        content = replace_function(content, name, replacement.encode())
    content = replace_function(content, 'check_IP', HELPERS + CHECK_IP)
    content = replace_once(content, b'if [[ -z $RESPONSE ]];then\nmode_lite=1\nelse\nmode_lite=0\nfi\n', b'mode_lite=0 # A failed source does not disable other source queries.\n')
    content = replace_once(content, JSON_ANCHOR, JSON_ANCHOR + JSON_FINAL)
    content = re.sub(rb'(?m)^show_progress_bar[^\n]*&\nbar_pid="\$!"&&disown "\$bar_pid"\ntrap "kill_progress_bar" RETURN\n',
                     b'', content)
    if content.count(MAIN_ANCHOR) != 1:
        raise ValueError('independent IP policy requires its fixed main boundary')
    content = content[:content.index(MAIN_ANCHOR)] + MAIN
    return quote_json_values(content)


def transform(content, policies, references):
    if not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('bounded fixed IPQuality source required')
    if hashlib.sha256(content).hexdigest() != SOURCE_SHA256:
        raise ValueError('independent IPQuality canonical source mismatch')
    if set(policies) != set(POLICY_ORDER):
        raise ValueError('independent IPQuality fixed policy closure mismatch')
    for name in POLICY_ORDER:
        if name == 'data':
            content = policies[name]['transform']('ip.sh', content, references)
        else:
            content = policies[name]['transform']('ip.sh', content)
    result = patch(content)
    if len(result) > MAX_SOURCE or b'\0' in result:
        raise ValueError('independent IPQuality output exceeds source bounds')
    return result
