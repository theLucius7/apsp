# 部署与节点接入

所有命令在仓库根目录运行。

## 面板统一安装与运维

需要 Python 3、可访问的 Docker Engine 及支持 `up --wait` 的 Compose 插件。工具沿用本仓库的 Compose 和 Dockerfile，不自动安装 Docker、不修改 DNS/防火墙。核对 `deploy/release-public-keys.json` 与[发布信任根说明](release.md)后运行：

```sh
python3 scripts/panel.py install --public-url https://panel.example.com
python3 scripts/panel.py status
python3 scripts/panel.py doctor
python3 scripts/panel.py logs --tail 100 --follow
```

首次初始化生成权限 600 的 `.env`、独立随机数据库和管理员密码，使用仓库内正式发布公钥作为构建根。管理员初始密码在本机环境文件中读取，工具不打印。`install` 的 URL 是面板对外地址；宿主仍只绑定 `127.0.0.1:8080`，公网 HTTPS 需按下文配置反向代理。可先 `init --public-url ...` 再检查配置；`--port` 只在首次初始化时有效，已有环境不会覆盖。

所有命令支持 `--env-file /私有路径/.env`、`--project 项目名`，应与原安装保持一致。现有环境从旧流程创建时，请补齐经过核对的非空 `APSP_RELEASE_PUBLIC_KEYS` 再构建。Shell 中同名 APSP/Compose 环境变量不覆盖指定文件，避免误用数据库密码或项目卷；Docker 连接设置仍沿用操作者环境。

```sh
python3 scripts/panel.py backup --backup-dir /私有备份目录
python3 scripts/panel.py upgrade --backup-dir /私有备份目录
python3 scripts/panel.py stop
python3 scripts/panel.py start
```

- `upgrade` 使用当前检出的源码；同步并审阅源码后执行。先暂停面板，备份 PostgreSQL 与制品卷，再恢复原容器、构建新镜像和启动等待健康。数据库不可用、任一备份失败都会阻止构建升级；尽力恢复原容器。Dockerfile 的 `/healthz` 健康检查使 `--wait` 等待面板健康，启动/迁移失败返回非零，不自动降级数据库。
- 重复 `install` 保留密码、身份和数据卷；已有容器时同样先备份。若只剩旧数据库卷而无容器，拒绝跳过备份安装，应先用原镜像恢复服务再升级。`start` 不自动构建或替换已有容器，`stop` 保留全部卷。
- `install` 缺少环境文件时，先只读检查同一 Compose 项目的全部容器与数据卷；存在任一旧状态就拒绝生成新密码。请找回原私有环境文件后恢复服务并备份，不能用新生成的环境文件替代旧数据库凭据。`init` 只生成文件，不证明已有服务可使用新凭据。
- `doctor` 只读检查 Compose 配置、Docker、数据库及面板本机 `/healthz`。不验证公网 HTTPS、CDN、Agent 的实际连接或订阅代理流量。
- 备份目录 700、文件 600，含 `environment`、`database.dump`、`panel-data.tar.gz` 和 SHA-256 清单 `manifest.json`。只有清单 `complete=true` 且摘要匹配的目录才可作为完整备份。源码记录是备份时的当前 checkout，实际旧镜像 ID 另存为 `image`；镜像本体不包含在备份中，应另行保留。
- 恢复先在隔离环境演练：核对摘要、恢复私有环境文件、以原镜像创建数据库服务，在面板停止时用 `pg_restore` 恢复数据库并解包制品到其数据卷，再启动匹配版本面板。不要把旧备份直接覆盖到正在写入的生产数据库；应用迁移后回滚需同时恢复数据库、制品与匹配镜像。当前管理工具不自动执行恢复。
- 同一 checkout 的同一项目操作互斥。异常退出残留 `.local/panel-项目名.lock` 时，先确认没有其他安装/备份进程再删除该空目录；不要从多个 checkout 同时操作同一 Compose 项目。

本轮执行情况见 PROGRESS；脚本夹具验证不等于真实容器安装、恢复或生产升级已经通过。下方保留原 Compose 手动方式。

## 真机已观测的部署问题

