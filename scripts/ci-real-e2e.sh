#!/usr/bin/env bash
# Install on a disposable Ubuntu host; remove only resources created by this run.
set -Eeuo pipefail
umask 077

usage() {
  printf '%s\n' 'Usage: scripts/ci-real-e2e.sh --agent-root DIR --runtime-root DIR --summary FILE'
}
die() { printf 'Real e2e failed: %s\n' "$*" >&2; exit 1; }
agent_root= runtime_root= summary=
while [[ $# -gt 0 ]]; do
  case "$1" in
    --agent-root) agent_root=$2; shift 2 ;;
    --runtime-root) runtime_root=$2; shift 2 ;;
    --summary) summary=$2; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done
[[ -n $agent_root && -n $runtime_root && -n $summary ]] || { usage >&2; exit 2; }
[[ $(uname -s) == Linux && $(uname -m) == x86_64 && -d /run/systemd/system ]] || die 'requires Linux amd64 with real systemd'
[[ ${GITHUB_ACTIONS:-} == true || ${APSP_E2E_DISPOSABLE_HOST:-} == 1 ]] || die 'use a disposable runner; manual reproduction requires APSP_E2E_DISPOSABLE_HOST=1'
for tool in docker python3 sudo systemctl systemd-run openssl curl readelf go sha256sum minisign install; do
  command -v "$tool" >/dev/null || die "missing tool: $tool"
done
sudo -n true || die 'noninteractive sudo is required'
[[ $(/bin/mv --version) == *'GNU coreutils'* ]] || die 'real artifact publication requires GNU mv'
for path in /etc/apsp /opt/apsp /var/lib/apsp /run/apsp /usr/local/bin/apsp-agent \
  /etc/systemd/system/apsp-agent.service /etc/systemd/system/apsp-singbox@.service; do
  if sudo test -e "$path" || sudo test -L "$path"; then die 'existing Apsp installation detected; leaving it untouched'; fi
done
if getent passwd apsp-singbox >/dev/null || getent group apsp-singbox >/dev/null; then
  die 'existing runtime identity detected; leaving it untouched'
fi
repository=$(cd "$(dirname "$0")/.." && pwd -P)
cd "$repository"
scratch=$(mktemp -d "${RUNNER_TEMP:-/tmp}/apsp-real-e2e.XXXXXX")
trust_directory=
APSP_RELEASE_PUBLIC_KEYS=$(python3 scripts/ci-test-trust.py)
export APSP_RELEASE_PUBLIC_KEYS
export COMPOSE_PROJECT_NAME="apsp-real-e2e-${GITHUB_RUN_ID:-local}-${GITHUB_RUN_ATTEMPT:-1}-$$"
export APSP_BIND_ADDRESS=127.0.0.1
export APSP_PORT=18080
export APSP_PUBLIC_URL=http://127.0.0.1:18080
export APSP_DB_PASSWORD=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
export APSP_ADMIN_PASSWORD=$(python3 -c 'import secrets; print(secrets.token_hex(32))')
export APSP_E2E_ADMIN_PASSWORD="$APSP_ADMIN_PASSWORD"
export APSP_E2E_STATUS_COMMAND='sudo /usr/local/bin/apsp-agent status'
export APSP_E2E_ARTIFACT_ROOT=$scratch/artifacts
phase=preflight owned_installation=0 hosts_entry=0 fixture_pid= client_pid= tls_container=
passed=0 failure_line=
marker=$COMPOSE_PROJECT_NAME
compose=(docker compose --env-file /dev/null -f deploy/docker-compose.yml -f "$scratch/compose.override.yml")

