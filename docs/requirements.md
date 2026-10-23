# Apsp MVP：自主执行任务说明

> 本文件是一份完整的任务说明。执行者请从头读到尾，再开始动手。
> 第 0 节是工作方式，第 2 节是验收标准，第 12 节是执行顺序。

---

## 0. 给执行者的指令（先读）

**你的目标：** 在当前仓库（它将成为 `apsp` 主仓库）中，从零实现 Apsp 的最小可用版本（MVP），使第 2 节的验收场景可以跑通。按第 12 节的顺序推进，每一步都要有可运行的代码和通过的测试。

**工作方式：**

1. 你是在无人值守的情况下长时间运行，**不要停下来提问**。遇到歧义时，选择"最简单、且符合第 4 节原则"的方案，然后把问题、你的选择和理由追加到 `docs/open-questions.md`，继续往下做。
2. 开始前先创建 `docs/PLAN.md`，把第 12 节的每个阶段拆成具体任务。
3. 每完成一个阶段（G1、G2……），更新 `PROGRESS.md`：完成了什么、如何验证的、已知问题、下一步。然后提交一次 git commit（Conventional Commits 格式）。
4. 每个阶段结束前必须通过：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。
5. **不要实现第 3 节列出的"不做"项**，也不要为未来功能创建空目录或空模块。
6. 核心路径不允许只写 TODO。确实做不完的，在 `PROGRESS.md` 里写清楚缺什么、为什么。
7. 不要修改 sing-box 源码；不要在代码或配置里写入真实的密钥、域名、IP。
8. 所有 crate 顶部加 `#![forbid(unsafe_code)]`。
9. 代码标识符和代码注释用英文；`docs/` 下的文档、README、PROGRESS 用中文。
10. 如果环境网络受限，导致依赖下载、sing-box 构建或 Postgres 不可用：先完成不依赖它们的部分，相关测试标记为 `#[ignore]` 并写明原因，在 `PROGRESS.md` 中列出需要人工执行的步骤。
11. 时间不够时的优先级：G1–G7（后端与 Agent）优先于 G8（前端）。只有 API、没有界面的 MVP 也可以接受。
12. 只使用第 5 节列出的依赖。确需新增时，在 `docs/adr/` 写一条 ADR 说明理由。

---

## 1. 项目背景

Apsp（apsp）是一个自托管的服务器与代理网络控制平面：

- 一台主服务器部署**面板**（panel）。
- 其他服务器安装 **Agent**，Agent 主动连接面板。
- 面板保存"期望状态"，把它**编译**成每台服务器的原生 sing-box 配置；Agent 负责**对账**（让本机状态与期望一致）、上报服务器信息和按用户统计的流量。
- sing-box **不改一行源码**，从上游 tag 原样构建，只额外启用官方已有的 `with_v2ray_api` 构建标签。

---

## 2. MVP 定义（验收场景）

MVP 完成的标志是下面这条链路可以完整跑通：

1. `docker compose up` 启动面板和 PostgreSQL，用初始管理员密码登录。
2. 管理员把带 `with_v2ray_api` 的 sing-box 构建产物和 Agent 二进制放进面板的制品目录。
3. 在面板"添加服务器"，得到一条安装命令；在一台全新的 Debian 12 服务器上以 root 执行。
4. 30 秒内，这台服务器在面板上显示为在线，并显示系统信息和实时指标。
5. 在这台服务器上创建一个 **VLESS + Reality** 节点，面板自动分配端口、生成密钥并发布。部署状态显示"已应用、健康"。
6. 创建一个用户并授权使用这个节点，面板自动重新编译并发布，得到该用户的订阅链接。
7. 把订阅导入 sing-box 客户端，可以正常上网。
8. 1 至 2 分钟内，面板能看到这个用户在这个节点上的上传和下载流量。
9. 重启 Agent、重载 sing-box 之后，流量不会重复计算，也不会丢失已上报的数据。

---

## 3. 范围

### 做

- 面板：单管理员登录、服务器管理与接入、VLESS+Reality 节点、用户与节点授权、自动编译与发布、部署状态、流量汇总、订阅（分享链接格式 + sing-box JSON 格式）、制品下载、安装脚本。
- Agent：注册、长连接、遥测、对账与应用、sing-box 适配器、流量账本、本地状态库、`status` 命令。
- 编译器：模型 → sing-box 1.14 服务端配置，带黄金文件测试。
- 部署文件：systemd 单元、OpenRC 服务脚本、安装脚本模板、面板的 docker-compose、sing-box 构建脚本。

