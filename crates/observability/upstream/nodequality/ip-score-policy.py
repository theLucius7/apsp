"""Validate pinned IP score values before arithmetic and report formatting."""
import hashlib

MAX_SOURCE = 2 * 1024 * 1024
SOURCES = {
    'ip.sh': {
        'source_sha256': 'e332b5405ca12fe03ad5f427789c38c92f04bb2b66df05d296060e4cd8483c1e',
        'patched_sha256': 'f434c87f920cc9594aca3978b58e28dcaf786c074d70d64475f567068d093c4d',
    },
}
HELPERS = r'''# Apsp modification (2026-10-01): unknown scores never imply low risk.
apsp_ip_score_value(){
printf '%s' "$1" | jq -rs --arg path "$2" --arg kind "$3" '
  if length != 1 then empty else .[0] end
  | select(type == "object")
  | select(.success != false and .status != "fail" and .status != "error")
  | select((.error == null or .error == false or .error == "")
           and (.errors == null or .errors == []))
  | getpath($path | split("."))
  | if $kind == "integer" then
      select(type == "number") | select(. >= 0 and . <= 100 and floor == .)
    elif $kind == "ipapi" then
      select(type == "string")
      | select(test("^(0(\\.[0-9]+)?|1(\\.0+)?) \\((Very Low|Low|Elevated|High|Very High)\\)\\z"; "i"))
    elif $kind == "dbip" then
      select(type == "string") | ascii_downcase | select(. == "low" or . == "medium" or . == "high")
    else empty end
  ' 2>/dev/null || :
}
apsp_ip_score_json(){
jq -cn --arg value "$1" 'if $value == "" then null else $value end'
}
apsp_ip_unknown_score(){
[[ -n $2 ]] || printf '%s：未知（未取得有效评分）\n' "$1"
}
'''.encode()
NUMERIC = {
    'scamalytics': ('scamalytics.scamalytics_score', 20),
    'abuseipdb': ('data.abuseConfidenceScore', 25),
    'ip2location': ('fraud_score', 33),
    'ipqs': ('fraud_score', 75),
}
REPLACEMENTS = [(b'db_scamalytics(){\n', HELPERS + b'db_scamalytics(){\n')]
for name, (path, threshold) in NUMERIC.items():
    original = f'{name}[score]=$(echo "$RESPONSE"|jq -r \'.{path}\')\n'
    replacement = f'{name}[score]=$(apsp_ip_score_value "$RESPONSE" "{path}" integer)\n'
    REPLACEMENTS.append((original.encode(), replacement.encode()))
    guard = f'if [[ ${{{name}[score]}} -lt {threshold} ]];then\n'
    checked = (f'if [[ -z ${{{name}[score]}} ]];then\n{name}[risk]="未知"\n'
               + 'el' + guard)
    REPLACEMENTS.append((guard.encode(), checked.encode()))
for name, path, kind in (('ipapi', 'company.abuser_score', 'ipapi'), ('dbip', 'threatLevel', 'dbip')):
    field = 'scoretext' if name == 'ipapi' else 'risktext'
    original = f'{name}[{field}]=$(echo "$RESPONSE"|jq -r \'.{path}\')\n'
    replacement = f'{name}[{field}]=$(apsp_ip_score_value "$RESPONSE" "{path}" {kind})\n'
    REPLACEMENTS.append((original.encode(), replacement.encode()))
REPLACEMENTS.append((
    b'sscore_text(){\n',
    b'sscore_text(){\nif [[ $# -ne 6 || ! $2 =~ ^[0-9]+$ ]];then return 0; fi # Apsp: no arithmetic on unknown input.\n',
))
NOTICE = '''[[ $mode_lite -ne 0 ]] || apsp_ip_unknown_score IP2Location "${ip2location[score]}"
apsp_ip_unknown_score Scamalytics "${scamalytics[score]}"
apsp_ip_unknown_score ipapi "${ipapi[score]}"
if [[ $mode_lite -eq 0 ]];then
apsp_ip_unknown_score AbuseIPDB "${abuseipdb[score]}"
apsp_ip_unknown_score IPQS "${ipqs[score]}"
fi
apsp_ip_unknown_score DB-IP "${dbip[score]}"
'''.encode()
RANGE = b'echo -ne "\\r${sscore[range]}\\n"\n'
REPLACEMENTS.append((RANGE, RANGE + NOTICE))
for label, name in (('IP2LOCATION', 'ip2location'), ('SCAMALYTICS', 'scamalytics'),
                    ('ipapi', 'ipapi'), ('AbuseIPDB', 'abuseipdb'), ('IPQS', 'ipqs'), ('DBIP', 'dbip')):
    original_field = 'ipapi[ipqs]' if name == 'ipqs' else name + '[score]'
    original = 'score_updates+=".Score |= . + { ' + label + ': \\"${' + original_field + ':-null}\\" } | "\n'
    replacement = 'score_updates+=".Score |= . + { ' + label + ': $(apsp_ip_score_json "${' + name + '[score]:-}") } | "\n'
    REPLACEMENTS.append((original.encode(), replacement.encode()))


def patch(content):
    for before, after in REPLACEMENTS:
        if content.count(before) != 1:
            raise ValueError('IP score policy requires unique anchors')
        content = content.replace(before, after, 1)
    return content


def transform(role, content):
    if role not in SOURCES or not isinstance(content, bytes) or len(content) > MAX_SOURCE:
        raise ValueError('unknown IP score role or source byte limit exceeded')
    identity = SOURCES[role]
    if hashlib.sha256(content).hexdigest() != identity['source_sha256']:
        raise ValueError('IP score policy input SHA256 mismatch')
    result = patch(content)
    if len(result) > MAX_SOURCE + 4096 or hashlib.sha256(result).hexdigest() != identity['patched_sha256']:
        raise ValueError('IP score policy output SHA256 or byte limit mismatch')
    return result