write_summary() {
  python3 - "$scratch/state.json" "$summary" "$passed" "$phase" "${result:-0}" "$owned_installation" "${failure_line:-0}" <<'PY'
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys

state_path, output, passed, phase, exit_code, owned_installation, failure_line = sys.argv[1:]
specification = importlib.util.spec_from_file_location("e2e_summary_driver", "scripts/e2e-driver.py")
driver = importlib.util.module_from_spec(specification)
specification.loader.exec_module(driver)
traffic_specification = importlib.util.spec_from_file_location("e2e_traffic_evidence", "scripts/e2e-traffic-evidence.py")
traffic = importlib.util.module_from_spec(traffic_specification)
traffic_specification.loader.exec_module(traffic)
summary = {"passed": passed == "1", "last_phase": phase, "exit_code": int(exit_code), "runtime_version": "1.14.2"}
if not summary["passed"]:
    summary["failure_line"] = int(failure_line)
traffic_evidence = traffic.load(Path(state_path).with_name(traffic.EVIDENCE_NAME))
if traffic_evidence:
    summary["traffic"] = traffic_evidence
readiness = Path(state_path).with_name("ready-timeout.json")
if readiness.is_file():
    last = json.loads(readiness.read_text())
    summary["readiness"] = driver.readiness_status(last["snapshot"], last.get("expected_agent_version"))
if passed != "1" and owned_installation == "1":
    summary["systemd"] = {}
    for unit in ("apsp-agent.service", "apsp-singbox@main.service"):
        try:
            query = subprocess.run(["systemctl", "show", unit,
                                    "--property=ActiveState,SubState,Result,ExecMainStatus"],
                                   capture_output=True, text=True, timeout=5, check=False)
            summary["systemd"][unit] = driver.systemd_status(query.stdout, query.returncode == 0)
        except (OSError, subprocess.SubprocessError):
            summary["systemd"][unit] = {"query_succeeded": False}
if Path(state_path).is_file():
    state = json.loads(Path(state_path).read_text())
    summary["agent_version"] = state.get("installation", {}).get("version")
    if state.get("requested_port") == 443 and state.get("port") == 443:
        summary["reality_port"] = 443
    summary["usage"] = {
        label: {key: checkpoint["usage"][key] for key in ("uplink", "downlink", "total")}
        for label, checkpoint in state.get("checkpoints", {}).items()
        if label in ("ready", "first", "before", "after-agent", "after-runtime", "resumed", "before-reinstall", "after-reinstall")
    }
signature = Path(state_path).with_name("signature-checks.json")
if signature.is_file():
    checks = json.loads(signature.read_text())
    allowed = ("cached-binary", "cached-proof", "legacy-unsigned-cache", "restored-systemd-verifier")
    summary["signature_checks"] = [label for label in checks.get("checks", []) if label in allowed]
    summary["test_trust_root"] = "TEST_ONLY"
ledger = Path(state_path).with_name("ledger-summary.json")
if ledger.is_file():
    summary["ledger"] = {
        label: {key: counters[key] for key in ("uplink", "downlink", "total")}
        for label, counters in json.loads(ledger.read_text()).items()
        if label in ("before", "after-agent", "after-runtime", "before-reinstall", "after-reinstall")
    }
retirement = Path(state_path).with_name("retirement.json")
if retirement.is_file():
    checks = json.loads(retirement.read_text())
    allowed = ("passed", "online_delete_confirmed", "server_removed", "receipt_acknowledged",
               "credentials_removed", "configuration_removed", "ledger_preserved", "outbox_drained",
               "agent_stopped", "runtime_stopped", "agent_restart_refused", "runtime_restart_refused")
    summary["retirement"] = {name: checks[name] for name in allowed if type(checks.get(name)) is bool}
    totals = checks.get("usage", {})
    if isinstance(totals, dict) and all(isinstance(totals.get(key), str)
            and re.fullmatch(r"0|[1-9][0-9]*", totals[key]) for key in ("uplink", "downlink", "total")):
        summary["retirement"]["usage"] = {key: totals[key] for key in ("uplink", "downlink", "total")}
Path(output).parent.mkdir(parents=True, exist_ok=True)
Path(output).write_text(json.dumps(summary, indent=2) + "\n")
if os.environ.get('GITHUB_ACTIONS') == 'true' and not summary['passed']:
    # Publish only the same allowlisted fields as the acceptance artifact.
    print('::error title=Reality acceptance failed::' + json.dumps(summary, ensure_ascii=True))
if os.environ.get('GITHUB_STEP_SUMMARY'):
    with Path(os.environ['GITHUB_STEP_SUMMARY']).open('a') as report:
        report.write('### 真实安装与 Reality 验收\n\n')
        report.write(f"结果：{'通过' if summary['passed'] else '失败'}；最后阶段：{phase}。\n\n")
        report.write('| 阶段 | 上传字节 | 下载字节 |\n| --- | ---: | ---: |\n')
        for label, usage in summary.get('usage', {}).items():
            report.write(f"| {label} | {usage['uplink']} | {usage['downlink']} |\n")
        if summary.get('retirement', {}).get('passed') is True:
            report.write('\n在线退役、凭证与配置清理、账本保留及禁止重启检查：通过。\n')
PY
}