### 不做（严禁实现）

链路、转发、链式代理、外部出口、出口池；用户分组；配额强制执行与计费；DDNS；WebSSH；frp；Shadowsocks 与 SSM API；VLESS+Reality 以外的任何协议；xray；多个 sing-box 实例；独立的特权 helper 进程（只定义 trait）；防火墙和 nftables；正式的自更新机制（MVP 靠重新执行安装脚本升级）；非 Linux 运行平台；Clash 订阅格式；多管理员与权限；多语言界面（界面只用中文）；面板高可用。OpenRC 已按用户追加要求纳入，详见 ADR 0017；非 Linux 编译产物范围见 ADR 0015。

### 可选加分项（只有在 G1–G9 全部完成后才做）

后续用户已明确要求补齐 Agent 监控、任务、自更新与多系统常驻运行，覆盖上面的对应排除项，并以 ADR 0018 为实现约束；MVP 的其他排除项保留。

1. 制品签名：对 `SHA256SUMS` 做 ed25519 签名，Agent 校验。
2. 服务器详情页的"高频模式"：有人查看时指标上报频率提高到 2 秒。

---

## 4. 已确定的架构决策（不得更改）

1. **声明式全量快照。** 面板为每台服务器编译完整配置，带单调递增的版本号（rev）。Agent 拉取、校验、应用、回报。心跳携带已应用版本，面板发现不一致就重新通知。Agent 本地保留最后一份可用配置，面板离线时节点照常运行。
2. **Agent 三层结构。** `agent-core`（原生功能）只依赖 `adapter-sdk` 和 `protocol`；`adapter-*` 只依赖 `adapter-sdk`；只有 `agent` 二进制入口同时依赖全部 crate，负责把适配器注册进 core。`agent-core` 源码中不得出现 `singbox` 或 `sing-box` 字样（CI 用 grep 检查）。适配器是无状态的翻译层：不持久化任何东西，不连接面板。
3. **sing-box 以独立的 systemd 或 OpenRC 服务运行**（分别为 `apsp-singbox@main.service`、`apsp-singbox@main`），不是 Agent 的子进程。Agent 重启不影响代理服务。
4. **计量只在用户认证的那一端计算一次。** 读取统计时 `reset=false`，累计读取，在本地计算差值并持久化，带序号上报；面板按 (server_id, epoch, seq) 去重后确认。
5. **统计用户名按"成员 + 节点"生成**：`u{user_id}_n{node_id}`。原因见第 9 节。
6. **特权操作经过 `Privileged` trait。** MVP 中 Agent 以 root 运行，trait 的实现直接在进程内执行；以后替换成独立 helper 进程时，调用方不用改。
7. **意图日志。** 每次应用前在状态库写一条意图记录，完成后标记。Agent 启动时先处理未完成的意图。
8. **版本合并。** 正在应用某个版本时又来了多个新版本，完成后直接应用最新的那个，跳过中间版本。
9. **协议兼容规则。** 消息统一使用信封格式，字段只增不改；遇到不认识的字段忽略，遇到不认识的消息类型记录日志后忽略。
10. **Agent 只从面板下载东西**，从不访问 GitHub 等外部站点。
11. 所有本地 API（sing-box 的 v2ray_api）只监听 127.0.0.1。

---

## 5. 技术栈与仓库结构

### 技术栈

- Rust stable，edition 2024，Cargo workspace。
- 面板：`axum`、`tokio`、`sqlx`（postgres + migrate）、`tower-http`、`rust-embed`（内嵌前端）、`argon2`、`serde`/`serde_json`、`tracing`、`uuid`、`rand`、`sha2`、`base64`、`x25519-dalek`、`ed25519-dalek`、`thiserror`/`anyhow`。
- Agent：`tokio`、`tokio-tungstenite`（rustls）、`reqwest`（rustls）、`rusqlite`（bundled）+ `rusqlite_migration`、`ed25519-dalek`、`sysinfo`、`tonic` + `prost`（统计 gRPC；构建时用 `protoc-bin-vendored` 或 `protox`，不依赖系统 protoc）、`clap`、`tracing`、`sha2`、`serde`/`toml`。
- 前端：React + Vite + TypeScript，不使用组件库，样式用普通 CSS。
- 数据库：PostgreSQL 16（开发时用 docker compose 启动）。

