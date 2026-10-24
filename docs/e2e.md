# 真实 Reality 验收驱动

`scripts/e2e-driver.py` 通过真实面板 API 创建一套专用服务器、节点、用户和授权，记录健康部署及精确用量。操作者负责准备 Linux/systemd 设备、通过独立可信的 bootstrap 安装、启动独立客户端、产生流量，以及重启或重载服务。完整人工验收边界仍见 `bash scripts/e2e-real.sh guide`。

驱动仅创建本次随机名称对应的资源，不修改或删除已有业务对象。每个创建步骤立即保存编号；若请求已提交但响应丢失，使用相同私有 state 重试可以按本次唯一名称恢复。不要删除 state 后在同一设备重新开始，也不要同时运行两个使用相同 state 的驱动。

## 准备与安装

需要 Python 3.9 或以上，无第三方 Python 依赖。Linux 和 macOS 可以驱动面板，受管设备仍必须是 Linux/systemd。先按[部署文档](deploy.md)独立配置可信 bootstrap 与发布公钥，并在面板导入协议兼容的已签名 Release。将面板地址、节点地址和伪装域名放在当前 shell 的私有环境变量中，真实地址、密码、脚本和证据均不得提交。

```sh
E2E_PRIVATE=$(mktemp -d)
chmod 700 "$E2E_PRIVATE"
export APSP_E2E_ADMIN_PASSWORD_FILE="$E2E_PRIVATE/admin-password"
# 使用安全编辑器写入管理员密码，再执行 chmod 600 "$APSP_E2E_ADMIN_PASSWORD_FILE"。
# 设置 PANEL_ORIGIN、NODE_PUBLIC_HOST、REALITY_SNI 为本次环境值。
python3 scripts/e2e-driver.py --state "$E2E_PRIVATE/state.json" prepare \
  --origin "$PANEL_ORIGIN" --public-host "$NODE_PUBLIC_HOST" --sni "$REALITY_SNI"
```

密码也可通过 `APSP_E2E_ADMIN_PASSWORD` 提供，缺省则隐藏输入。密码和 Cookie 不写入 state、不打印；state、安装描述、客户端凭据和阶段证据以 0600 保存，所在目录必须是仓库外的 0700 私有目录。

管理员已启用 TOTP 时，在驱动命令的阶段名之前加 `--totp`，例如 `python3 scripts/e2e-driver.py --state "$E2E_PRIVATE/state.json" --totp ready`，登录时会隐藏输入一次性验证码。非交互单次认证可通过 `APSP_E2E_TOTP_CODE` 提供验证码；`e2e-real.sh snapshot` 可设置 `APSP_E2E_TOTP=1` 隐藏输入，也支持同一验证码环境变量。每个命令都会重新登录，必须使用新时间步的验证码；同一 30 秒时间步不能重放，连续命令需等待下一步。验证码和种子不保存到 state 或证据中，验收无需关闭 TOTP；默认 CI 使用新建管理员，不启用 TOTP。

`prepare --port 443` 可显式指定节点监听端口，范围为 1–65535，保留统计接口 18085；省略时由面板在 20000–29999 分配。选择会在创建资源前保存，重试必须使用相同参数，旧版未记录显式端口的 state 仍可按自动分配模式恢复。已创建节点若被改端口，驱动会拒绝继续，避免把另一个配置当成本次验收。

`prepare` 在私有目录保存 `enrollment.json`，包含一次性令牌、面板来源、服务器编号及独立选择的 Agent 版本和标签；按凭据保管并安全传到专用设备。自动选择描述为 `version=latest`、`tag=null`，安装器识别目标设备后才选择最新兼容稳定版本；这两个字段不是已经安装的版本证据。面板不再提供可直接执行的安装脚本。令牌过期或准备升级时，重新签发安装描述；需要可复现的跨版本验收时明确指定真实已签版本：

```sh
python3 scripts/e2e-driver.py --state "$E2E_PRIVATE/state.json" install --refresh --agent-version 0.3.0
```

在已独立配置可信 bootstrap 的设备执行以下命令，令牌通过子进程环境传递，不打印到终端。`E2E_PRIVATE` 指向安全传入的私有目录：

```sh
sudo python3 - "$E2E_PRIVATE/enrollment.json" <<'PY'
import json, os, pathlib, subprocess, sys
descriptor = json.loads(pathlib.Path(sys.argv[1]).read_text())
environment = dict(os.environ, APSP_ENROLLMENT_TOKEN=descriptor["token"])
subprocess.run(["/usr/local/bin/apsp-bootstrap", "--version", descriptor["version"],
                "--panel", descriptor["origin"]], env=environment, check=True)
PY
```

Agent 与面板产品版本独立；接入选定版本必须已导入且协议兼容。已有未签名安装先按[发布文档](release.md)迁移验证缓存。历史 0.1.0→0.2.0 真机验收保留在 `PROGRESS.md`；当前签名流程及同版本重装不能替代那次跨版本验收。