cleanup() {
  result=$?
  trap - EXIT
  # Keep the triggering failure while still attempting every cleanup operation.
  if [[ $result != 0 ]]; then set +e; fi
  if [[ $result != 0 && ($phase == first-traffic || $phase == resumed-traffic) ]]; then
    # Inspect only this run's fixtures before stopping them. Keep the original failure.
    python3 scripts/e2e-traffic-evidence.py --scratch "$scratch" failure \
      --client-pid "${client_pid:-0}" --fixture-pid "${fixture_pid:-0}" || true
  fi
  write_summary || true
  [[ -z $client_pid ]] || { kill "$client_pid" 2>/dev/null || true; wait "$client_pid" 2>/dev/null || true; }
  [[ -z $fixture_pid ]] || { kill "$fixture_pid" 2>/dev/null || true; wait "$fixture_pid" 2>/dev/null || true; }
  [[ -z $tls_container ]] || docker rm --force -- "$tls_container" >/dev/null 2>&1 || true
  if [[ $owned_installation == 1 ]]; then
    sudo systemctl disable --now apsp-agent.service apsp-singbox@main.service >/dev/null 2>&1 || true
    sudo rm -f /etc/systemd/system/apsp-agent.service /etc/systemd/system/apsp-singbox@.service /usr/local/bin/apsp-agent
    sudo rm -rf /etc/apsp /opt/apsp /var/lib/apsp /run/apsp
    sudo systemctl daemon-reload
    sudo systemctl reset-failed apsp-agent.service apsp-singbox@main.service >/dev/null 2>&1 || true
    if getent passwd apsp-singbox >/dev/null; then sudo userdel apsp-singbox; fi
    if getent group apsp-singbox >/dev/null; then sudo groupdel apsp-singbox; fi
  fi
  if [[ $hosts_entry == 1 ]]; then
    sudo python3 - "$marker" <<'PY'
from pathlib import Path
import sys
path = Path('/etc/hosts')
marker = '# ' + sys.argv[1]
path.write_text(''.join(line for line in path.read_text().splitlines(keepends=True) if not line.rstrip().endswith(marker)))
PY
  fi
  if [[ -f $scratch/compose.override.yml ]]; then
    "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
    docker image rm "$COMPOSE_PROJECT_NAME-panel:local" >/dev/null 2>&1 || true
  fi
  [[ -z $trust_directory ]] || sudo rm -rf -- "$trust_directory"
  sudo rm -rf -- "$scratch"
  if [[ $result != 0 ]]; then printf 'Real e2e failed during %s; only the redacted summary is public.\n' "$phase" >&2; fi
  exit "$result"
}
trap cleanup EXIT
trap 'if [[ -z $failure_line ]]; then failure_line=$LINENO; fi' ERR
trap 'exit 130' INT
trap 'exit 143' TERM