### 目录结构

```text
apsp/
├── Cargo.toml                 workspace
├── crates/
│   ├── protocol/              包名 apsp-protocol：面板↔Agent 消息定义
│   ├── compiler/              包名 apsp-compiler：模型 → sing-box 配置
│   ├── panel/                 包名 apsp-panel：面板后端，内嵌 web/dist
│   ├── agent/                 包名 apsp-agent：Agent 二进制入口
│   ├── agent-core/            包名 apsp-agent-core
│   ├── adapter-sdk/           包名 apsp-adapter-sdk
│   └── adapter-singbox/       包名 apsp-adapter-singbox
├── web/                       前端
├── plugins/sing-box/          systemd 与 OpenRC 运行时服务模板
├── deploy/
│   ├── install.sh.tmpl        安装脚本模板（由面板渲染）
│   ├── apsp-agent.service
│   ├── apsp-agent.openrc
│   └── docker-compose.yml     面板 + PostgreSQL
├── tools/build-singbox.sh     从上游源码构建 sing-box
├── scripts/e2e-real.sh        真实服务器上的手动端到端验证步骤
├── docs/
│   ├── adr/                   架构决策记录
│   ├── glossary.md            术语表（见第 14 节）
│   ├── protocol.md            面板↔Agent 协议规范
│   ├── PLAN.md
│   └── open-questions.md
├── PROGRESS.md
├── AGENTS.md                  分层规则与禁止事项（给人和 AI 看）
└── .github/workflows/ci.yml   fmt、clippy、test、core 禁用词检查
```

`agent-core` 内部按功能拆分：`identity`、`transport`、`reconcile`、`state`（含迁移文件）、`telemetry`、`usage`、`artifacts`、`system`（`Privileged` 和 `ServiceManager` trait 及其实现）。先用单文件实现，超过约 400 行再拆成目录。

---

## 6. 面板 ↔ Agent 协议

### 信封

```json
{ "v": 1, "type": "heartbeat", "id": "<uuid>", "ts": 1790000000, "payload": {} }
```

### WebSocket：`GET /api/agent/v1/ws`

连接建立后的认证流程：

1. 面板发送 `auth.challenge`：`{ nonce, server_time }`。
2. Agent 回复 `auth.response`：`{ server_id, signature }`，签名内容为 nonce，使用设备的 ed25519 私钥。
3. 面板用注册时保存的公钥验证，通过后发送 `hello.ack`：`{ server_time, session_token, session_expires_at }`。session_token 用于之后的 HTTP 请求，有效期 1 小时，每次重连重新签发。
4. Agent 发送 `hello`：`{ agent_version, protocol_version, capabilities: [], applied: { "<module>": <rev> } }`。

**Agent → 面板：**

| type | payload |
|---|---|
| `hello` | 见上 |
| `heartbeat` | `{ applied: {module: rev}, uptime_secs }`，每 20 秒一次 |
| `telemetry.static` | 系统、内核、架构、CPU 型号与核数、内存与磁盘总量、虚拟化类型、主机名、Agent 与 sing-box 版本 |
| `telemetry.metrics` | CPU%、内存已用、load 1/5/15、磁盘已用、网卡累计收发字节与速率、TCP/UDP 连接数、运行时间，每 10 秒一次 |
| `apply.result` | `{ module, rev, op_id, status: "applied" \| "failed", healthy: bool, error?: string }` |
| `usage.batch` | `{ epoch, seq, period_start, period_end, records: [{ stat_name, uplink, downlink }] }` |

**面板 → Agent：**

| type | payload |
|---|---|
| `auth.challenge`、`hello.ack` | 见上 |
| `manifest.changed` | `{ rev }` |
| `usage.ack` | `{ epoch, seq }` |

超过 60 秒没有收到任何消息，面板判定该服务器离线。Agent 断线后按指数退避重连，最长间隔 60 秒，并加 0–30% 的随机抖动。

### HTTP（请求头 `Authorization: Bearer <session_token>`）

- `GET /api/agent/v1/manifest` →
  ```json
  { "rev": 12, "modules": { "singbox": {
      "kernel_version": "1.14.2",
      "artifact": { "url": "...", "sha256": "..." },
      "config_rev": 7,
      "bundle_url": "...", "bundle_sha256": "...",
      "stats_listen": "127.0.0.1:18085" } } }
  ```