- [Agent 版本选择](https://github.com/theLucius7/apsp/issues/3)：历史 0.1.0→0.2.0 真机专项曾需要覆盖旧安装脚本的版本与摘要。新的签名发布流程从已导入、协议兼容的 Release 选择 Agent，接入界面可以指定版本；面板产品版本不再决定 Agent 版本。
- [CDN 设备路径](https://github.com/theLucius7/apsp/issues/4)：实际部署中，经 CDN 的设备 HTTP 请求返回 403，直达 Caddy origin、保留原域名和 TLS 的请求返回 200。页面访问成功不代表安装、注册、制品下载和 WebSocket 路径均可用，应逐项验证；回环 CI 不覆盖 CDN。
- [节点监听端口](https://github.com/theLucius7/apsp/issues/5)：历史验收曾依靠容器端口映射。现在创建和编辑节点可直接指定 443 等可用端口；创建时留空才自动分配 20000–29999。主机已有服务占用端口、云防火墙与容器端口映射仍需单独检查。
- [公网请求超时](https://github.com/theLucius7/apsp/issues/6)：真实持续载荷中曾出现 15 秒请求超时，包含升级前的请求；90 秒预算的升级后双向载荷通过，但仍需继续诊断公网短超时。回环 CI 不认证公网请求均无错误。

## 用 Compose 启动面板

需要 Docker Engine / Docker Desktop 和带 `--wait` 支持的 Docker Compose v2。下面的命令在仓库根目录运行：

```bash
git clone https://github.com/theLucius7/apsp.git
cd apsp

# Create once; never overwrite credentials for an existing database.
python3 scripts/init-env.py --public-url http://127.0.0.1:8080

docker compose --project-name apsp --env-file .env \
  -f deploy/docker-compose.yml up -d --build --wait
```

构建前将 `.env` 中的 `APSP_RELEASE_PUBLIC_KEYS` 填为独立核对过的 minisign 公钥记录 JSON 数组，例如 `["公钥记录"]`。这是公开信息，但必须核对来源；缺失或空集合会拒绝签名制品操作。Compose 把它作为镜像构建参数传入 Rust 编译器，容器运行时环境不能更换根。修改后必须重新构建镜像。

访问 <http://127.0.0.1:8080>，使用 `.env` 中的 `APSP_ADMIN_PASSWORD` 登录。也可从 [.env.example](../.env.example) 手动创建配置，两个密码分别生成，不要填写相同值。数据库密码放入连接 URL，示例使用不需要额外转义的十六进制值。

初始化命令以 `0600` 权限创建配置，生成独立随机密码，并拒绝覆盖已有文件或符号链接。可用 `--port 18080` 选择宿主机端口，或用 `--output /path/to/private.env` 将凭据保存在仓库外，随后把同一路径传给 Compose 的 `--env-file`；命令不会输出密码。生成的信任根默认为空，构建前仍须独立核对并填写发布公钥。

首次启动自动创建 PostgreSQL 数据库、执行迁移并保存管理员密码散列。已有数据库再次启动时，修改 `APSP_ADMIN_PASSWORD` **不会重置** 已有密码；修改数据库容器的密码环境变量也不会重置已有数据库用户密码。

| 配置 | 用途 |
|---|---|
| `APSP_DB_PASSWORD` | 必填，PostgreSQL 密码 |
| `APSP_ADMIN_PASSWORD` | 必填，首次创建管理员的密码 |
| `APSP_RELEASE_PUBLIC_KEYS` | 必填的构建时公开信任根 JSON 数组；缺根时制品操作拒绝 |
| `APSP_PUBLIC_URL` | 必填，Agent 与用户实际访问的 HTTP(S) origin，不包含路径 |
| `APSP_BIND_ADDRESS` | 默认 `127.0.0.1`，宿主机监听地址 |
| `APSP_PORT` | 默认 `8080`，宿主机端口 |
| `RUST_LOG` | 默认 `info` |
| `APSP_ABUSEIPDB_API_KEY` | 可选，仅保存在私有环境配置；为空/无效时官方查询入口未启用且信息未知 |

接入远端 Agent **之前**，将 `APSP_PUBLIC_URL` 改为远端可访问的地址，例如自己的 HTTPS 域名，再执行 `docker compose … up -d --build --wait` 重建面板容器以应用环境变量；仅执行 `restart` 不会更新容器环境。公网部署应通过 HTTPS 反向代理；代理需支持 `/api/agent/v1/ws` 的 WebSocket 升级和长连接。若反向代理位于同一宿主机，可继续只监听回环地址；确需直接暴露端口时显式设置 `APSP_BIND_ADDRESS`。这里不配置防火墙，管理员自行保证面板和节点端口可达。

可参考 [Caddy 配置示例](../deploy/Caddyfile.example)，在已有 Caddy 配置中追加站点，使用保留示例域名并替换为部署域名，不覆盖其他站点：

```caddyfile
panel.example.com {
    reverse_proxy 127.0.0.1:8080
}
```

端口应与 `APSP_PORT` 一致。先校验 Caddy 配置再重载，检查 HTTPS 健康接口以及实际设备在线状态。Caddy 支持 WebSocket；若前面还有 CDN，应确认设备 API、安装脚本、订阅和长连接都能到达 origin，且不会被缓存或访问策略拒绝。

### 分开检查设备到 CDN 与 origin 的路径

在独立客户端和实际接入设备各执行一次公开域名检查，随后仅在获准直达源站的设备执行第二条命令。下面使用保留示例地址；替换为本次面板域名与源站地址。`--resolve` 只改变这次请求的 DNS 结果，URL、Host、TLS SNI 和证书核对仍使用面板域名；不修改 hosts，不关闭证书验证，也不自动跳转。

```sh
curl --silent --show-error --noproxy '*' --proto '=https' \
  --connect-timeout 5 --max-time 20 --output /dev/null \
  --write-out 'cdn http=%{http_code} connect=%{time_connect}s tls=%{time_appconnect}s total=%{time_total}s\n' \
  https://panel.example.com/healthz
curl --silent --show-error --noproxy '*' --proto '=https' \
  --resolve panel.example.com:443:192.0.2.10 \
  --connect-timeout 5 --max-time 20 --output /dev/null \
  --write-out 'origin http=%{http_code} connect=%{time_connect}s tls=%{time_appconnect}s total=%{time_total}s\n' \
  https://panel.example.com/healthz
```

分别保存操作者、设备、时间、curl 退出码和 HTTP 状态。DNS/连接/TLS 失败时 HTTP 状态可能为 `000`；CDN 403 而直达 origin 200 只定位到两条路径存在差异，还需在私有 CDN/反代请求日志确认哪个组件拒绝。两条路径都返回 200 只证明健康接口可达。

CDN 和反代应允许以下真实设备操作到达原有面板认证，保留 Authorization、查询参数、请求体和 WebSocket Upgrade。设备 API 不缓存、不使用浏览器验证码、交互式登录或会丢失 POST 的跳转；放行设备路径不取消面板认证。

| 路径 | 必须单独验证的行为 |
| --- | --- |
| `/install.sh`、`/install.ps1`、`/api/bootstrap/versions` | 带有效一次性令牌取得安装描述/签名版本列表；描述为 JSON，不作为 shell 执行；禁止缓存含令牌响应 |
| `POST /api/agent/v1/enroll` | 标准安装流程真实注册，返回服务器编号；仅在专用接入/升级步骤消费令牌，不用真实令牌做无效探测 |
| `GET /api/agent/v1/ws` | 完成认证与 101 升级，稳定收到实际心跳；普通 GET 的 401 不能代替 WebSocket 成功 |
| `/api/agent/v1/manifest`、`/settings`、`/update`、`/bundles/{rev}`、`/artifacts/{name}/{version}/{arch}` | 已认证 Agent 读取状态、设置、更新元数据、配置包和运行时/诊断制品，下载字节与签名校验完成 |
| `/api/agent/v1/telemetry`、`/commands`、`/commands/{id}`、`/probes`、`/probe-results` | GET/POST 与压缩遥测请求体保持原样，面板实际 ACK 和数据更新可见 |
| `/api/agent/v1/diagnostics` 及任务、章节、取消确认子路径；`/api/agent/v1/retirement/receipt` | 已启用功能按实际方法完成确认，不能只检查服务 active |

表内缩写子路径均位于 `/api/agent/v1/` 下。未认证设备 API 的面板 401 是预期鉴权拒绝；注册 403 应核对面板授权及 CDN/WAF，429 应核对限速后等待允许的重试时间。不要公开完整响应、令牌或 Authorization；注册错误仅显示状态与排查方向。当前 Agent 二进制从 GitHub/独立镜像匿名下载，面板旧 Agent 文件接口返回 409 是既定行为，GitHub 下载路径需另行验证。

面板镜像默认使用两个 Rust 编译任务，降低 LTO 构建时的内存压力。资源充足时可先执行 `docker compose --env-file .env -f deploy/docker-compose.yml build --build-arg CARGO_BUILD_JOBS=4 panel`，再执行 `up -d --wait`；后一步不要增加 `--build` 以覆盖刚才的参数。

PostgreSQL 没有映射到宿主机端口。`postgres-data` 保存数据库，`panel-data` 保存制品；面板进程以 UID/GID `10001:10001` 运行。普通更新保留命名卷：

```bash
docker compose --project-name apsp --env-file .env -f deploy/docker-compose.yml ps
docker compose --project-name apsp --env-file .env -f deploy/docker-compose.yml logs --tail=100 panel
docker compose --project-name apsp --env-file .env -f deploy/docker-compose.yml up -d --build --wait
```

停止使用 `docker compose … down` 即可；`down --volumes` 会删除数据库和制品数据，只用于明确要清空的测试环境。更新和迁移前应备份数据库、制品目录以及 Agent 的身份与状态目录。

## 管理员登录与二步验证

管理员及 sing-box 代理用户现支持 Passkey，开通、恢复和部署要求见 [Passkey 登录与开通](passkeys.md)。请将公开地址设置为实际使用的 HTTPS 域名；IP 地址或公网 HTTP 会禁用 Passkey，保留原密码入口。代理用户从插件内生成一次性开通链接，不能使用管理员会话或订阅令牌登录。

登录后进入“账户安全”，输入管理员密码开始设置，将显示的秘密或 `otpauth://` 导入地址添加到验证器，再在 5 分钟内输入六位验证码确认。秘密只在本次设置页面显示，离开后需重新生成；确认之前仍按原方式登录。启用成功会退出其他管理员会话，保留当前会话。后续登录同时提交密码和验证码，关闭二步验证也需要密码与尚未使用的验证码。

验证码每 30 秒更新，允许前后各一个时间步，但成功消费的步号不能再次使用。确认启用、登录或关闭后如需立即进行下一次验证，请等待验证器更新；保持手机和面板主机时间同步。

修改登录密码在「系统 › 账号安全 › 登录密码」：输入当前密码（启用二步验证时还需验证码）和至少 12 个字符的新密码。修改成功会退出其他管理员会话并撤销所有者的管理员 Passkey，保留当前会话；首次部署时 `APSP_ADMIN_PASSWORD` 只用于创建管理员，之后修改它不会改变已有密码。

登录、修改密码与二步验证写操作共用限速：每个实际 TCP 来源 IP 在固定 60 秒窗口内最多接受 8 次，全局最多 64 次，并发验证最多 4 个。通过额度检查后的认证成功和失败均计数，因额度或并发限制返回的 429 不消耗额度；重启面板不会清空窗口，收到 429 时稍后重试。反向代理后的用户共享代理地址额度，面板不信任 `X-Forwarded-For` 或 `Forwarded` 来绕过限制，本地验收也不豁免。

未提供恢复码。丢失验证器时，由部署所有者经受保护的数据库管理通道恢复；普通管理员密码不能单独关闭 TOTP。确认拥有主机与数据库管理权、备份数据库并记录操作者和时间后，可运行以下事务，清除二步验证状态并使全部管理员会话失效：

```bash
docker compose --project-name apsp --env-file .env -f deploy/docker-compose.yml \
  exec -T postgres psql -U apsp -d apsp -v ON_ERROR_STOP=1 <<'SQL'
BEGIN;
SELECT id FROM admins WHERE id = 1 FOR UPDATE;
UPDATE admins SET
  totp_secret = NULL, totp_last_step = NULL,
  totp_pending_secret = NULL, totp_pending_expires = NULL,
  totp_pending_session = NULL
WHERE id = 1;
DELETE FROM sessions WHERE admin_id = 1;
COMMIT;
SQL
```

恢复不更改管理员密码；重新登录后立即设置新的验证器。TOTP 秘密保存在 PostgreSQL，数据卷和备份应按身份凭据保管。接口与状态码见 [API 文档](api.md#二步验证)。

## 导入签名 Release

当前公开 `agent-v0.3.0` 的签名安装器仍依赖面板二进制下载。新的独立入口核对其完整签名 proof，从 GitHub 下载对应 Agent，再使用固定官方 blob 中内嵌的可信 Linux 执行器安装，因此可兼容旧发布而不修改任何正式资产。旧 Agent 的自动升级能力仍需用新的兼容签名 Agent 手工迁移；新平台仍需发布对应正式签名制品。

旧 0.3.0 及更早 Agent 的签名不包含后来新增的完整验机启动门禁。恢复既有安装前须先通过受控维护停止旧 Agent；入口不会自行停用运行中的服务。已有配置需 Python 3.11 或更新版本严格解析，预检只读取配置指定的状态库，未显式配置时使用原 `/var/lib/apsp/core/state.db`。systemd/OpenRC 状态与本地状态端点无法明确确认停止，或 SQLite/WAL/SHM、检查点损坏、读取期间变化时拒绝安装。`Preparing` 的 NodeQuality 完整任务（包括旧缺 plugin/mode 字段）被拒绝，原 JSON 不改；`Started` 保留精确旧版本，旧 Agent 只继续其确切 r2 及原参数回收，其他版本/新参数要求兼容的新签名 Agent，不能降级后把历史变成失败。门禁放行 daily 不代表旧 0.3.0 适配器支持 mode，实际日常执行仍需相应兼容的已签 Agent/包装器。接入前及激活前重复预检，最后拒绝时恢复旧配置；这不宣称能阻止其他特权操作者在预检后另行启动服务。

正常部署无需在服务器编译运行时或 `docker cp` 制品。面板原“制品”页现为「插件中心 → [插件目录](plugin-catalog.md)」，只介绍插件和展示版本、架构，不再提供 Release 导入表单，也不在面板本机安装插件。旧 `/#/artifacts` 书签继续显示目录，新入口为 `/#/plugins/catalog`。

签名 Release 的准备属于部署维护流程。保留的管理员运维接口 `POST /api/artifacts/import-release` 按 [API 文档](api.md)的认证和 CSRF 要求提交已发布标签，可省略 `targets` 自动匹配服务器平台，或显式传入目标平台架构。自动匹配使用已接入服务器的上报信息；没有可用信息时默认面板宿主平台，新服务器与面板架构不同时应显式选择目标。ARM 目标仅下载兼容的 ARM 制品，不下载 AMD。完整签名 proof 始终保存并验证，草稿、缺签名、错误版本、归档或摘要不一致均拒绝。

相同标签可重复导入以追加其他架构，或修复缺失、损坏的下载文件；不会覆写同一签名身份的不同内容。已有完整导入目录保持兼容。只下载所需架构不改变正式 Release 对完整资产集合的要求。此接口只准备分发数据，不执行安装；移除网页表单不等于删除验签、分发或运维 API。

发布工作流从选定源码构建 Linux amd64/arm64 的 Agent、固定版本运行时、当前 NodeQuality 包装器、固定安装器、`release.json` 与 `SHA256SUMS`；维护者在本机签署清单，再上传 `SHA256SUMS.minisig`。已公开的 [agent-v0.3.0](https://github.com/theLucius7/apsp/releases/tag/agent-v0.3.0) 固定在源码 `75cd846`，包含 r2 包装器，正式签名与面板导入已验证。当前源码默认包装器为 r14，须完成对应能力验收后另行构建、签署和发布，不能覆盖已发布 r2，或借旧 Release 的验收宣称新能力已通过。

Linux musl 静态 Agent 保留原制品目录。GNU、macOS、Windows、FreeBSD 与完整运行时的实现和手动验证入口继续保留，详见 [设备平台与能力](platforms.md)。当前主线 `ci.yml` 也包含全平台检查定义，但所有工作流均按用户要求临时暂停；全部任务完成后统一确定恢复范围，见 [协作规则](../AGENTS.md#临时-ci-暂停2026-10-01-用户要求)。原生生产部署仍需独立验证来源的已签平台 bundle，不能直接使用日常 CI 的 TEST_ONLY 制品。

面板核对签名、仓库/tag、架构、版本、归档内容和安装后二进制摘要，验证完成后公布所选制品的本地清单。相同组件版本不能用不同内容覆盖。Agent 下载后独立以自身内嵌公钥再次验证，运行时服务启动前也复验本地签名缓存。

签名发布和自建公钥的完整步骤见 [发布与信任根](release.md)；编译规范见 [开发文档](dev.md)。CI 的测试公钥是公开测试夹具，禁止用于正式节点、镜像或发布。

## 复制安装命令

接入页选择 Shell 或 PowerShell，并选择“自动匹配”或真实已签版本。复制的一行命令可在目标服务器执行：

| 服务器 | 入口与权限 | 常驻方式 |
|---|---|---|
| Linux AMD64/ARM64，GNU 或 musl | Shell，root 或具备 sudo 的管理员 | systemd / OpenRC |
| macOS ARM64 | Shell，root 或具备 sudo 的管理员 | launchd |
| FreeBSD AMD64/ARM64 | Shell，root 或具备 sudo 的管理员 | rc.d |
| Windows AMD64/ARM64 | PowerShell，普通终端触发 UAC 提升或直接使用管理员终端 | 计划任务 |

自动匹配在执行时识别本机系统与 CPU/ABI，选择最新兼容稳定版本；指定版本时只安装该版本。macOS AMD64、32 位系统和未知 ABI 会明确拒绝，不能借其他系统制品安装。需要对应平台的正式签名 Agent 发布；版本下拉只提供已经导入完整签名 proof 的版本。没有签名制品的平台会提示由维护者准备对应发布后重试；可从插件目录查看已收录版本。

命令从官方 GitHub 固定 blob 下载自包含入口，核对 SHA-256 后才执行。Linux/FreeBSD 自动使用系统软件源准备依赖，macOS 在缺少 Python 时安装固定官方 pkg 并准备固定 minisign，Windows 自动准备本机架构 minisign。目标服务器需能访问官方 GitHub、平台依赖来源和面板；Linux 需运行中的 systemd 或 OpenRC。

Agent 资产缺失或缺少对应架构时，由维护者检查可信 GitHub Release。运行时及诊断制品的本地缺失可按目标架构重复导入修复。

入口独立验证正式根、完整发布 proof 和本机兼容性。即使面板只缓存 ARM，AMD 服务器也可安装同一签名发布的 AMD Agent：入口直接从 GitHub 或服务器已配置的独立 HTTPS 镜像取对应目标，不向 GitHub/镜像发送接入令牌或设备凭据，也不因此下载运行时和其他架构。面板不提供 Agent 二进制；软链路径拒绝。重复安装保留原设备身份，缓存预检失败时不会切换服务；令牌过期或已使用时重新生成命令。

首次信任来源为固定官方 HTTPS 渠道与已批准的入口公钥，不从面板下载新的发布根。`/install.sh` 和 `/install.ps1` 仅提供安装描述 JSON，不能管道执行。自建根、离线部署或需要独立预置验证器时使用下一节。决策与适用范围见 [ADR 0037](adr/0037-bootstrap-and-selective-import.md) 与 [ADR 0041](adr/0041-cross-platform-enrollment.md)。Windows/macOS/FreeBSD 入口的函数与签名测试不替代真实平台服务安装验收。

## 手动准备可信 bootstrap

手动安装的验证器和根公钥必须先从面板以外的可信渠道获得，并由操作者独立核对。不能执行面板提供的 `curl … | sh` 来建立信任，也不能先执行未知的新 Agent 让它验证自己。

在已核对来源的本地仓库中，检查 `tools/bootstrap.py`、`tools/release.py` 和发布公钥，再在目标 Linux 节点安装可信 bootstrap。以下命令仅安装已经由操作者核对的本地文件：

```bash
sudo apt-get update
sudo apt-get install -y python3 minisign ca-certificates curl coreutils passwd
sudo install -d -m 755 /usr/local/lib/apsp /etc/apsp/trust
python3 tools/release.py render-installer --template deploy/install.sh.tmpl --agent-unit deploy/apsp-agent.service --runtime-unit plugins/sing-box/apsp-singbox@.service --output /临时目录/trusted-install.sh
sudo install -m 644 /临时目录/trusted-install.sh /usr/local/lib/apsp/trusted-install.sh
sudo install -m 755 tools/bootstrap.py /usr/local/lib/apsp/bootstrap.py
sudo install -m 644 tools/release.py /usr/local/lib/apsp/release.py
sudo install -m 644 /已独立核对的路径/public-keys.json /etc/apsp/trust/public-keys.json
sudo tee /usr/local/bin/apsp-bootstrap >/dev/null <<'SH'
#!/bin/sh
exec python3 /usr/local/lib/apsp/bootstrap.py "$@"
SH
sudo chmod 755 /usr/local/bin/apsp-bootstrap
```

bootstrap 的公钥文件仅用于独立确认安装起点；它不会写进 Agent 的运行时配置，也不能替换 Agent 编译时的公钥。后续版本或根轮换须由已信任的旧根批准，私钥泄漏恢复须走独立可信渠道，见发布文档。

## 接入 Debian 12 并使用节点

1. 在全新 Debian 12 amd64/arm64 服务器安装基础工具：

   ```bash
   sudo apt-get update
   sudo apt-get install -y python3 minisign ca-certificates curl coreutils passwd
   ```

2. 先按目标服务器架构导入签名 Release。在后台“服务器”点击“添加服务器”，填写名称并选择监控频率：实时为 1 秒采样 / 3 秒上传、均衡为 3 / 10 秒、轻量为 10 / 30 秒，也可自定义 1–60 秒间隔，上传不能快于采样。按需切换公网地址识别、自动更新，并添加初始 TCP/ICMP 拨测目标；默认不创建任何拨测。点击“创建并继续”一次保存这些配置，进入接入页面，选择 Agent 版本并复制完整安装命令到目标服务器执行，官方入口会自动准备验证工具。以下步骤使用 Debian 12 的 systemd；Linux 安装器也支持运行中的 OpenRC，缺少这两种服务管理器的容器不能直接安装。OpenRC 依赖及 macOS、FreeBSD、Windows 的原生服务入口见 [设备平台与能力](platforms.md)。令牌 24 小时有效且只可消费一次。

   新增和编辑表单也支持地区、分组、标签、成本、到期记录及按账单日计算的网卡流量额度。金额与字节使用精确字符串，月重置使用 UTC；日期自动顺延只维护记录。用法、迁移和观测边界见[服务器资产与流量额度](server-assets.md)。

3. 接入页可见时每 3 秒检查设备状态，分别显示服务器创建、设备注册与设备上线。安装完成后检查系统信息与最新指标；采样及拨测设置在设备同步后生效。命令获取失败可直接重试，无需重新添加服务器；过期命令不再显示，需重新生成。关闭弹窗后可从服务器详情的“接入 / 升级”继续。已有设备在线仅表示当前状态，不代表安装或升级已经完成，应另外核对上报版本。也可在设备上执行：

   ```bash
   sudo apsp-agent status
   sudo systemctl status apsp-agent.service --no-pager
   sudo journalctl -u apsp-agent.service -n 80 --no-pager
   ```

4. 在此服务器上创建节点，填写客户端可达的公开地址及可访问、支持 TLS 的伪装域名。监听端口留空时自动分配 20000–29999 中的空闲端口，也可指定 443 等 1–65535 内的端口；18085 保留给统计接口。面板拒绝同一服务器上其他有效节点占用的端口；Caddy 等主机程序是否占用、云防火墙是否放行仍需自行检查。面板自动生成节点密钥，无用户授权的节点不开放入站端口。
5. 创建用户并授权节点；连续变更合并 5 秒后自动发布。服务器详情应显示目标版本等于已应用版本、当前配置健康。失败后的健康回滚和最近错误会分别显示，不应把旧配置仍健康视为新版本已成功。
6. 在用户页复制订阅，导入支持该配置的客户端。通用格式为 base64 多行 VLESS 分享链接；`singbox` 格式为 sing-box JSON，提供 `127.0.0.1:2080` 混合代理入口。订阅只包含此用户当前有效、已应用且健康的节点。
7. 通过代理产生可控下载/上传，1–2 分钟后检查用户页“该用户的节点流量”。服务器网卡计数与代理用户流量是不同口径。

设备的关键路径：

| 路径 | 内容 |
|---|---|
| `/etc/apsp/agent.toml` | Agent 配置 |
| `/etc/apsp/identity/` | 设备身份和绑定的面板 origin，需保留 |
| `/var/lib/apsp/core/state.db` | SQLite 账本、意图、已应用状态与待确认批次 |
| `/run/apsp/agent.sock` | 权限 0600 的本地 status socket |
| `/opt/apsp/core/current/` | 当前 Agent 二进制 |
| `/opt/apsp/plugins/sing-box/current/` | 当前运行时二进制 |
| `/var/lib/apsp/plugins/sing-box@main/current/` | 当前原生配置 |

代理服务为 `apsp-singbox@main.service`，使用独立非特权用户，systemd 授予绑定 443 等低端口所需的 `CAP_NET_BIND_SERVICE`；统计 API 仅监听 `127.0.0.1:18085`。Agent 以 root 运行，特权操作通过内部 trait 边界执行。

编辑节点可修改监听端口；新配置应用成功后，客户端需更新订阅。省略端口的 API 更新保留现值，未实际改变字段的节点更新不会重复安排发布。

### 重置订阅链接

在用户页打开订阅窗口，点击“重置链接”并确认。旧地址立即失效，新地址仍对应原用户及授权；需将新地址交给对应用户并更新客户端订阅。此操作不改变代理 UUID、已应用配置或流量记录，已下载的配置仍可连接。需要终止代理访问时，应撤销该用户的节点授权。

### 升级设备

Agent 支持自动更新，默认关闭。安装好的监督服务可在服务器详情“Agent 设置”中开启“自动更新 Agent”，从绑定面板选择协议兼容、匹配平台且签名通过的新稳定版本；正常检查间隔约六小时并附加抖动。更新保留身份和账本，代理运行时独立运行；试运行失败或未确认的更新中断会恢复旧 Agent。具体平台与恢复边界见 [设备平台与能力](platforms.md#监控任务与更新)。

手动升级时，导入协议兼容的新签名 Release 后，到原服务器详情点击“接入 / 升级”，可指定已导入的 Agent 版本，签发**新的**一次性令牌，再执行可信 bootstrap 命令。面板和 Agent 产品版本无需相同；普通首次安装使用页面的独立单行入口；自建根/离线流程继续独立预置验证器，重复安装也可明确指定已信任的旧 Agent 验证下一版。

安装器先核对签名和实际内容，预检既有运行时、诊断和未完成回滚引用，再注册暂存 Agent、切换当前版本并重启 Agent。身份、配置与状态库继续保留；同一服务器只允许原公钥再次注册，不能复制其他设备的身份目录。旧命令消费后不可复用。

历史未签名缓存不会被自动认可；预检失败保持旧安装，明确报错。需按发布文档提供与实际二进制完全匹配、经过独立验证的签名证明，或安排迁移维护窗口。历史 0.1.0→0.2.0 连续升级验收不等于已经完成旧未签版本到新签名版本的迁移。

### 自有 CA 的面板连接

使用自有 CA 的 HTTPS 面板时，节点操作者先通过独立可信渠道核对并准备 CA，再在 Agent 本机配置的顶层指定证书文件；在注册设备之前设置即可：

```toml
panel_url = "https://panel.example.invalid"
panel_ca_file = "/etc/apsp/trust/panel-ca.pem"
```

参见 [配置示例](../deploy/agent-private-panel.example.toml)。该字段省略时继续使用公有 webpki 根；设置后附加证书只用于此 Agent 的面板注册、制品与配置下载、认证 WSS 和退役回执。面板下载仍要求同一 origin，证书主机名与有效期校验继续生效。GitHub 下载、公网地址探测、安装入口及独立代理运行时不会加载此文件。

文件最多 256 KiB、32 张完整 PEM 证书，不能包含私钥或其他 PEM 块；路径必须是绝对普通文件，文件及其上级目录都不能为符号链接。macOS 等存在系统目录别名的平台应填写真实路径，例如 `/private/etc/apsp/trust/panel-ca.pem`。将 CA 放在设备身份、运行配置与安装目录之外，确保退役清理凭据后仍可确认回执。CA 与发布制品公钥各自独立，添加它不会授权未签名制品。修改 CA 后重启 Agent，使所有面板连接使用新信任；仅更新文件不会替换现有 TLS 连接的信任集。详细边界见 [ADR 0074](adr/0074-private-panel-certificate-authorities.md)。

### 远程命令的本地授权

远程命令默认关闭。只有节点操作者可在本机 Agent 配置的顶层设置 `allow_remote_commands = true` 并重启 Agent 来开启；例如 Linux 的 `/etc/apsp/agent.toml`。此字段不属于 `[settings]`，面板的 Agent 设置接口不能启用它。保持 `false` 或省略字段时，Agent 不领取远程命令，界面也禁用提交；旧响应缺少能力字段时同样默认禁用。

开启此项等于授权绑定面板以 Agent 服务账号执行任意 shell 命令，Unix 通常为 root，Windows 为 SYSTEM。制品签名只约束已签制品，不能限制这些命令的内容或效果；仅在接受这一信任范围时开启。关闭时在本机恢复 `false` 并重启 Agent。

### 删除与退役设备

删除在线且支持退役的 Agent 时，面板先发送退役请求。Agent 阻止新操作、等待正在执行的受管操作，采集可观测的最后用量并停止运行时和诊断；已持久用量全部确认后，清除设备凭据和运行配置，再提交签名完成回执。面板验证后删除记录、撤销设备会话和接入令牌，移出节点列表和订阅。已确认历史用量保留，退役 Agent 停止且重启不会恢复代理。

在线删除最多等待 20 秒；失败或超时保留记录与原退役请求，可重试。在线旧 Agent 不支持退役时会提示先升级。判断以实际 WebSocket 连接为准，列表“在线”按最近消息显示，可能稍有延迟。

**离线删除没有设备清理确认**：只软删除面板记录、撤销面板接入及未完成诊断，离线机器仍可能运行最后配置。需在设备本地停止相关服务、清理身份和运行配置。在线请求失败后再次主动删除，若设备已离线，也会走此分支；尚有用量未确认时撤销认证会阻止自动完成清理。只有已清理且保存签名回执的设备才能稍后补交确认。退役状态不会被普通重复安装清除，重新使用该机器需先明确完成本地清理。

清理只涉及 Agent 托管的身份和配置，保留历史计量、退役记录和公开制品，不承诺物理介质安全擦除。故障恢复与确认边界见 [ADR 0019](adr/0019-server-retirement.md)。

## IP 质量与 NodeQuality 报告

服务器详情页显示 Agent 上报的 IPv4/IPv6。网卡地址在连接建立时和运行中更新；NAT 服务器可在 `/etc/apsp/agent.toml` 增加公网地址，随后重启 Agent：

```toml
# Documentation addresses only; replace with this server's actual public IPs.
public_ips = ["192.0.2.10", "2001:db8::10"]
```

点击“刷新 IP 质量”时，由面板访问 NodeQuality 使用的 [IPQuality](https://github.com/xykt/IPQuality) 数据库接口，查询位置、ASN、用途、风险及代理等信息。各数据库独立展示，包含更新时间、原始字段和错误；第三方数据可能缺失或互相矛盾，不合成为一个无依据的总分。私网和回环地址不向外部接口查询。本次开发环境对该接口的实际请求返回 403，因此记录了服务错误；成功字段解析和失败处理通过受控 HTTP 夹具验证，不能据此宣称线上数据库服务当前可用。

日常检查使用已导入签名 Release 中的 NodeQuality 外插；完整验机目前暂停新任务，原因在界面显示。单独编译或拷贝未签名目录不能代替验签导入。外插固定 [NodeQuality 上游提交](https://github.com/LloydAsp/NodeQuality/tree/a92fca6c0067df29ddd03fdc2fee6f3000f64545)，保留原样源码和许可证，版本为 `a92fca6c0067df29ddd03fdc2fee6f3000f64545-r14`。旧制品不能覆盖；历史顶层上传开关不能证明内层脚本零上传；旧排队完整任务保存明确失败原因，已有 Started 继续收集与取消，不重新执行。

r14 保留固定上游来源与原文许可证，执行时不下载或安装 NextTrace 等工具；daily 所需工具须由维护者预置，缺失时明确拒绝对应执行。固定辅助脚本与静态数据须在整份源码校验完成后使用，不回退在线 main。外插工作路径不能包含空白或 shell 通配符，使用默认目录即可。

日常目标服务器需要 Linux systemd、root、Bash、Python 3 与面板制品访问；已配置 TCP 目标决定实际探测范围。最小 Debian 系统可先安装：

```bash
sudo apt-get update
sudo apt-get install -y bash curl python3 ca-certificates
```

服务器详情的“日常检查”只查询逐源 IP 缓存和已配置的有限 TCP 目标，不执行硬件、rootfs 或公开测速；固定 64MiB/32tasks，仍要求 256MiB 启动预留与 2GiB 磁盘。完整验机按钮禁用并显示离线受控工具链未就绪。新面板与 Agent 应一起升级；门禁前旧 Agent 已领取任务需升级或取得取消确认，面板不能撤回已返回的 HTTP。已有任务的章节、文本、原版本和签名身份保留，每台服务器仍只允许一个任务。

界面显示排队、运行、成功或失败，并保留本地文本报告及可用的在线链接。在线上传失败时，本地报告仍可查看。固定上游在正常清理分支也返回 1；历史 r5 包装器仅在确认原入口 `main → post_cleanup` 的末尾退出分支且完整本地报告通过校验时将该特例记作成功，原返回值仍保存在 `upstream-exit.txt`。任意早退、信号清理、清理拒绝或缺报告不会因此成功；可选上传 HTTP 403 或传输失败单独显示告警。任务有整体运行时限；未安装依赖、上游下载失败、报告缺失和超时均返回错误。上游 chroot 用于隔离测试文件，systemd 使用独立挂载命名空间处理清理，不提供针对不可信程序的安全沙箱。[ADR0016](adr/0016-nodequality-diagnostics.md) 记录旧工具链的外网例外；新完整执行已由 [ADR0031](adr/0031-nodequality-full-start-gate.md) 暂停，不能以这一例外绕过门禁。

面板报告文本最多 256 KiB，超过时显示截断说明；原始 `report.zip` 默认保存在节点的 `/var/lib/apsp/plugins/diagnostics/<任务 UUID>/`，可由管理员在节点本地读取。

## 实机验收与已知边界

`scripts/e2e-real.sh guide` 给出完整人工步骤；`snapshot` 在 Debian 设备收集当前状态、服务 PID、统计监听和只读账本摘要，可选保存按用户、节点筛选的面板用量。它不会安装软件、修改业务配置、重启服务或更改账本；可选的面板查询会创建临时管理员会话并在结束时注销。

```bash
bash scripts/e2e-real.sh guide
# Run as root on an already enrolled Debian 12 node:
export APSP_PANEL_URL=https://panel.example.com
export APSP_USER_ID=1
export APSP_NODE_ID=1
sudo --preserve-env=APSP_PANEL_URL,APSP_USER_ID,APSP_NODE_ID \
  bash scripts/e2e-real.sh snapshot before ./evidence
```

可选的面板摘要会在终端隐藏输入管理员密码，不把密码或会话写入证据。已启用 TOTP 时，在上述 `sudo` 命令前 `export APSP_E2E_TOTP=1`，并在 `--preserve-env` 中追加 `APSP_E2E_TOTP`，脚本会隐藏输入新的六位验证码。`scripts/e2e-driver.py` 可使用全局参数 `--totp` 请求同样的输入；非交互调用可通过 `APSP_E2E_TOTP_CODE` 提供本次验证码，脚本不保存秘密或验证码，每次新登录须使用尚未消费的验证码。不要把验证码写进命令历史、日志或证据文件。

证据目录仍可能包含主机名、用量和内部编号，应按私有运维数据保管。记录重启前、重启 Agent 后、重载运行时后以及恢复流量后的快照；在暂停客户端、等待连续两次采样稳定和待确认批次清零后比较，确认不会重复计入已确认流量。

Agent 在托管应用前读取终值，再打开新计量周期；外部强制重载或异常进程退出可能留下不可观测的短采样窗口，会记录告警。已经在本地 outbox 落盘的批次可重发，面板事务去重后确认；这不代表能恢复从未采集到的字节。脚本默认不修改 outbox 来伪造丢失确认，丢 ACK 重传由自动化集成测试覆盖。

未包含配额、计费、链式代理、其他代理协议、多管理员或权限体系。订阅 URL 是用户访问凭据，应仅交给对应用户。

## 正式 IP 查询来源

IP 信息页区分一个 check-place 聚合入口和 AbuseIPDB 官方接口。要启用官方查询，在私有 `.env` 中填写自己账户的 `APSP_ABUSEIPDB_API_KEY`，重建容器环境后生效；不要提交或粘贴密钥到面板/Issue/日志。没有密钥时不会访问该接口，页面提供不可用原因。凭据改变不删除既有快照；关闭入口后保存结果显示为历史。真实额度、授权和官方网络可达性需要单独验证，403/429 不会重试或更改 UA。

官方接口固定只读 CHECK、30 天报告窗口，不请求 verbose，不包含报告人资料或上传/写入。评分保留官方原值，不等于“干净”。面板查询不能证明节点流媒体解锁，IPQuality 节点自查目前未启用，详情见 [ADR 0027](adr/0027-ip-provider-adapters.md)。

## 服务器运营设置与 Agent 下载

升级到包含 `0018_server_operations.sql` 的版本后，后台「面板设置」配置公开看板，「告警设置」配置离线阈值与 Telegram。服务器新增和编辑中配置单节点告警、Agent 自动更新与 GitHub 镜像；详情可进行本期流量矫正。默认私有看板、离线阈值 5 分钟，Telegram 默认关闭。操作和计量语义见 [服务器资产与运营说明](server-assets.md)。

Agent 安装与自动更新从 GitHub Release 获取已签二进制，面板只下发更新元数据，旧 Agent 下载接口不再提供文件。部署前需要使用本次源码的独立可信 bootstrap；它包含从 GitHub 下载后安装已签 Agent 的静态 Linux 执行器，可兼容旧 0.3.0 的完整签名 proof，不修改正式 Release。新平台仍需发布对应签名制品；衔接说明见 [发布与安装说明](release.md)。本次源码工作不代表已发布新 Release 或完成多平台实机升级验收，CI 暂停安排继续有效。