sudo python3 - <<'PY'
import socket
for family, address, port in ((socket.AF_INET, '0.0.0.0', 443),
                              (socket.AF_INET6, '::', 443),
                              *((socket.AF_INET, '127.0.0.1', port) for port in (2080, 18080, 18081, 18085))):
    with socket.socket(family) as listener:
        listener.bind((address, port))
PY

mkdir -m 0755 "$scratch/source"
cp -R "$agent_root/agent" "$scratch/source/agent"
cp -R "$runtime_root/sing-box" "$scratch/source/sing-box"
agent_version=$(python3 - "$scratch/source/agent" <<'PY'
import hashlib
from pathlib import Path
import re
import subprocess
import sys
root = Path(sys.argv[1])
versions = [path for path in root.iterdir() if path.is_dir() and not path.is_symlink()]
if len(versions) != 1: raise SystemExit('need one same-run Agent version')
version = versions[0].name
if not re.fullmatch(r'\d+\.\d+\.\d+(?:[-+][A-Za-z0-9.-]+)?', version): raise SystemExit('invalid version')
binary = versions[0] / 'amd64'
with binary.open('rb') as source: actual = hashlib.file_digest(source, 'sha256').hexdigest()
matches = [line.split()[0].lower() for line in (versions[0] / 'SHA256SUMS').read_text().splitlines()
           if re.fullmatch(r'[0-9a-fA-F]{64} [ *]amd64', line)]
if matches != [actual]: raise SystemExit('Agent checksum mismatch')
binary.chmod(0o755)
header = subprocess.check_output(['readelf', '-h', str(binary)], text=True)
if 'Advanced Micro Devices X86-64' not in header: raise SystemExit('Agent architecture mismatch')
program = subprocess.check_output(['readelf', '-lW', str(binary)], text=True)
dynamic = subprocess.check_output(['readelf', '-dW', str(binary)], text=True)
if 'INTERP' in program or 'NEEDED' in dynamic: raise SystemExit('Agent must be static musl')
if subprocess.check_output([str(binary), '--version'], text=True).strip() != 'apsp-agent ' + version:
    raise SystemExit('Agent version mismatch')
print(version)
PY
)
python3 scripts/verify-e2e-runtime.py "$runtime_root" "$scratch/runtime"
runtime=$scratch/runtime/sing-box
phase=signed-release
bash tools/build-nodequality.sh amd64 "$scratch/source" > "$scratch/nodequality-build.log" 2>&1
python3 scripts/ci-signed-release.py --source "$scratch/source" --bundle "$scratch/release" \
  --panel-root "$scratch/artifacts" --agent-version "$agent_version" --arch amd64
chmod -R a+rX "$scratch/artifacts"
# Provision the verifier and trust independently from the checked-out operator tools,
# before receiving any panel installation data. No panel-provided shell is executed.
trust_directory=$(sudo mktemp -d /root/apsp-ci-trust.XXXXXX)
sudo install -m 0400 crates/protocol/tests/fixtures/public-keys.json "$trust_directory/public-keys.json"
sudo install -m 0500 tools/bootstrap.py "$trust_directory/bootstrap.py"
sudo install -m 0400 tools/release.py "$trust_directory/release.py"
cat > "$scratch/compose.override.yml" <<'YAML'
services:
  panel:
    image: ${COMPOSE_PROJECT_NAME}-panel:local
    volumes:
      - ${APSP_E2E_ARTIFACT_ROOT}:/data/artifacts:ro
YAML

phase=compose
"${compose[@]}" up --detach --build --wait --wait-timeout 240 > "$scratch/compose.log" 2>&1

phase=fixtures
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 1 \
  -keyout "$scratch/camouflage.key" -out "$scratch/camouflage.crt" \
  -subj '/CN=apsp-e2e.example.test' -addext 'subjectAltName=DNS:apsp-e2e.example.test' \
  > "$scratch/certificate.log" 2>&1