跨版本升级专项先导入旧版和目标版的完整签名 Release，使用 `install --refresh --agent-version <旧版本>` 在专用设备接入；已发布的未签 0.1.0/0.2.0 不能通过改写摘要变成已签版本。核对实际旧版和基线后，在同一个 state、服务器编号和设备身份上执行 `install --refresh --agent-version <目标版本>`，以新的令牌重新运行上述可信安装器。升级前后保存设备公钥、身份文件摘要、账本、已应用配置、运行时 PID 与持续双向流量结果；两台新设备上的不同版本不能作为原地升级证据。缺少已签版本、协议不兼容和目标平台没有可安装制品现在分别报告原因。

## 部署预检

面板只在确认只读运行时预检后发布业务节点。先在设备上以 `0600` 写入 `/etc/apsp/fleet-policy.json`（至少含 `{"runtime_inspection":true}`）并重启 Agent，再执行：

```sh
python3 scripts/e2e-driver.py --state "$E2E_PRIVATE/state.json" preflight --timeout 600
```

`preflight` 为本服务器开启面板侧只读运行时检查，等待 Agent 声明该能力后采集预检；尚无部署时先确认首次空运行时安装并等待健康回执，再重新采集，最后确认预检恢复发布。首次空安装和确认前各重新验证一次管理员密码（启用 TOTP 时需新验证码）。失败只报告阻塞项名称与操作状态，不输出证据内容。

## 健康部署与独立客户端

```sh
python3 scripts/e2e-driver.py --state "$E2E_PRIVATE/state.json" ready \
  --agent-version 0.3.0 --timeout 600
```

`ready` 要求设备在线、目标版本等于已应用版本、健康为真且无部署错误，可同时核对 Agent 版本。只导出本次专用节点的 `client.json`，本地混合入口固定为 `127.0.0.1:2080`；初次成功保存 `ready` 用量基线，重复执行保留原基线。

如受管设备在容器中监听 20000、宿主将其映射到 28000，使用 `ready --client-port 28000`。这仅修改私有客户端 JSON 的目标端口，保留面板的原始端口，并在 state 记录本次客户端端口。公开地址必须可由独立客户端直达该端口；不要依赖仅代理 HTTPS 的反代来转发 Reality。

在独立客户端使用上游 sing-box 启动此 JSON，经 SOCKS 访问测试端点；同时完成可识别的下载和上传，再等待真实面板入账。自建测试端点应记录客户端实际完成字节数，避免把只有下行增长当作双向验收。

```sh
curl --fail --proxy socks5h://127.0.0.1:2080 https://example.com/
# 经同一 SOCKS 代理向受控 fixture GET 大文件、POST 上传文件。
python3 scripts/e2e-driver.py --state "$E2E_PRIVATE/state.json" traffic \
  --label first --after ready --min-uplink 1 --min-downlink 1 --timeout 180
```

可以把上下行阈值设为本次实际流量的合理下界。面板统计包含代理协议开销，不要求与 HTTP 文件大小严格相等。脚本仅对本次用户与节点查询，使用任意精度整数检查十进制总量、上下行同时增长且已确认用量没有倒退。

### 传输预算与超时诊断

隔离 Reality 验收每条 curl 固定为 **90 秒**，外层仅用 92 秒回收失控的 curl 进程；下载 2 MiB、上传 1 MiB，必须同时满足 HTTP 200、curl 退出 0 和完整载荷校验。`traffic --timeout 180` 是等待面板计量的预算，不会延长实际 curl 请求。公网人工请求也应事先固定时间预算、预期大小和次数，保存所有失败，不通过事后延时或重跑取代定位。

发生超时时，在停止本次夹具前按顺序记录：

1. 独立客户端至本地 SOCKS 入口的 TCP 可达性、客户端进程是否仍存在。
2. 代理服务器至受控 HTTP 目标的直接 TCP/HTTP，以及 Reality 伪装目标的 TCP/TLS；核对目标自身在同一时间预算内能否完整响应。公网目标的直连与代理请求须使用相同载荷、方法和时限。
3. 同一次代理请求的 curl 退出码、HTTP 状态、实际上下行字节、连接时间、首字节时间和总时长；例如 HTTP 200 但不足 2 MiB 且退出 28 仍为失败。
4. 相同时间窗口内客户端/运行时 PID、重启数、配置摘要与服务状态，宿主 CPU 数、负载、可用内存，以及异常发生在升级/重载之前还是之后。服务 active 不能证明传输恢复。

`scripts/e2e-traffic-evidence.py` 在隔离 CI 中保存固定字段白名单；失败时自动探测本次回环 HTTP、SOCKS 和运行时 TCP，以及本次私有 TLS 夹具并保留原退出码。它不会导出配置、凭据、完整响应或日志，不能用于任意公网地址。公网排查的地址、TLS 验证结果和私有日志由操作者保存在仓库外；不关闭 TLS 验证，不把生产节点用作负载夹具。