- `GET` bundle：返回 `{ "files": { "config.json": "<字符串>" } }`。
- 制品下载：`GET /api/agent/v1/artifacts/{name}/{version}/{arch}`。

### 注册（无需 session）

`POST /api/agent/v1/enroll`：`{ token, device_public_key, static_info }` → `{ server_id }`。token 为一次性，24 小时过期，在数据库事务中原子消费。

在 `docs/protocol.md` 中写出完整规范。在 `protocol` crate 中实现所有类型，并编写 serde 往返测试和"未知字段被忽略"的前向兼容测试。

---

## 7. 面板功能规格

### 配置（环境变量）

`APSP_DATABASE_URL`、`APSP_LISTEN`（默认 `0.0.0.0:8080`）、`APSP_PUBLIC_URL`、`APSP_DATA_DIR`（制品目录在其下的 `artifacts/`）、`APSP_ADMIN_PASSWORD`（仅首次启动时用于创建管理员）。

### 数据表（用 sqlx 迁移文件创建）

`admins`、`sessions`、`servers`（名称、状态、设备公钥、静态信息 JSON、最后在线时间）、`enrollment_tokens`、`nodes`（server_id、协议、端口、public_host、sni、reality 私钥与公钥、short_id）、`users`（名称、订阅 token）、`accesses`（user_id、node_id、uuid、stat_name，唯一约束 (user_id, node_id)）、`deployments`（server_id、module、rev、bundle JSON、bundle_sha256、created_at）、`server_module_status`（target_rev、applied_rev、healthy、last_error、updated_at）、`usage_records`（server_id、epoch、seq、stat_name、uplink、downlink、period_start、period_end，唯一约束 (server_id, epoch, seq, stat_name)）、`metrics_minutely`（保留 7 天）。

### 管理 API（Cookie 会话，HttpOnly + SameSite=Strict）

登录与登出；服务器的增删改查，以及生成安装命令；节点的增删改查；用户的增删改查；授权的增加与删除；部署状态与发布历史；每个用户、每个节点的流量汇总；制品列表。

### 关键行为

- **端口分配：** 每台服务器默认端口范围 20000–29999，取未被该服务器上其他节点占用的最小值。
- **Reality 密钥：** 用 `x25519-dalek` 生成，编码为 base64 URL-safe 无填充。必须编写测试，与 `sing-box generate reality-keypair` 的输出格式交叉验证（环境里没有 sing-box 时标记为 ignore）。short_id 为 8 位随机十六进制。
- **自动发布：** 节点、用户或授权发生变化后，等待 5 秒合并连续变更，然后重新编译受影响的服务器。bundle 的哈希有变化时才创建新 rev，并向在线的 Agent 发送 `manifest.changed`。
- **流量入库：** `usage.batch` 用 `ON CONFLICT DO NOTHING` 写入，无论是否重复都回复 `usage.ack`。
- **安装脚本：** `GET /install.sh?token=...` 用 `deploy/install.sh.tmpl` 渲染，填入面板地址和 token。
- **订阅：** `GET /sub/{token}?format=links|singbox`，默认 links。
  - links：把各节点的 `vless://{uuid}@{public_host}:{port}?encryption=none&flow=xtls-rprx-vision&security=reality&sni={sni}&fp=chrome&pbk={public_key}&sid={short_id}&type=tcp#{节点名}` 按行拼接，再整体 base64 编码。
  - singbox：一份最小可用的客户端配置，包含每个节点的 vless 出站（reality + utls chrome）、一个 selector、一个监听 127.0.0.1:2080 的 mixed 入站，路由默认走 selector。
  - 订阅只包含该用户自己的凭证，并且只包含已成功应用的节点。

### 前端页面（中文界面）

登录；服务器列表（在线状态、版本、CPU、内存）；服务器详情（静态信息、最新指标、部署状态：目标版本、已应用版本、是否健康、最近错误；新建服务器时显示安装命令）；节点（创建、列表）；用户（创建、授权节点、复制订阅链接、流量汇总）。构建产物由面板通过 `rust-embed` 内嵌提供。

---

## 8. Agent 功能规格

### 命令行