# Keep the TLS camouflage endpoint in a separate network namespace so the real
# unprivileged runtime, with CAP_NET_BIND_SERVICE, can listen on host port 443.
docker run --rm --no-healthcheck --entrypoint /usr/bin/openssl "$COMPOSE_PROJECT_NAME-panel:local" version \
  > "$scratch/tls-openssl-version.txt" 2>&1
tls_network=${COMPOSE_PROJECT_NAME}_default
[[ $(docker network inspect --format '{{index .Labels "com.docker.compose.project"}}' "$tls_network") == "$COMPOSE_PROJECT_NAME" ]] \
  || die 'TLS fixture network does not belong to this Compose project'
tls_container=$(docker create --name "$COMPOSE_PROJECT_NAME-tls" --network "$tls_network" \
  --user 0 --read-only --no-healthcheck --entrypoint /usr/bin/openssl \
  --mount "type=bind,src=$scratch/camouflage.crt,dst=/fixture/camouflage.crt,readonly" \
  --mount "type=bind,src=$scratch/camouflage.key,dst=/fixture/camouflage.key,readonly" \
  "$COMPOSE_PROJECT_NAME-panel:local" s_server -quiet -www -tls1_3 -groups X25519 \
  -accept 0.0.0.0:443 -cert /fixture/camouflage.crt -key /fixture/camouflage.key)
docker start "$tls_container" > "$scratch/tls-start.txt"
[[ $(docker inspect --format '{{.State.Running}}' "$tls_container") == true ]] || die 'TLS fixture failed to start'
docker inspect --format '{{json .NetworkSettings.Networks}}' "$tls_container" > "$scratch/tls-network.json"
sudo python3 - "$marker" "$tls_network" "$scratch/tls-network.json" <<'PY'
import ipaddress
import json
from pathlib import Path
import socket
import sys
import time

networks = json.loads(Path(sys.argv[3]).read_text())
if set(networks) != {sys.argv[2]}:
    raise SystemExit('TLS fixture is attached to an unexpected network')
address = ipaddress.ip_address(networks[sys.argv[2]]['IPAddress'])
if address.version != 4 or not address.is_private or address.is_loopback or address.is_unspecified or address.is_multicast:
    raise SystemExit('TLS fixture has an invalid private bridge address')
deadline = time.monotonic() + 20
while True:
    try:
        with socket.create_connection((str(address), 443), timeout=2):
            break
    except OSError:
        if time.monotonic() >= deadline:
            raise SystemExit('TLS fixture did not become reachable on its private port 443') from None
        time.sleep(0.2)
with Path('/etc/hosts').open('a') as output:
    output.write('\n' + str(address) + ' apsp-e2e.example.test # ' + sys.argv[1] + '\n')
PY
hosts_entry=1
python3 scripts/e2e-http-fixture.py --listen 127.0.0.1 --port 18081 > "$scratch/fixture.log" 2>&1 &
fixture_pid=$!

phase=prepare
driver=(python3 scripts/e2e-driver.py --state "$scratch/state.json")
"${driver[@]}" prepare --origin "$APSP_PUBLIC_URL" --public-host 127.0.0.1 --sni apsp-e2e.example.test --port 443
phase=install
owned_installation=1
sudo python3 scripts/ci-bootstrap-install.py --enrollment "$scratch/enrollment.json" \
  --trust-directory "$trust_directory" --bundle "$scratch/release" --log "$scratch/install.log"
"${driver[@]}" ready --agent-version "$agent_version" --timeout 600
python3 - "$scratch/state.json" "$scratch/client.json" <<'PY'
import json
from pathlib import Path
import sys
state, client = (json.loads(Path(path).read_text()) for path in sys.argv[1:])
ports = [outbound['server_port'] for outbound in client['outbounds'] if outbound['type'] == 'vless']
if state.get('requested_port') != 443 or state.get('port') != 443 or ports != [443]:
    raise SystemExit('Reality must use explicitly requested privileged port 443')
