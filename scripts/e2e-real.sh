#!/usr/bin/env bash
# Print the manual acceptance guide or collect read-only device evidence.
set -euo pipefail
umask 077

usage() {
  cat <<'TEXT'
用法：
  bash scripts/e2e-real.sh guide
  sudo bash scripts/e2e-real.sh snapshot <标签> <证据目录>
  python3 scripts/e2e-driver.py --help

guide 仅打印完整人工流程；snapshot 只读收集本机状态，不安装、重启或修改服务。
分阶段的真实面板驱动见 docs/e2e.md；由操作者控制专用设备与独立客户端。

snapshot 可选环境变量（前三项须同时提供）：
  APSP_PANEL_URL=https://panel.example.com
  APSP_USER_ID=1
  APSP_NODE_ID=1
  APSP_STATE_DB=/var/lib/apsp/core/state.db

提供面板参数后会隐藏输入管理员密码，将按用户+节点筛选的用量和部署状态
保存到本次证据目录；不会保存密码、会话、私钥或完整配置包。
使用 sudo 时按需 --preserve-env=APSP_PANEL_URL,APSP_USER_ID,APSP_NODE_ID。
TEXT
}

guide() {
  cat <<'TEXT'
apsp Linux systemd / OpenRC 人工端到端验收
=========================================
本脚本不是自动验收结果。请记录每一步的时间、版本、结果及失败日志。
准备一台全新 Linux amd64/arm64 服务器、一台独立客户端和可访问的面板。
systemd 基线使用 Debian 12；OpenRC 使用已启动 OpenRC 的专用 Linux 设备。
每项服务命令选择本机对应的 init 分支，不要交叉执行。
不要把 macOS 单测、虚拟服务测试或上游 check 成功当作下面的实机流程已通过。

一、面板、制品与接入
1. 按 docs/deploy.md 启动面板+PostgreSQL；公开地址必须从设备和客户端都可访问。
   独立核对发布公钥；登录后在制品页导入已签 Release，确认匹配设备架构的
   Agent 和运行时均通过发布签名、元数据与实际内容校验。
2. 在 Debian 设备安装基础工具：
     apt-get update
     apt-get install -y ca-certificates curl coreutils passwd iproute2 python3 minisign
   OpenRC 设备用本机包管理器安装 curl、CA、基础文件/账号工具、bash、iproute2、
   python3、minisign 和 unshare；supervise-daemon 必须支持 capabilities/no_new_privs。
   Agent 与运行时的架构、libc 必须匹配；glibc 运行时不能直接用于 Alpine musl。
3. 按 docs/deploy.md 在设备独立准备可信 apsp-bootstrap 与发布公钥。
   在面板添加服务器并选择已导入的兼容 Agent 版本，以 root 执行生成的命令。
   使用验收驱动时，按 docs/e2e.md 传入私有 enrollment.json 中的接入信息。
   令牌只使用一次；不得把另一个设备的身份复制过来。
4. 30 秒内确认服务器在线，记录设备架构、Agent 版本、CPU/内存和更新时间。
   在设备运行：
     apsp-agent status
     systemctl status apsp-agent.service --no-pager
   OpenRC 对应命令：rc-service apsp-agent status。
   此时尚未发布配置，代理运行时可以尚未启动。

二、节点、用户、客户端与流量
5. 新建 VLESS + Reality 节点，填写客户端可达的公开地址和可用的伪装域名。
   记录分配端口；确认服务器/服务商已有网络规则允许该端口访问。
6. 创建一个专用验收用户，仅授权此节点。等待 5 秒合并发布及设备应用完成。
   服务器详情：目标版本=已应用版本、健康=true、无本次部署错误。
   设备：
     systemctl is-active apsp-singbox@main.service
     /opt/apsp/plugins/sing-box/current/sing-box version
     ss -lnt 'sport = :18085'
   OpenRC 的服务状态命令为：rc-service apsp-singbox@main status。
   版本应为 1.14.2，包含 with_v2ray_api；18085 必须只在 127.0.0.1 监听。
7. 在独立客户端导入此用户的订阅，不要在服务器内使用直连代替代理链路。
   使用 sing-box JSON 时，本地混合入口是 127.0.0.1:2080。示例：
     curl --fail --proxy socks5h://127.0.0.1:2080 https://example.com/
   再使用自己控制的测试端点产生可识别的下载和上传；记录客户端实际完成量。
   在客户端确认请求确实经所选代理节点出站，并按需检查出站地址。
8. 等待 1 至 2 分钟，在用户页“该用户的节点流量”验证上传/下载均有合理增量。
   不要求统计字节与 HTTP 文件大小完全相等；记录代理统计口径和协议开销。

三、稳定基线与 Agent 重启
9. 暂停验收客户端，关闭其他使用本验收用户的连接。
   等待至少两次采样，确认面板该用户该节点用量稳定且 pending_batches=0。
   可先设置以下环境（将示例改为本次实际值）：
     export APSP_PANEL_URL=https://panel.example.com
     export APSP_USER_ID=1
     export APSP_NODE_ID=1
     bash scripts/e2e-real.sh snapshot before ./evidence
10. 记录运行时进程，再重启 Agent。systemd：
      systemctl show apsp-singbox@main.service -p MainPID
      systemctl restart apsp-agent.service
    等待 Agent 恢复连接后再次查询运行时 MainPID，应保持不变；代理为独立服务。
    OpenRC：先收集一次 snapshot，记录 runtime-service.txt 的 ChildPIDs，执行
      rc-service apsp-agent restart
    再收集快照，实际运行时 ChildPIDs 应保持不变。/run/*.pid 是 supervisor PID，
    不能用它代替实际运行时 PID；快照通过 Linux procfs 读取子进程。
    等待两次采样并收集：
      bash scripts/e2e-real.sh snapshot after-agent ./evidence
    检查已确认用量未倒退、未因重启翻倍；无新增流量时稳定总数应不变。

四、运行时重载与再次使用
11. 保持客户端暂停并确认待确认批次清零后，执行一次显式运行时重载：
      systemctl reload apsp-singbox@main.service
    等待本地统计接口恢复；检查日志中有无重载失败、统计重置/缺失窗口告警：
      journalctl -u apsp-agent.service --since '-5 minutes' --no-pager
      journalctl -u apsp-singbox@main.service --since '-5 minutes' --no-pager
    OpenRC 对应命令：
      rc-service apsp-singbox@main reload
      tail -n 80 /var/log/apsp/agent.log
      tail -n 80 /var/log/apsp/runtime.log
    再收集：
      bash scripts/e2e-real.sh snapshot after-runtime ./evidence
    外部 HUP 不经过 Agent 的终值采样；若重载时仍有流量，可能存在无法观测的
    短采样窗口，必须如实记录。已经确认的历史流量不能丢失或重复入账。
12. 恢复同一客户端，执行新的小批次上传/下载，等待 1 至 2 分钟：
      bash scripts/e2e-real.sh snapshot after-new-traffic ./evidence
    验证新流量继续增长，重载前的累计计数没有被再加一遍。
13. 通过面板修改节点 SNI 等原生配置后改回，分别等待健康应用，验证托管发布。
    托管应用会在重载前读取统计终值，再打开新计量周期。检查目标/已应用版本。
    不要为了测试向公网用户使用中的节点发布故意损坏的配置。

五、升级、身份和结论
14. 验证升级时先导入目标已签版本，在原服务器签发新的接入令牌并重新安装。
    旧未签缓存按 docs/release.md 完成迁移验证，不能跳过签名预检。
    保留 /etc/apsp/identity 和 /var/lib/apsp/core/state.db；注册服务器 ID 不变，
    已消费的旧令牌不能再用。完成后再次核对版本、部署健康、流量与待确认数。
15. 整理 before、after-agent、after-runtime、after-new-traffic 中的：
    - Agent connected/applied/healthy/pending_batches；
    - 运行时 MainPID、监听地址及版本；
    - 面板按本用户+本节点过滤的精确十进制用量；
    - 本地账本的最后 seq、epoch、待确认批次及未完成意图；
    - 本次 SHA256SUMS、客户端测试结果、异常窗口和人工观察。
    只有全部相应步骤实际执行才写“实机验收通过”。未执行项目保持“待验证”。
16. 在专用验收机整机重启后确认 Agent 与最后一份运行时配置自动恢复，再次验证
    身份、已确认用量、待确认批次及新流量。OpenRC 检查 default runlevel 已启用
    apsp-agent 与 apsp-singbox@main；记录版本、libc 与启动日志。

脚本不修改 outbox 来模拟丢失 ACK；批次重放去重与恢复另由自动集成测试覆盖。
证据可能包含主机名、内部编号和用量，保存在私有目录，不要提交真实环境证据。
TEXT
}

snapshot() {
  [[ $# == 2 ]] || { usage >&2; exit 2; }
  local label=$1 output=$2 evidence
  [[ $label =~ ^[A-Za-z0-9_-]+$ ]] || { echo '标签只能包含字母、数字、下划线和连字符' >&2; exit 2; }
  [[ $(uname -s) == Linux && $(id -u) == 0 ]] || { echo '请在已安装 Agent 的 Linux 设备上以 root 收集快照' >&2; exit 1; }
  for tool in apsp-agent ss python3; do
    command -v "$tool" >/dev/null || { printf '缺少工具：%s\n' "$tool" >&2; exit 1; }
  done
  mkdir -p -- "$output"
  evidence=$(mktemp -d "$output/$label.XXXXXX")
  date -u +'%Y-%m-%dT%H:%M:%SZ' > "$evidence/time.txt"
  uname -srmo > "$evidence/system.txt"
  apsp-agent --version > "$evidence/agent-version.txt"
  apsp-agent status > "$evidence/agent-status.json"
  if [[ -d /run/systemd/system ]] && command -v systemctl >/dev/null; then
    printf 'systemd\n' > "$evidence/init-system.txt"
    systemctl show apsp-agent.service -p ActiveState -p SubState -p MainPID -p NRestarts -p ActiveEnterTimestamp > "$evidence/agent-service.txt"
    systemctl show apsp-singbox@main.service -p ActiveState -p SubState -p MainPID -p NRestarts -p ActiveEnterTimestamp > "$evidence/runtime-service.txt"
  elif [[ -f /run/openrc/softlevel ]] && command -v rc-service >/dev/null; then
    printf 'openrc\n' > "$evidence/init-system.txt"
    openrc_snapshot apsp-agent > "$evidence/agent-service.txt"
    openrc_snapshot apsp-singbox@main > "$evidence/runtime-service.txt"
  else
    echo '需要运行中的 systemd 或 OpenRC' >&2; exit 1
  fi
  /opt/apsp/plugins/sing-box/current/sing-box version > "$evidence/runtime-version.txt"
  ss -lnt 'sport = :18085' > "$evidence/stats-listener.txt"
  python3 - "$evidence" <<'PY'
import getpass
import hashlib
import http.cookiejar
import json
import os
from pathlib import Path
import sqlite3
import sys
import urllib.parse
import urllib.request

output = Path(sys.argv[1])
database = Path(os.environ.get('APSP_STATE_DB', '/var/lib/apsp/core/state.db'))
if not database.is_file():
    raise SystemExit('找不到账本；自定义路径请设置 APSP_STATE_DB')
with sqlite3.connect(database.resolve().as_uri() + '?mode=ro', uri=True, timeout=5) as connection:
    connection.execute('BEGIN')
    sequence = connection.execute("SELECT value FROM kv WHERE key='usage:last_seq'").fetchone()
    pending = []
    for epoch, seq, batch in connection.execute('SELECT epoch,seq,batch FROM usage_outbox WHERE acknowledged=0'):
        pending.append({'epoch': epoch, 'seq': seq, 'payload_sha256': hashlib.sha256(batch.encode()).hexdigest()})
    baselines = [
        {'module': module, 'stat_name': name, 'epoch': epoch, 'uplink': up, 'downlink': down, 'observed_at': timestamp}
        for module, name, epoch, up, down, timestamp in connection.execute(
            'SELECT module,stat_name,epoch,uplink,downlink,observed_at FROM usage_baselines ORDER BY module,stat_name')
    ]
    ledger = {
        'last_seq': str(json.loads(sequence[0])) if sequence else '0',
        'pending_batches': sorted(pending, key=lambda row: int(row['seq'])),
        'pending_intents': connection.execute('SELECT COUNT(*) FROM intents WHERE completed=0').fetchone()[0],
        'baselines': baselines,
    }
(output / 'ledger.json').write_text(json.dumps(ledger, ensure_ascii=False, indent=2) + '\n')

names = ['APSP_PANEL_URL', 'APSP_USER_ID', 'APSP_NODE_ID']
values = [os.environ.get(name) for name in names]
if any(values) and not all(values):
    raise SystemExit('面板摘要需要同时设置 ' + '、'.join(names))
if all(values):
    origin, user, node = values
    parsed = urllib.parse.urlsplit(origin)
    if parsed.scheme not in ('https', 'http') or not parsed.hostname or parsed.username or parsed.password or parsed.query or parsed.fragment or parsed.path not in ('', '/'):
        raise SystemExit('APSP_PANEL_URL 必须是 HTTP(S) origin')
    if not user.isdecimal() or not node.isdecimal() or int(user) <= 0 or int(node) <= 0:
        raise SystemExit('用户和节点编号必须是正整数')
    origin = origin.rstrip('/')
    password = getpass.getpass('管理员密码（不保存）：')
    client = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar()))
    def request(path, data=None):
        body = None if data is None else json.dumps(data).encode()
        headers = {} if body is None else {'Content-Type': 'application/json'}
        with client.open(urllib.request.Request(origin + path, data=body, headers=headers), timeout=20) as response:
            return json.load(response)
    code = os.environ.pop('APSP_E2E_TOTP_CODE', None)
    if code is None and os.environ.get('APSP_E2E_TOTP') == '1':
        code = getpass.getpass('当前 TOTP 验证码（不保存，请用未使用的新码）：')
    if code is not None and (len(code) != 6 or not code.isascii() or not code.isdecimal()):
        raise SystemExit('TOTP 验证码需为六位数字')
    request('/api/login', {'password': password, 'totp_code': code})
    del password, code
    try:
        usage = request('/api/plugins/sing-box/usage?' + urllib.parse.urlencode({'user_id': user, 'node_id': node}))
        node_data = request('/api/plugins/sing-box/nodes/' + node)
        deployment = request('/api/plugins/sing-box/servers/' + str(node_data['server_id']) + '/deployments')
        (output / 'panel-usage.json').write_text(json.dumps(usage, ensure_ascii=False, indent=2) + '\n')
        (output / 'panel-deployment.json').write_text(json.dumps(deployment, ensure_ascii=False, indent=2) + '\n')
        (output / 'selection.json').write_text(json.dumps({'user_id': user, 'node_id': node, 'server_id': node_data['server_id']}, indent=2) + '\n')
    finally:
        request('/api/logout', {})
PY
  printf '只读快照已保存：%s\n' "$evidence"
  printf '%s\n' '请人工比较各阶段数据；快照完成不等于完整实机验收通过。'
}

openrc_snapshot() {
  local name=$1 result=0
  rc-service -- "$name" status || result=$?
  printf 'StatusExit=%s\n' "$result"
  python3 - "$name" <<'PY'
from pathlib import Path
import sys

name = sys.argv[1]
try:
    supervisor = int(Path(f'/run/{name}.pid').read_text())
    children = Path(f'/proc/{supervisor}/task/{supervisor}/children').read_text().strip()
except (OSError, ValueError):
    print('SupervisorPID=unavailable\nChildPIDs=unavailable')
else:
    print(f'SupervisorPID={supervisor}\nChildPIDs={children}')
PY
}

case ${1:-guide} in
  guide) [[ $# -le 1 ]] || { usage >&2; exit 2; }; guide ;;
  snapshot) shift; snapshot "$@" ;;
  --help|-h) usage ;;
  *) usage >&2; exit 2 ;;
esac