- `apsp-agent enroll --panel <URL> --token <T>`：生成设备密钥，完成注册。
- `apsp-agent run`：常驻运行。
- `apsp-agent status`：通过 root 可访问的 unix socket 查询运行中的 Agent，显示面板连接状态、各模块已应用版本、健康状态、待确认的流量批次数量。

### 文件位置

- 配置：`/etc/apsp/agent.toml`（`panel_url`）。
- 身份：`/etc/apsp/identity/device.key`（权限 0600）、`/etc/apsp/identity/server_id`。
- 状态库：`/var/lib/apsp/core/state.db`，从第一天起使用迁移机制。表：`kv`、`intents`、`usage_baselines`、`usage_outbox`。
- 所有路径都可以通过配置覆盖，以便测试时使用临时目录。

### 对账流程（sing-box 模块）

触发条件：收到 `manifest.changed`，或每 60 秒轮询一次。步骤如下：

1. 拉取 manifest。
2. 确认内核版本已安装。未安装则下载、校验 sha256、解压到 `/opt/apsp/plugins/sing-box/<版本>/`，执行 `sing-box version`，解析输出中的 `Tags:` 行，**必须包含 `with_v2ray_api`，否则拒绝并回报错误**。
3. `config_rev` 与已应用版本不同时，拉取 bundle，校验哈希，写入 `/var/lib/apsp/plugins/sing-box@main/revisions/<rev>/config.json`。
4. 用目标版本的二进制执行 `sing-box check -c <文件>`。
5. 计划：配置哈希相同为 noop；配置变化为 reload；内核版本变化为 restart。
6. 写入意图记录。如果是 reload 或 restart，先读取一次流量计数作为当前周期的终值。
7. 原子切换 `current` 符号链接（先建临时链接，再 rename）。
8. 通过 `ServiceManager` 请求重载或重启，自动选择 systemctl 或 rc-service；OpenRC 重载向实际运行时子进程发送 HUP。
9. 健康检查：单元状态为 active、端口在监听、统计 API 可访问。
10. 成功：标记意图完成，记录已应用版本，发送 `apply.result`。失败：切回上一个版本并重载，发送带错误信息的 `apply.result`。

同一模块的操作串行执行；遵守第 4 节的版本合并规则；启动时先处理未完成的意图。

### 适配器接口（adapter-sdk）

基础 trait `Adapter`：`describe`、`prepare`、`plan`、`apply`、`health`。可选 trait `UsageSource`：`read_counters`。每次调用都必须有超时。`agent-core` 提供 `FakeAdapter`、`FakeServiceManager`，用于测试。

### 流量账本（agent-core/usage）

1. 每 30 秒调用一次 `UsageSource::read_counters`，拿到每个 stat_name 的累计上传和下载字节数。
2. 与同一 epoch 的基准值相减得到差值。在同一个 SQLite 事务里更新基准值，并写入一条带递增 seq 的待上报记录。
3. 发送 `usage.batch`，收到 `usage.ack` 后标记为已确认，定期清理已确认的记录。
4. epoch 是一个 uuid。Agent 自己触发 reload 或 restart 时，先读终值，再开启新 epoch。发现计数变小、而 Agent 没有触发过重载时，同样开启新 epoch，并在日志中记录可能存在的缺失窗口。

### 遥测

静态信息在每次 `hello` 之后发送一次。指标每 10 秒采集并发送一次，采集失败的值不发送，不能填 0。

---

## 9. sing-box 技术要点（已核实）

1. **官方 release 不包含 `with_v2ray_api`。** 配置里出现 `experimental.v2ray_api` 时，官方构建会在启动时直接报致命错误退出。所以必须从上游源码自行构建，并由 Agent 检查构建标签。
2. **构建方式：** 检出上游确切的 tag，参考上游 Makefile 中的 release 构建方式，使用上游的默认构建标签（`release/DEFAULT_BUILD_TAGS`）和链接参数（`release/LDFLAGS`），**只追加** `with_v2ray_api`，不改任何源码。`tools/build-singbox.sh` 实现这个过程，并按面板制品目录的结构输出文件和 `SHA256SUMS`。执行前先阅读上游对应 tag 的 Makefile 和 release 目录，确认文件格式。
3. **SIGHUP 重载**会先检查配置，然后关闭当前实例、再创建新实例。已有连接会断开，统计计数会清零。因此重载前必须先读一次计数。
4. **统计接口：** v2ray_api 的 StatsService 使用 gRPC。调用 `QueryStats` 时使用 `patterns` 字段（复数，单数的 `pattern` 已弃用），`reset` 设为 false。用户计数的名称格式是 `user>>>{name}>>>traffic>>>uplink` 和 `user>>>{name}>>>traffic>>>downlink`。**这个名称里不包含入站标签**，所以统计用户名必须按"成员 + 节点"唯一。proto 文件从上游 1.14.2 tag 复制到 `crates/adapter-singbox/proto/1.14/`。
5. v2ray_api 没有认证，只能监听 127.0.0.1。
6. 目标版本：**sing-box 1.14.2**。配置字段以 1.14 官方文档为准。本文中的字段与官方文档不一致时，以官方文档为准，并记录到 `docs/open-questions.md`。