PY
runtime_pid=$(sudo systemctl show apsp-singbox@main.service -p MainPID --value)
[[ $runtime_pid =~ ^[1-9][0-9]*$ ]] || die 'independent runtime is not active'

wait_agent() {
  python3 - <<'PY'
import json
import subprocess
import time
deadline = time.monotonic() + 120
while time.monotonic() < deadline:
    try:
        process = subprocess.run(['sudo', '/usr/local/bin/apsp-agent', 'status'],
                                 check=True, capture_output=True, text=True, timeout=10)
        value = json.loads(process.stdout)
        if (value.get('connected') is True and value.get('healthy', {}).get('singbox') is True
                and value.get('applied', {}).get('singbox', 0) > 0):
            break
    except (subprocess.SubprocessError, ValueError):
        pass
    time.sleep(2)
else:
    raise SystemExit('Agent did not restore its live status connection')
PY
}

# Read only this run's configured user/node baseline, never identity material.
assert_ledger() {
  sudo python3 - "$scratch/state.json" "$1" "${2:-}" > "$scratch/ledger-next.json" <<'PY_LEDGER'
import json
from pathlib import Path
import sqlite3
import sys

state_path, label, previous = sys.argv[1:]
state = json.loads(Path(state_path).read_text())
name = f"u{state['user_id']}_n{state['node_id']}"
with sqlite3.connect('file:/var/lib/apsp/core/state.db?mode=ro', uri=True, timeout=5) as connection:
    connection.execute('BEGIN')
    rows = connection.execute('SELECT uplink,downlink FROM usage_baselines WHERE module=? AND stat_name=?', ('singbox', name)).fetchall()
    pending = connection.execute('SELECT COUNT(*) FROM usage_outbox WHERE acknowledged=0').fetchone()[0]
if len(rows) != 1 or pending != 0:
    raise SystemExit('Sampled ledger baseline missing or still awaiting acknowledgement')
current = tuple(int(value) for value in rows[0])
path = Path(state_path).with_name('ledger-summary.json')
proof = json.loads(path.read_text()) if path.is_file() else {}
checkpoint = state['checkpoints'][label]['usage']
actual = tuple(int(checkpoint[key]) for key in ('uplink', 'downlink'))
if previous == 'zero':
    if current != (0, 0):
        raise SystemExit('Runtime reload did not establish sampled zero counters')
elif previous:
    baseline = proof[previous]
    previous_usage = state['checkpoints'][previous]['usage']
    delta = tuple(actual[index] - int(previous_usage[key]) for index, key in enumerate(('uplink', 'downlink')))
    expected = tuple(current[index] - int(baseline[key]) for index, key in enumerate(('uplink', 'downlink')))
    if delta != expected or any(value < 0 for value in expected):
        raise SystemExit('Panel increment differs from sampled runtime ledger increment')
elif actual != current:
    raise SystemExit('Panel totals differ from sampled runtime ledger totals')
proof[label] = {key: str(value) for key, value in zip(('uplink', 'downlink', 'total'), (*current, sum(current)))}
print(json.dumps(proof))
PY_LEDGER
  mv "$scratch/ledger-next.json" "$scratch/ledger-summary.json"
  printf 'Panel and sampled runtime ledger counters agree: %s\n' "$1"
}

start_client() {
  "$runtime" run -c "$scratch/client.json" > "$scratch/client.log" 2>&1 &
  client_pid=$!
  for attempt in {1..30}; do
    kill -0 "$client_pid" 2>/dev/null || die 'Reality client exited'
    if python3 - <<'PY'
import socket
try:
    with socket.create_connection(('127.0.0.1', 2080), timeout=1): pass
except OSError:
    raise SystemExit(1)
PY
    then return; fi
    sleep 1
  done
  die 'Reality client did not listen'
}
stop_client() { kill "$client_pid"; wait "$client_pid" || true; client_pid=; }
traffic_batch() {
  python3 scripts/e2e-traffic-evidence.py --scratch "$scratch" transfer --phase "$phase" --direction download
  python3 - "$scratch/download.bin" "$scratch/upload.bin" <<'PY'
from pathlib import Path
import sys
download = Path(sys.argv[1]).read_bytes()
if download != b's' * (2 * 1024 * 1024): raise SystemExit('proxy download mismatch')
Path(sys.argv[2]).write_bytes(b'u' * (1024 * 1024))
PY
  python3 scripts/e2e-traffic-evidence.py --scratch "$scratch" transfer --phase "$phase" --direction upload
  [[ $(cat "$scratch/upload-response.txt") == 1048576 ]] || die 'proxy upload mismatch'
}

