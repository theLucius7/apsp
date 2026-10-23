#!/usr/bin/env bash
# Observe the normal terminal branch of the pinned entrypoint with line-preserving swap guards.
# Source SHA-256: 4e1b25894cadf908ef61fb0d9ce874a75524c6dafc2ea26f0477107288e0c018.
# The swap guards preserve line 455 and the post_cleanup body.
# Its post_cleanup exits 1 at line 455 even after successful cleanup. The earlier
# refusal branch and signal/EXIT cleanup are separate execution paths.
unset BASH_ENV
set -o functrace
trap 'if [[ $BASH_COMMAND == "exit 1" && ${BASH_SOURCE[0]} == "$APSP_REPORT_UPSTREAM" && ${FUNCNAME[0]:-} == post_cleanup && ${FUNCNAME[1]:-} == main && $LINENO == 455 ]]; then printf "%s\n" "pinned-normal-cleanup" > "$APSP_REPORT_WORKSPACE/.runner/upstream-completed"; fi' DEBUG
