#!/usr/bin/env bash
# Each run owns a unique Compose project, including the volumes removed by its trap.
set -euo pipefail
repository=$(cd "$(dirname "$0")/.." && pwd)
cd "$repository"
APSP_RELEASE_PUBLIC_KEYS=$(python3 scripts/ci-test-trust.py)
export APSP_RELEASE_PUBLIC_KEYS
export COMPOSE_PROJECT_NAME="apsp-ci-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
export APSP_BIND_ADDRESS=127.0.0.1
export APSP_PORT=${APSP_PORT:-18080}
export APSP_PUBLIC_URL="http://127.0.0.1:$APSP_PORT"
export APSP_DB_PASSWORD=${APSP_DB_PASSWORD:-$(python3 -c 'import secrets; print(secrets.token_hex(24))')}
export APSP_ADMIN_PASSWORD=${APSP_ADMIN_PASSWORD:-$(python3 -c 'import secrets; print(secrets.token_hex(24))')}
compose=(docker compose --env-file /dev/null -f deploy/docker-compose.yml)
state_file=$(mktemp)
cleanup() {
  result=$?
  trap - EXIT
  if [ "$result" -ne 0 ]; then
    "${compose[@]}" ps || true
    "${compose[@]}" logs --no-color || true
  fi
  "${compose[@]}" down --volumes --remove-orphans || true
  rm -f "$state_file"
  exit "$result"
}
trap cleanup EXIT
check_api() {
  python3 - "$1" "$state_file" <<'PY'
import http.cookiejar
import json
import os
import re
import sys
import urllib.error
import urllib.request
from pathlib import Path

phase, state_file = sys.argv[1:]
base = 'http://127.0.0.1:' + os.environ['APSP_PORT']
client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))

def request(path, data=None):
    body = None if data is None else json.dumps(data).encode()
    headers = {} if data is None else {'Content-Type': 'application/json'}
    return client.open(urllib.request.Request(base + path, data=body, headers=headers), timeout=15)

with request('/healthz') as response:
    assert response.read() == b'ok'
with request('/') as response:
    assert response.headers.get_content_type() == 'text/html'
    assert response.headers['Cache-Control'] == 'no-store'
    html = response.read().decode()
    assert 'apsp' in html
assets = re.findall(r'(?:src|href)="(/assets/[^\"]+)"', html)
assert any(path.endswith('.js') for path in assets)
assert any(path.endswith('.css') for path in assets)
for path in assets:
    with request(path) as response:
        body = response.read()
        assert body and b'<!doctype html>' not in body.lower()
        if path.endswith('.js'):
            assert response.headers.get_content_type() == 'text/javascript'
        elif path.endswith('.css'):
            assert response.headers.get_content_type() == 'text/css'
def expect(path, code):
    try:
        request(path)
    except urllib.error.HTTPError as error:
        assert error.code == code, (path, error.code)
    else:
        raise AssertionError(path + ' must return ' + str(code))

# Unauthenticated API requests are refused before routing, so they cannot
# probe which administrative endpoints exist.
expect('/api/does-not-exist', 401)
expect('/assets/missing.js', 404)
with request('/api/login', {'password': os.environ['APSP_ADMIN_PASSWORD']}) as response:
    assert 'HttpOnly' in response.headers['Set-Cookie']
expect('/api/does-not-exist', 404)
if phase == 'create':
    with request('/api/servers', {'name': 'CI 持久化检查'}) as response:
        assert response.status == 201
        server = json.load(response)
        Path(state_file).write_text(json.dumps({'server_id': server['id']}))
    with request('/api/servers/' + str(server['id']) + '/enrollment', {}) as response:
        enrollment = json.load(response)
        assert enrollment['installation'] is None
        assert enrollment['install_command'] is None
        assert enrollment['warning']
    with request('/api/artifacts') as response:
        assert json.load(response) == []
else:
    server_id = json.loads(Path(state_file).read_text())['server_id']
    with request('/api/servers/' + str(server_id)) as response:
        assert json.load(response)['name'] == 'CI 持久化检查'
print('Compose HTTP and persistence smoke:', phase, 'passed')
PY
}
"${compose[@]}" up --detach --build --wait --wait-timeout 180
"${compose[@]}" exec -T panel sh -c '[ "$(id -u)" -eq 10001 ] && test -w /data && printf persisted > /data/.ci-write-probe'
check_api create
"${compose[@]}" up --detach --force-recreate --wait --wait-timeout 180
"${compose[@]}" exec -T panel sh -c '[ "$(cat /data/.ci-write-probe)" = persisted ]'
check_api verify