### 服务端配置的目标形状（编译器输出，示意）

```json
{
  "log": { "level": "warn", "timestamp": true },
  "inbounds": [{
    "type": "vless", "tag": "node-3", "listen": "::", "listen_port": 20000,
    "users": [{ "name": "u1_n3", "uuid": "<uuid>", "flow": "xtls-rprx-vision" }],
    "tls": { "enabled": true, "server_name": "<sni>",
      "reality": { "enabled": true,
        "handshake": { "server": "<sni>", "server_port": 443 },
        "private_key": "<private_key>", "short_id": ["<short_id>"] } }
  }],
  "outbounds": [{ "type": "direct", "tag": "direct" }],
  "route": { "final": "direct" },
  "experimental": { "v2ray_api": { "listen": "127.0.0.1:18085",
    "stats": { "enabled": true, "users": ["u1_n3"] } } }
}
```

编译器输出必须是确定性的（相同输入产生完全相同的字节），以便做黄金文件测试。没有任何授权用户的节点，暂时不输出其入站。

---

## 10. 服务器上的路径与服务

```text
/opt/apsp/core/<版本>/apsp-agent        current → <版本>
/usr/local/bin/apsp-agent → /opt/apsp/core/current/apsp-agent
/opt/apsp/plugins/sing-box/<版本>/sing-box   current → <版本>
/etc/apsp/agent.toml
/etc/apsp/identity/
/var/lib/apsp/core/state.db
/var/lib/apsp/plugins/sing-box@main/{revisions/, current, data/}
```

`deploy/apsp-agent.service`：

```ini
[Unit]
Description=Apsp Agent
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=/opt/apsp/core/current/apsp-agent run
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

`plugins/sing-box/apsp-singbox@.service`：

```ini
[Unit]
Description=Apsp sing-box runtime (%i)
After=network-online.target
Wants=network-online.target

[Service]
User=apsp-singbox
Group=apsp-singbox
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
NoNewPrivileges=true
ExecStart=/opt/apsp/plugins/sing-box/current/sing-box run -c /var/lib/apsp/plugins/sing-box@%i/current/config.json -D /var/lib/apsp/plugins/sing-box@%i/data
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

OpenRC 使用 `deploy/apsp-agent.openrc`、`plugins/sing-box/apsp-singbox.openrc`，安装为 `/etc/init.d/apsp-agent` 和 `/etc/init.d/apsp-singbox@main`。两个服务独立监督，runtime 使用专用用户、ambient capability 与 no_new_privs；日志保存在 `/var/log/apsp/`。运行时配置路径与 systemd 相同。

**安装脚本**（`deploy/install.sh.tmpl`）：检查 root、Linux 和运行中的 systemd/OpenRC；识别架构（x86_64 对应 amd64，aarch64 对应 arm64）；支持 shadow 或 BusyBox 创建系统用户与组 `apsp-singbox`（nologin）；创建目录并设置权限；从面板下载 Agent 二进制并校验 sha256；先执行暂存二进制 `enroll`，成功后安装到版本目录、更新 current 链接和对应服务文件；启用两个服务，重启 Agent。OpenRC 使用 default runlevel，首次发布前不启动尚无配置的运行时。脚本必须可以重复执行，重复执行就相当于升级 Agent，保留已有身份与账本。

Agent 发布构建使用 musl 静态链接：`x86_64-unknown-linux-musl` 和 `aarch64-unknown-linux-musl`。

---

## 11. 测试要求

