#!/usr/bin/env bash
# Run real OpenRC and the installer only inside a disposable test container.
set -euo pipefail
repository=$(cd "$(dirname "$0")/.." && pwd)
image="apsp-openrc-smoke:${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
cleanup() {
  result=$?
  trap - EXIT
  docker image rm "$image" >/dev/null 2>&1 || true
  exit "$result"
}
trap cleanup EXIT
docker build --tag "$image" --file "$repository/tools/openrc-test.Dockerfile" "$repository/tools"
docker run --rm --init --network none --cap-add SYS_ADMIN --security-opt apparmor=unconfined --env "APSP_TEST_AGENT=${APSP_TEST_AGENT:-}" --volume "$repository:/src:ro" "$image"