phase=first-traffic
start_client
traffic_batch
"${driver[@]}" traffic --label first --after ready --min-uplink 1048576 --min-downlink 2097152
stop_client
phase=stable-before
"${driver[@]}" verify --label before --interval 35 --timeout 240
assert_ledger before
phase=agent-restart
sudo systemctl restart apsp-agent.service
wait_agent
"${driver[@]}" verify --label after-agent --unchanged-from before --interval 35 --timeout 240
assert_ledger after-agent
[[ $(sudo systemctl show apsp-singbox@main.service -p MainPID --value) == "$runtime_pid" ]] || die 'Agent restart changed independent runtime PID'
phase=runtime-reload
sudo systemctl reload apsp-singbox@main.service
wait_agent
"${driver[@]}" verify --label after-runtime --unchanged-from before --interval 35 --timeout 240
assert_ledger after-runtime zero
phase=resumed-traffic
start_client
traffic_batch
"${driver[@]}" traffic --label resumed --after after-runtime --min-uplink 1048576 --min-downlink 2097152
stop_client
"${driver[@]}" verify --label before-reinstall --interval 35 --timeout 240
assert_ledger before-reinstall after-runtime
phase=signed-preflight-rejections
"${driver[@]}" install --refresh --agent-version "$agent_version"
sudo env APSP_E2E_DISPOSABLE_HOST=1 python3 scripts/ci-signed-preflight.py \
  --enrollment "$scratch/enrollment.json" --trust-directory "$trust_directory" \
  --bundle "$scratch/release" --log-directory "$scratch/signature-logs" \
  --summary "$scratch/signature-checks.json"
phase=reinstall
sudo find /etc/apsp/identity -maxdepth 1 -type f -exec sha256sum {} + | sort > "$scratch/identity-before.txt"
# The same token remains unconsumed after every rejected preflight.
sudo python3 scripts/ci-bootstrap-install.py --enrollment "$scratch/enrollment.json" \
  --trust-directory "$trust_directory" --bundle "$scratch/release" --log "$scratch/reinstall.log"
wait_agent
"${driver[@]}" ready --agent-version "$agent_version" --timeout 600
"${driver[@]}" verify --label after-reinstall --unchanged-from before-reinstall --interval 35 --timeout 240
assert_ledger after-reinstall after-runtime
sudo find /etc/apsp/identity -maxdepth 1 -type f -exec sha256sum {} + | sort > "$scratch/identity-after.txt"
cmp "$scratch/identity-before.txt" "$scratch/identity-after.txt" || die 'reinstallation changed device identity files'
[[ $(sudo systemctl show apsp-singbox@main.service -p MainPID --value) == "$runtime_pid" ]] || die 'same-version reinstallation changed runtime PID'
phase=online-retirement
sudo --preserve-env=APSP_E2E_ADMIN_PASSWORD env APSP_E2E_DISPOSABLE_HOST=1 \
  python3 scripts/ci-retirement.py --state "$scratch/state.json" --summary "$scratch/retirement.json"
# The root helper writes a private summary; let the runner read only this redacted file.
sudo chown -- "$(id -u):$(id -g)" "$scratch/retirement.json"
phase=complete
passed=1
printf '%s\n' 'Signed installation, cache rejection, Reality traffic, accounting, restart, reload, reinstallation, and online retirement passed.'
