# 开发、构建与验证

除明确写出 `cd web` 的前端开发步骤外，命令在仓库根目录运行。代码位置见[目录与维护约定](repository.md)，文档入口见[导航](README.md)，脚本用途见 [scripts](../scripts/README.md) 与 [tools](../tools/README.md)。

当前 GitHub Actions 按 [AGENTS.md](../AGENTS.md#临时-ci-暂停2026-10-01-用户要求) 暂停。以下 CI 小节描述保留的工作流设计；暂停期间只做本地验证，不触发、重跑或恢复工作流，不把未运行/取消的 CI 记为通过。

## 验证后的空间清理

集中验证结束后，保留对应源码身份、结果摘要和最新一套必要签名制品，清掉本轮编译 `target`、临时 PostgreSQL、旧缓存与源码归档，以及已经停止的测试容器和虚拟机磁盘。测试源码留在仓库；生产数据库、密钥和其他项目材料不能随测试一起删除。先确认精确所有权和活动引用，再按明确名单删除，禁止全局 `docker system prune`。失败保留有界摘要，不保存多轮完整环境。该约定见 [AGENTS.md](../AGENTS.md#测试材料清理2026-10-03-用户最新要求)，本次实测清理结果见[清理记录](acceptance/test-material-cleanup.md)。

## 本地开发与测试

需要 Rust stable、PostgreSQL 16。仓库提交了 `web/dist`，不修改前端时可直接编译面板；修改前端时使用固定版本 **Bun 1.4.2** 管理依赖和运行构建，并同步构建产物。CI 与 Docker 前端构建使用相同版本。

Passkey 验证使用 `webauthn-rs`，源码构建面板还需 OpenSSL 开发库和 `pkg-config`（Debian/Ubuntu：`libssl-dev pkg-config`）。Dockerfile 已安装构建及运行库；Agent 无此新增依赖。默认 Rust 测试覆盖来源、权限、挑战、邀请和速率约束；完整虚拟认证器回归需设置 `APSP_PLAYWRIGHT_MODULE`、`APSP_CHROME_PATH` 后执行 `cargo test -p apsp-panel --test passkeys virtual_authenticator_browser_roundtrip -- --ignored --nocapture`。它使用 sqlx 创建的隔离数据库和回环 HTTP 服务，不启用后台插件，不访问真实云 API。该用例默认忽略，不能将默认 cargo test 结果作为浏览器/硬件认证验收。

尚未安装 Bun 时，按[官方安装说明](https://bun.com/docs/installation)安装指定版本，并确认当前终端可调用：

```bash
curl -fsSL https://bun.com/install | bash -s "bun-v1.4.2"
export PATH="$HOME/.bun/bin:$PATH"
bun --version  # Must report 1.4.2.
```

如果不使用 Compose 面板，可以单独启动一个开发数据库。在按[部署文档](deploy.md)生成 `.env`、且没有占用 5432 端口的环境中运行：

```bash
# Source only the generated configuration with URL-safe hexadecimal passwords.
set -a
. ./.env
set +a

docker run -d --name apsp-dev-postgres \
  -p 127.0.0.1:5432:5432 \
  -e POSTGRES_DB=apsp -e POSTGRES_USER=apsp \
  -e POSTGRES_PASSWORD="$APSP_DB_PASSWORD" \
  -v apsp-dev-postgres-data:/var/lib/postgresql/data postgres:16
until docker exec apsp-dev-postgres pg_isready -U apsp -d apsp; do sleep 1; done

export APSP_DATABASE_URL="postgres://apsp:$APSP_DB_PASSWORD@127.0.0.1:5432/apsp"
export DATABASE_URL="$APSP_DATABASE_URL"
export APSP_LISTEN=127.0.0.1:8080
export APSP_DATA_DIR="$PWD/data"
cargo run --locked -p apsp-panel
```

数据库名、用户和卷在首次执行时创建，面板启动时自动迁移。重用该容器时运行 `docker start apsp-dev-postgres`，继续使用最初的数据库密码。只使用本地原生 PostgreSQL 时，也可以先创建具有建库权限的开发角色和数据库，再把 `APSP_DATABASE_URL` 与测试用 `DATABASE_URL` 指向它；sqlx 集成测试会创建独立测试数据库。

另开终端开发前端，Vite 将 API 请求代理到 `127.0.0.1:8080`：

```bash
cd web
bun install --frozen-lockfile
bun run dev
# After editing the frontend:
bun run build
```

需要签名的本地集成测试显式用公开 TEST_ONLY 根构建；生产构建改用操作者自己的公钥 JSON 数组。公钥通过编译器环境变量固定在产物中，修改根必须重新构建。正式发布流程拒绝仓库中全部测试根。

提交前运行：

```bash
# DATABASE_URL must allow creating isolated test databases on PostgreSQL 16.
# This public TEST_ONLY root is for tests, never production.
export APSP_RELEASE_PUBLIC_KEYS="$(cat crates/protocol/tests/fixtures/public-keys.json)"
python3 tools/check-core-boundary.py
cargo fmt --check
(cd web && bun install --frozen-lockfile && bun test && bun run build)
cargo clippy --all-targets -- -D warnings
cargo test
git diff --check
git diff --stat -- web/dist
```

前端构建放在 Rust 检查之前，因为面板嵌入 `web/dist`。前端有修改时，检查并一并提交生成产物；已提交产物的重建一致性可用 `git diff --exit-code -- web/dist` 检查，不把本次正常生成的差异误判为失败。

`bun test` 运行 `web/tests/*.test.ts` 的数据与协议逻辑回归；`web/tests/*.mjs` 是使用本地 API 夹具的浏览器回归。修改登录/导航/路由时可选择已有的 `dashboard.mjs`、`server-operations.mjs`、`plugin-catalog.mjs` 和 `node-routes.mjs`，修改插件时再加对应插件场景。它们读取构建后的 `web/dist`，需要本地 Playwright/Chromium；可通过 `APSP_PLAYWRIGHT_MODULE` 与 `APSP_CHROME_PATH` 指定已安装位置，然后用 `bun web/tests/<场景>.mjs` 执行。浏览器测试不需要连接生产面板。

依赖真实上游二进制的测试默认标记为 ignored，设置 `APSP_TEST_SINGBOX` 后显式执行，不能把默认 `cargo test` 当作这些专项检查已经通过：

```bash
export APSP_TEST_SINGBOX=/绝对路径/sing-box
cargo test -p apsp-compiler --test compilation -- --ignored
cargo test -p apsp-adapter-singbox --test runtime -- --ignored
cargo test -p apsp-panel --test reality -- --ignored
```

CI 另外检查分层禁用词、Linux 构建与 Compose 启动持久化。CI 的通过状态需要以远端实际运行结果为准。

## 安全功能的开发与验证

面板启动时应用登录安全与设备退役迁移，已有数据库的管理员密码、授权和用量仍保留；共享或正式数据库迁移前按部署文档备份。登录与二步验证写操作依赖实际 TCP peer，新增 HTTP 测试服务需使用 `app.into_make_service_with_connect_info::<std::net::SocketAddr>()`，不能通过伪造转发头设置来源。限速保存在 PostgreSQL；同一代理下的开发请求共享 8 次/60 秒额度，没有测试或回环地址豁免。只有通过来源与全局两级额度检查的认证尝试才同时计数；因额度或并发限制返回 429 时不消耗额度。

使用上面的独立测试数据库与 TEST_ONLY 公钥，可以定向执行：

```bash
cargo test --locked -p apsp-panel \
  --test auth_security --test node_ports --test subscription_reset --test retirement
cargo test --locked -p apsp-agent-core --lib retirement::tests
```

面板测试通过真实 PostgreSQL 和 HTTP/WebSocket 检查 TOTP 会话绑定、过期和并发重放、限速与伪造转发头、443/默认端口分配和冲突、订阅旧链接失效，以及在线退役、离线软删除和回执验签。Agent 退役单元测试使用受控系统接口，检查操作互斥、待确认用量阻止清理、停服务失败、崩溃恢复和历史数据保留；它们不代替 Linux 上实际 systemd 停服验证。

私有面板 CA 回归在 Unix 上使用 OpenSSL 和 `mkfifo` 生成隔离 TEST_ONLY 素材，实际服务由已锁定的 tokio-rustls 0.26.6 承载 HTTPS 与 WSS，不关闭 TLS 验证。定向命令为 `cargo test --locked -p apsp-agent-core --lib panel_tls::tests` 和 `cargo test --locked -p apsp-agent-core --lib private_panel_ca`；它们覆盖旧配置、CA 文件预算及 FIFO 替换、真实注册／下载／设备签名认证／退役，以及未知 CA、错误主机名和过期证书。素材和进程只由测试拥有，不导入宿主信任库。单元测试不能代替专用 Linux 上真实 Agent 服务的整条连接与清理验收。

开发时不要把真实验证器秘密或设备凭据加入夹具。TOTP 确认与登录会消费验证码；自动化脚本需等待新的时间步，不能靠清空生产数据库的防重放状态重试。具体操作与部署所有者恢复步骤见[部署文档](deploy.md#管理员登录与二步验证)，API、状态码与订阅重置边界见 [API 文档](api.md)。

## 真实安装与 Reality CI

`real-e2e` 在 Ubuntu 24.04 amd64 runner 上复用本次 musl Agent 制品，按版本与构建脚本摘要缓存固定上游运行时。缓存恢复后仍校验版本、架构、完整构建标签和 SHA-256。仅在隔离 CI 中使用仓库公开的 TEST_ONLY 私钥签署测试 Release；Agent 和面板均编译对应测试公钥，制品名称明确标记 TEST_ONLY。Compose 启动面板与 PostgreSQL 后，在干净宿主通过独立预置的公钥、minisign 与 bootstrap 执行已签安装器，由 systemd 管理 Agent 与独立运行时。

本地客户端经非 root 运行时的 Reality 443 端口向回环夹具下载 2 MiB、上传 1 MiB，检查文件内容、上传响应与真实用户节点用量增量。暂停后连续 70 秒采样稳定、outbox 清空，再验证 Agent 重启不更换运行时 PID、运行时重载不重复入账；恢复同量流量时将面板新增量与只读账本新周期基准逐字节比较。同版本重装核对身份与用量连续性，缓存二进制、签名证明或缺少签名时必须被预检拒绝。端到端流程及私有 state 驱动见 [真实验收文档](e2e.md)。

人工验收已启用 TOTP 的面板时，在 `scripts/e2e-driver.py` 的子命令前加全局参数 `--totp`，或设置 `APSP_E2E_TOTP=1`，隐藏输入本次验证码；`scripts/e2e-real.sh snapshot` 的可选面板查询也支持该环境变量。非交互调用可一次性提供 `APSP_E2E_TOTP_CODE`，每次登录使用新码，脚本不保存种子、验证码或会话。TOTP 未启用时保持原调用即可；CI 的测试面板并不因此自动启用 TOTP。

CI 公开制品仅包含版本、阶段和精确用量摘要；完整配置、订阅凭据、设备身份、安装令牌和日志不上传。此 job 使用回环 HTTP 路径和本地 TLS 伪装目标，不覆盖外部 CDN、DNS、证书部署或云防火墙配置。

新 job 的实际执行结果见对应提交的 [Actions](https://github.com/theLucius7/apsp/actions)，验收范围只在该 job 通过后成立。运行 0.1.0 到 0.2.0 升级属于真机专项，不能由同版本重装测试推断。

## 原生平台与 OpenRC 检查

平台目标及服务边界见 [设备平台与能力](platforms.md)。日常 CI 同时构建 Linux libc × 架构、macOS ARM64、FreeBSD 与 Windows 双架构；原生 Agent 行为夹具和服务夹具使用公开 TEST_ONLY 证明，Python 签名支持仅属于 CI。FreeBSD Linux cross 构建编译同一个测试公钥，再由 FreeBSD 13.5/14/15 验证同一二进制。

Linux musl 任务运行 `scripts/ci-openrc-smoke.sh`，必须在隔离 Alpine 容器中执行。进程夹具通过真实 OpenRC 验证监督、HUP、权限、依赖树刷新和重复安装，安装前使用独立 minisign/bootstrap 验证静态安装器；同一任务另用实际 Agent 执行诊断服务、命名空间、失败、超时和完成任务不可重放检查。该进程夹具的缓存预检为明确的 stub，真实签名缓存与未完成操作由 Rust 测试和 systemd real-e2e 验证。