[Issue #6](https://github.com/theLucius7/apsp/issues/6) 还包含固定 90 秒下的间歇失败和同源码一次重跑成功，根因尚未确定。现有取证和本节步骤没有修复其根因；回环 CI 的某次成功也不证明公网持续传输全部通过。

## 暂停、重启和重载

停止独立客户端，关闭本次用户的全部连接。显式设置只读 Agent 状态命令；命令按 `shlex` 分词后直接执行，不经过驱动端 shell。操作者负责命令仅进行只读查询，下面使用本机设备示例：

```sh
export APSP_E2E_STATUS_COMMAND='sudo apsp-agent status'
# 容器示例：docker exec apsp-acceptance-agent apsp-agent status
# SSH 示例：ssh acceptance-host sudo apsp-agent status
python3 scripts/e2e-driver.py --state "$E2E_PRIVATE/state.json" verify \
  --label before --interval 35 --timeout 240
```

`verify` 每 35 秒获取新的面板用量和本地 Agent status，连续三次快照覆盖两个完整采样间隔；每次都要求 Agent 已连接、本地应用版本与面板相同、健康为真、`pending_batches=0`。`--interval` 可增加，但不能短于 30 秒；`--timeout` 适用于 CI 和较慢环境。

由操作者记录运行时 MainPID、设备公钥及本地身份文件摘要，重启 Agent，再检查 MainPID 未变。分别在 Agent 恢复和显式重载运行时后执行：

```sh
# 设备：systemctl restart apsp-agent.service
python3 scripts/e2e-driver.py --state "$E2E_PRIVATE/state.json" verify \
  --label after-agent --unchanged-from before
# 设备：systemctl reload apsp-singbox@main.service
python3 scripts/e2e-driver.py --state "$E2E_PRIVATE/state.json" verify \
  --label after-runtime --unchanged-from before
```

`--unchanged-from` 在每次采样立即断言已确认上下行与总量原封不动、设备公钥保持不变。仍有连接、重复入账、历史丢失或身份变化都会失败，不能等待这些差异消失后再宣称通过。外部重载时必须保持客户端暂停，避免非托管 HUP 的终值采样窗口影响结论。

恢复同一独立客户端，完成第二批上传和下载：

```sh
python3 scripts/e2e-driver.py --state "$E2E_PRIVATE/state.json" traffic \
  --label resumed --after after-runtime --timeout 180
```

升级后可重新执行 `ready --agent-version 0.3.0`（改为实际选择的版本），再用原稳定基线验证身份与已确认用量。设备身份文件、账本和运行时连续性仍应由操作者记录，不由面板公钥相等推断全部已通过。

## CI 中的在线退役

`scripts/ci-real-e2e.sh` 显式创建监听 443 的节点，独立客户端实际通过该端口完成双向 Reality 流量。TLS 伪装夹具在本次 Compose 网络的独立容器内监听 443，不发布宿主端口，使宿主 443 专用于受管运行时；夹具复用本次面板镜像中的 OpenSSL，证书仅以只读方式挂载。CI 在签名缓存拒绝、Reality 流量、重启、重载及同版本重装检查全部完成后，调用 `scripts/ci-retirement.py` 做最后一项验收。它要求 root、Linux、显式的一次性环境标志，以及固定的回环 CI 面板；使用本次私有 state 中的随机名称核对服务器，确认设备在线、Agent 版本一致且支持退役后，才发送一次删除请求。管理员密码通过继承的环境变量提供，不进入命令参数。此辅助脚本需要 Python 3.11 或以上，人工 `e2e-driver.py` 的操作范围保持不变。

退役阶段检查：

- 在线删除返回 204，服务器详情随后返回 404；超时或失败立即使 CI 失败，不会再次删除而转入离线分支。
- Agent 的本地退役记录已收到面板确认，身份目录中的设备私钥、服务器编号和面板来源文件，以及受管运行配置和 SQLite 配置快照均已清理。
- 使用账本文件保留，已确认的上下行基准和序列不变、outbox 没有未确认批次，面板的用户及节点历史用量仍等于 `after-reinstall`。允许末次采样推进时间，以及既有逻辑清理已确认的 outbox 行。
- Agent 以 78 正常退出且不自动重启，独立运行时不再运行。显式再次启动 Agent 仍退出 78，凭证不会重新生成；显式启动运行时则在配置文件预检处被拒绝。

成功结果写入私有临时目录的 `retirement.json`。总 CI 摘要仅收录白名单布尔检查和精确用量，不包含请求编号、回执签名、设备公钥、地址或令牌。辅助脚本本身不执行环境清理；主 CI 脚本最终统一清理本次创建的 Compose、服务和目录。实际 Linux/systemd 通过状态以对应提交的 Actions 结果为准。

## 证据与失败处理

每个通过的阶段将证据写入同名 JSON，并保存在 state 的 `checkpoints` 中。证据包括筛选用量、部署版本、设备公钥与版本，以及稳定阶段的本地 outbox 状态；真实证据保持私有。超时或断言失败返回非零，不写通过记录，不自动回滚、删除资源或操作账本。

`python3 scripts/test-e2e-driver.py` 覆盖创建响应丢失后的恢复、不重复创建或修改其他对象、双向增长、重复入账、身份变化、outbox 未清空及两个采样间隔的失败边界。此契约验证不能代替实际 Reality、systemd、升级与客户端流量验收。
