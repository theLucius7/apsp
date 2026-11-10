"""Keep unconfigured node queries explicit without borrowed access material."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
SOURCES = {
    'ip.sh': {
        'source_sha256': 'ef1640451ffb5ef8f2a0b2059d2e4998eab85388ca8adc5177576ba9c6799e95',
        'patched_sha256': '625cc7d3053b02514480a55446137d19b0335d17790178ec97d3f011e2401dc6',
    },
}
REASONS = {
    'ipregistry': '未配置经授权的节点正式接口；未使用网页临时 key 或公共备用 key，信息未知',
    'dbip': '未配置经授权的节点正式接口；未使用网页临时 key，信息未知',
    'disney': '未配置经授权的节点认证适配；未使用公共 cookies 或固定授权材料，信息未知',
    'youtube': '未配置经授权的节点认证适配；未使用公共 cookies，信息未知',
    'chatgpt': '未配置经授权的节点认证适配；未使用固定 cookies，信息未知',
}
FUNCTIONS = {
    'db_ipregistry': 'ipregistry',
    'db_dbip': 'dbip',
    'function MediaUnlockTest_DisneyPlus': 'disney',
    'function MediaUnlockTest_YouTube_Premium': 'youtube',
    'function OpenAITest': 'chatgpt',
}
MEDIA = {'DisneyPlus': 'disney', 'Youtube': 'youtube', 'ChatGPT': 'chatgpt'}


def refusal(array):
    lines = [
        '# Apsp modification (2026-10-01): official node credentials are absent.',
        '# Keep the source and result column; do not run the borrowed-material path.',
        array + '=()',
        array + '[reason]=' + "'" + REASONS[array] + "'",
        '((ibar_step+=3))',
    ]
    if array == 'ipregistry':
        lines += ['ipregistry[susetype]="   未知    "', 'ipregistry[scomtype]="   未知    "']
    elif array == 'dbip':
        lines += ['dbip[risk]="未知"']
    else:
        lines += [array + '[ustatus]="  未知   "', array + '[uregion]="${smedia[nodata]}"',
                  array + '[utype]="${smedia[nodata]}"']
    return ('\n'.join(lines) + '\nreturn 0\n').encode()


READ_REF_GUARD = b'''# Apsp: no online public cookies, retry or inherited access material.
Cookie=()
return 0
'''
ACCESS_HELPER = r'''# Apsp: access failures are per-source metadata, never positive query results.
apsp_provider_access_json(){
jq -cn \
  --arg ipregistry "${ipregistry[reason]:-}" \
  --arg dbip "${dbip[reason]:-}" \
  --arg disney "${disney[reason]:-}" \
  --arg youtube "${youtube[reason]:-}" \
  --arg chatgpt "${chatgpt[reason]:-}" '
  def blocked($reason): {Status: "not_attempted", ErrorKind: "credential_not_configured",
    Reason: $reason, Execution: "node_self", Attempted: false};
  {ipregistry: blocked($ipregistry), DBIP: blocked($dbip), DisneyPlus: blocked($disney),
   Youtube: blocked($youtube), ChatGPT: blocked($chatgpt)}'
}
'''.encode()
JSON_RESULT = b'''ipjson=$(echo "$ipjson"|jq "$head_updates$basic_updates$type_updates$score_updates$factor_updates$media_updates$mail_updates.")
'''
ACCESS_FILTER = '.ProviderAccess = $access'
for provider in MEDIA:
    ACCESS_FILTER += (' | .Media.' + provider + '.Status = "未知"'
                      ' | .Media.' + provider + '.Region = null'
                      ' | .Media.' + provider + '.Type = null'
                      ' | .Media.' + provider + '.Reason = $access.' + provider + '.Reason')
ACCESS_FILTER += ' | .Type.Usage.ipregistry = null | .Type.Company.ipregistry = null'
JSON_ACCESS = r'''local apsp_access_json
apsp_access_json=$(apsp_provider_access_json) || return 70
ipjson=$(printf '%s' "$ipjson" | jq --argjson access "$apsp_access_json" '@ACCESS_FILTER@') || return 70
'''.replace('@ACCESS_FILTER@', ACCESS_FILTER).encode()
DISPLAY = ''.join("printf '%s：%s\\n' '" + label + "' \"${" + array + "[reason]}\"\n"
                  for label, array in [('ipregistry', 'ipregistry'), ('DB-IP', 'dbip'),
                                       ('Disney+', 'disney'), ('YouTube Premium', 'youtube'),
                                       ('ChatGPT', 'chatgpt')]).encode()
REPLACEMENTS = [(b'read_ref(){\n', b'read_ref(){\n' + READ_REF_GUARD)]
for function, array in FUNCTIONS.items():
    anchor = (function + '(){\n').encode()
    REPLACEMENTS.append((anchor, anchor + refusal(array)))
REPLACEMENTS += [
    (b'save_json(){\n', ACCESS_HELPER + b'save_json(){\n'),
    (JSON_RESULT, JSON_RESULT + JSON_ACCESS),
    (b'show_tail(){\n', b'show_tail(){\n' + DISPLAY),
]


def patch(content):
    for before, after in REPLACEMENTS:
        if content.count(before) != 1:
            raise ValueError('public access policy requires unique fixed anchors')
        content = content.replace(before, after, 1)
    return content


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown public access role or source byte limit exceeded')
    spec = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != spec['source_sha256']:
        raise ValueError('public access policy input SHA256 mismatch')
    result = patch(content)
    if len(result) > MAX_SOURCE + 16384 or hashlib.sha256(result).hexdigest() != spec['patched_sha256']:
        raise ValueError('public access policy output SHA256 or byte limit mismatch')
    return result
