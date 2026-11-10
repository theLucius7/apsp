#!/usr/bin/env bash
# Refuse the retained loader's historical online trace installation.
set -euo pipefail
fixed_command='wget https://github.com/nxtrace/NTrace-core/releases/download/v1.3.7/nexttrace_linux_amd64 -qO /usr/local/bin/nexttrace'
if [[ $# == 4 && $2 == /bin/bash && $3 == -c ]]; then
  if [[ $4 == "$fixed_command" || $4 == "${fixed_command/nexttrace_linux_amd64/nexttrace_linux_arm64}" ]]; then
    printf '%s\n' 'Error: online trace installation is forbidden; complete-toolchain admission remains closed' >&2
    exit 70
  fi
fi
exec "$APSP_REAL_CHROOT" "$@"