- **compiler：** 黄金文件测试放在 `crates/compiler/tests/golden/`，设置 `UPDATE_GOLDEN=1` 时重新生成。
- **protocol：** serde 往返测试；未知字段被忽略的前向兼容测试。
- **usage：** 差值计算；epoch 切换；计数变小；重复的 ack；Agent 重启后从待上报队列继续发送。
- **reconcile：** 使用 FakeAdapter 和 FakeServiceManager，覆盖版本合并、应用失败后回滚、意图记录的崩溃恢复（模拟中途退出）。
- **panel：** 使用测试用 PostgreSQL（`sqlx::test`）。环境里没有 Postgres 时标记为 ignore 并写明原因。
- **进程内端到端测试：** 启动面板（测试数据库）和 Agent（临时目录 + FakeAdapter），跑通：注册 → 认证 → hello → 创建节点和用户 → 发布 → apply.result → usage.batch → ack → 通过管理 API 查到流量。
- **`scripts/e2e-real.sh`：** 在真实 Debian 12 虚拟机上做第 2 节验收的手动步骤说明和辅助命令。
- **CI：** fmt、clippy（`-D warnings`）、test，外加一条检查：`agent-core` 目录中不得出现 `singbox` 或 `sing-box`。

---

## 12. 执行顺序与完成标准

| 阶段 | 内容 | 完成标准 |
|---|---|---|
| G1 | workspace 骨架、全部 crate 的空结构、AGENTS.md、docs/glossary.md、docs/adr（把第 4 节的每条决策各写成一条 ADR）、CI 配置 | `cargo build` 通过；CI 文件存在 |
| G2 | protocol crate、docs/protocol.md | 往返测试和前向兼容测试通过 |
| G3 | compiler crate | 黄金文件测试通过，输出是确定性的 |
| G4 | 面板基础：数据库迁移、管理员认证、服务器与注册 token、注册接口、WebSocket 与挑战认证、manifest 和 bundle 接口、制品下载、安装脚本渲染 | 集成测试通过 |
| G5 | agent-core：配置、身份与注册、传输与重连、遥测、状态库与迁移、对账（FakeAdapter）、流量账本、status 命令 | 第 11 节的 usage 和 reconcile 测试通过 |
| G6 | adapter-sdk、adapter-singbox：版本与构建标签解析、check、apply、health、StatsService 客户端 | 单元测试通过；有 sing-box 二进制时，真实 check 测试通过 |
| G7 | 面板业务：节点、用户、授权、自动编译与发布、流量入库、订阅 | 进程内端到端测试通过 |
| G8 | 前端全部页面 | `web` 构建成功，面板能提供页面 |
| G9 | 部署文件、tools/build-singbox.sh、scripts/e2e-real.sh、README 快速上手、PROGRESS.md 最终报告 | 按 README 能在本地启动面板 |

每个阶段都要满足第 0 节第 4 条的检查要求，才能进入下一个阶段。

---

## 13. 最终交付物

1. 可编译、测试全部通过的仓库。
2. `README.md`：项目简介；在本地启动面板；构建 sing-box；构建 Agent；在一台服务器上完成第 2 节验收的步骤。
3. `PROGRESS.md` 最终报告：哪些已经可用、如何验证的；哪些还没有完成、原因是什么；需要人工执行的步骤；建议的下一步。
4. `docs/open-questions.md`：运行过程中所有自行做出的决定。

---

## 14. 术语表（代码与文档统一使用）

| 中文 | 代码中 | 含义 |
|---|---|---|
| 模型 | model | 管理员在面板上编辑的对象 |
| 编译 | compile | 把模型变成每台服务器的配置包 |
| 配置包 | bundle | 某个运行时、某个版本的完整配置文件集合 |
| 主机清单 | manifest | 一台服务器上所有模块的期望状态 |
| 对账 | reconcile | Agent 比对期望状态与实际状态 |
| 计划 | plan | 适配器判断本次变更需要的动作：noop、reload、restart |
| 应用 | apply | 适配器把配置包落到运行时上 |
| 意图 | intent | 应用前写下的记录，用于崩溃后恢复 |
| 周期 | epoch | 一段连续的计数周期，sing-box 重载或重启后开启新周期 |
| 适配器 | adapter | 把 core 的统一操作翻译成外部程序原生接口的一层 |
| 外插 | plugin | 原样安装、不做修改的外部程序，例如 sing-box |
