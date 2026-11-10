#!/usr/bin/env bash
set -euo pipefail
umask 077
upload=0
for argument in "$@"; do
  [[ $argument != https://api.nodequality.com/api/v1/record ]] || upload=1
done
if [[ $upload == 0 ]]; then
  # A release digest does not authorize the rootfs or its embedded tools.
  # Every source request must use the signed local bundle. Rootfs downloads
  # have no network fallback, including the former exact v0.0.2 URL shapes.
  exec python3 "$APSP_CHAIN_HELPER" serve "$APSP_CHAIN_DIRECTORY" "$@"
fi
python3 "$APSP_REPORT_HELPER" capture "$APSP_REPORT_WORKSPACE"
if [[ ${APSP_UPLOAD_REPORT:-false} != true ]]; then
  printf '%s\n' 'disabled' > "$APSP_REPORT_WORKSPACE/upload-disabled.txt"
  printf '%s\n' 'Public report upload is disabled.'
  exit 0
fi
arguments=()
while [[ $# != 0 ]]; do
  if [[ $1 == --data-binary && ${2:-} == @- ]]; then
    arguments+=(--data-binary "@$APSP_REPORT_WORKSPACE/upload.base64")
    shift 2
  else
    arguments+=("$1")
    shift
  fi
done
set +e
"$APSP_REAL_CURL" --connect-timeout 15 --max-time 60 --max-filesize 65536 \
  --write-out $'\nAPSP_RESPONSE_STATUS:%{http_code}' "${arguments[@]}" \
  | python3 "$APSP_REPORT_HELPER" response "$APSP_REPORT_WORKSPACE"
statuses=("${PIPESTATUS[@]}")
set -e
[[ ${statuses[1]} == 0 ]] || exit "${statuses[1]}"
exit "${statuses[0]}"
