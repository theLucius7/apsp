"""Refuse the fixed OpenAI access path without borrowing authorization material."""
import hashlib
import re

MAX_SOURCE = 2 * 1024 * 1024
SOURCES = {
    'ip.sh': {
        'source_sha256': 'db6752d56fed4947863abed95f5a419e97928d71c0660f8ee160f2ffe927f0f7',
        # Deliberately unusable until the final immutable chain is materialized.
        'patched_sha256': 'fe0f5bb5f4db28f171d766e9c69d35c38d2d522d09c01ec2256afbbfd4c37197',
    },
}

HELPERS = r'''# Apsp modification (2026-10-01): no borrowed OpenAI authorization.
apsp_openai_not_attempted(){
jq -cn --arg target_ip "$IP" --arg checked_at "$(date +%s)" '{provider:"openai",status:"unknown",target_ip:$target_ip,checked_at:($checked_at|tonumber),Attempted:false,last_attempt_at:null,elapsed_seconds:null,Error:{category:"credential_not_configured",phase:"not_attempted",message:"未配置经授权的节点认证适配；未查询该来源，ChatGPT 可用性未知"},Attempts:[]}'
}
'''.encode()

FUNCTION_REPLACEMENTS = {
    'OpenAITest': r'''function OpenAITest(){
chatgpt=()
chatgpt[ustatus]='未知'
chatgpt[uregion]="${smedia[nodata]}"
chatgpt[utype]="${smedia[nodata]}"
if chatgpt[access]=$(apsp_openai_not_attempted);then :;else return 70;fi
printf '%s\n' 'ChatGPT：未知（未配置经授权的节点认证适配，未查询该来源）'
}
'''.encode(),
}
FUNCTIONS = tuple(FUNCTION_REPLACEMENTS)
JSON_ANCHOR = b'ipjson=$(echo "$ipjson"|jq "$head_updates$basic_updates$type_updates$score_updates$factor_updates$media_updates$mail_updates.")\n'
JSON_ADDITION = r'''local apsp_openai_json
if [[ -n ${chatgpt[access]:-} ]];then
apsp_openai_json=${chatgpt[access]}
elif apsp_openai_json=$(apsp_openai_not_attempted);then :;else return 70;fi
ipjson=$(printf '%s' "$ipjson"|jq --argjson openai "$apsp_openai_json" '.Sources=(.Sources//{}) | .Sources.OpenAI=$openai | .Media.ChatGPT=((.Media.ChatGPT//{}) + $openai + {Status:"未知",Region:null,Type:null})') || return 70
'''.encode()
METADATA_REPLACEMENTS = [(JSON_ANCHOR, JSON_ANCHOR + JSON_ADDITION)]


def function_span(name, content):
    if name not in FUNCTION_REPLACEMENTS or not isinstance(content, bytes):
        raise ValueError('unknown OpenAI policy function or source type')
    pattern = rb'(?m)^function ' + name.encode() + rb'\(\)\{\n[^\0]*?^\}\n'
    matches = list(re.finditer(pattern, content))
    if len(matches) != 1:
        raise ValueError('OpenAI policy requires unique complete function boundaries')
    return matches[0].span()


def patch(content):
    start, stop = function_span('OpenAITest', content)
    content = content[:start] + HELPERS + FUNCTION_REPLACEMENTS['OpenAITest'] + content[stop:]
    for before, after in METADATA_REPLACEMENTS:
        if content.count(before) != 1:
            raise ValueError('OpenAI policy requires unique JSON anchors')
        content = content.replace(before, after, 1)
    return content


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown OpenAI policy role or source byte limit exceeded')
    identity = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != identity['source_sha256']:
        raise ValueError('OpenAI policy input SHA256 mismatch')
    result = patch(content)
    if len(result) > MAX_SOURCE + 4096 or hashlib.sha256(result).hexdigest() != identity['patched_sha256']:
        raise ValueError('OpenAI policy output SHA256 or byte limit mismatch')
    return result
