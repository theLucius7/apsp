# 仓库协作规则

本文只写现在生效的规则，每条规则在这里只有一个出处。产品范围与界面位置以[功能地图](docs/product-map.md)为准，对象与用语以[对象词典](docs/redesign/03-objects.md)为准，代码与数据的归属以[功能划分与逻辑结构](docs/architecture.md)与[领域归属表](docs/domain-map.md)为准，具体决策见[架构决策索引](docs/adr/README.md)。2026-11-08 整理前的原文、各项用户授权的来历与每条旧条款的去向见[协作规则整理记录](docs/history/agents-rules-20261108.md)。

遇到歧义，采用最简单且符合既定架构的方案，将问题、选择和理由记录到 `docs/open-questions.md` 后继续执行。

## 提交身份（2026-11-09 用户更正）

提交身份按会话为谁工作区分，不写进仓库：

- `origin` 是 `theLucius7/apsp` 的会话为仓库所有者工作，提交作者与提交者都用 `Lucius7 Nya <i@lucius7.dev>`。
- 在 fork（如 `imengying/apsp`）里为其他贡献者工作的会话用该贡献者自己的身份，不改成所有者身份。合入、变基或 cherry-pick 别人的提交时保留原作者，提交者是执行合入的一方。
- 身份只放本地：仓库级 `git config user.name`、`user.email`，以及被 `.gitignore` 忽略的 `.claude/hooks/git-identity.sh`、`.claude/settings.local.json`。不要把身份脚本或钩子注册提交进仓库，合入外部 PR 时也不带入。2026-11-07 随 #245 合入的身份钩子把 fork 所有者的身份带进了本仓库，此后所有者会话的提交都署了 `imengying`，2026-11-09 已移除。
- 云端会话的新克隆没有本地配置，第一次提交前按上面写入仓库级配置，并设 `commit.gpgsign false`（环境签名密钥属于代理，不属于本人）。提交前用 `git var GIT_AUTHOR_IDENT` 与 `git var GIT_COMMITTER_IDENT` 确认，不使用代理默认身份。
- 提交信息不加 `Co-Authored-By` 行。

## 交付方式（2026-11-08 用户确认）

- **方案与高风险改动走 PR。** 高风险指修改数据库基线（`crates/panel/migrations/0001_current.sql`、`crates/agent-core/src/state/schema.sql`）、面板与 Agent 协议、部署方式或生产数据。**其余改动直接提交 main**，包括只改文档与小修复。
- main 合并或推送后一分钟内自动部署（见“生产边界”）；只改文档的提交只记录版本，不重建、不重启面板。直接提交 main 前确认改动不属于高风险类别。
- 每完成一项工作，在 `PROGRESS.md` 记录完成项、验证结果、已知问题与下一步，并以一个 Conventional Commits 提交交付。
- 保留现有未提交工作，不覆盖不属于当前任务的修改。

## 生产边界

按 ADR 0122 统一当前结构，不再承诺旧 APSP 开发版本兼容或自动数据库升级。面板与 Agent 仅初始化空库、核对当前基线；旧库明确拒绝启动并由使用者手动重建，禁止自动删除数据。旧协议回退和历史迁移修补已清理；页面地址按 [ADR 0126](docs/adr/0126-workspaces-language-and-addresses.md) 的地址策略管理。当前业务、外部标准及生产数据边界保持。

生产面板随 main 自动部署：`apsp-deploy.timer` 每分钟检查 main，合并或推送后一分钟内开始部署（2026-11-07 核实仍在运行；#249 按“当前没有自动部署”的说法合并，当晚生产停机）。生产库只认已部署的基线。修改 `crates/panel/migrations/0001_current.sql` 的提交必须附带 `scripts/baseline-adoption-*.sql`，把当前部署的基线接入新基线，写法见 `docs/database-baseline.md`「生产安装」。部署器换镜像前会在生产库结构副本上试启动新镜像，需要时先在副本上执行这些脚本，成功后才在生产执行并换镜像；缺少或不通过时部署被拦下，生产保持原版本。修改 `crates/agent-core/src/state/schema.sql` 后，发布新 Agent 前同样要先处理现有主机的状态库（当前已安装的 0.3.7 状态库由新 Agent 首次打开时记为基线）。见 `docs/database-baseline.md`「生产安装」。

外部操作与签收：

- 真实云资源操作（开通、计费、带宽、启停、更换 EIP）、真实 DNS 写入、真实通知发送都不属于开发验证，开发测试只用接口替身；生产迁移、正式发布与实机验收单独授权；移除功能或数据需用户单独授权，但有替代实现的旧模型、旧界面、旧接口与专用测试按 [ADR 0131](docs/adr/0131-one-model-per-object.md) 整套删除，旧数据转换后随基线变更删除（先导出存档，基线变更照常走 PR 并附接入脚本）。
- 实机能力按 P0 保护服务器 → P0 IP 查询 → P1 sing-box 业务归位 → P1 共用诊断框架 → P2 TCP 接入的顺序签收。源码审查、源码合入、实机能力签收和正式发布/部署分别记录；前置阶段未通过时，后续实现可经独立审查及相称验证后合入作为准备，但不能签收、正式发布或部署新增诊断能力，不以其 CI 结果宣称前置阶段完成。逐项记录故障场景、证据对应的源码和未验证范围；NodeQuality 历史报告可读与完整执行能力须分别验收。执行条件和当前缺口见[整改顺序与验收状态](docs/acceptance/ordered-remediation.md)。
- NodeQuality 完整验机启动门禁、工具许可、签名制品及真实外部操作授权保持；缺工具、无观测和条件验收未运行必须分别记录，不能凭入口或台账声称执行能力完成。资源场景在专用测试机验证，不在生产机器上反复运行完整验机。

## 临时 CI 暂停（2026-10-01 用户要求）

当前 GitHub Actions 工作流已暂停。所有进行中的任务完成前，只做相称的本地验证，不触发、重跑或重新启用 CI；不要因单个 PR 或单个任务完成而恢复。全部任务完成后再统一恢复并验证最终整合提交。暂停期间未执行或取消的 CI 必须记为未验证，不据此宣称 main 全绿。恢复时移除此节。

发布 Agent 需要 CI 时，须由用户临时批准启用 `CI` 与 `Signed release draft`，只用于该版本的 PR 与 main 验证、草稿构建和发布校验，发布完成或中止后由同一会话重新停用。`agent-v0.3.1` 至 `agent-v0.3.8` 的历次例外均已结束（2026-11-08 核对：标签都已发布，四个工作流均为手动停用），记录见整理记录。

## 分层

详细架构约束见 `docs/adr/0001-declarative-snapshots.md` 至 `docs/adr/0011-loopback-local-api.md`。

- `protocol` 定义面板与 Agent 的公共消息；`compiler` 将模型确定性地编译成完整配置包（流量服务的配置编译库，客户端配置部分供服务权益使用）。
- `agent-core` 的工作区依赖只能是 `adapter-sdk`、`protocol`；原生配置、身份、传输、对账、状态、遥测、流量和制品管理放在 core。
- `adapter-*` 的工作区依赖只能是 `adapter-sdk`。适配器只负责无状态翻译，不自行持久化，不连接面板。
- 只有 `agent` 二进制入口同时依赖 core 和具体适配器，并将适配器注册到 core。
- `agent-core` 目录内任何文件不得出现 `singbox` 或 `sing-box` 字样；使用模块标识、能力和统一接口。
- core 管服务器；代理用户、授权、订阅、用户流量、配额和周期属于服务权益领域（`crates/access`），详见 [ADR 0023](docs/adr/0023-proxy-business-boundary.md) 与 [搬迁及启用兼容 ADR 0030](docs/adr/0030-singbox-plugin-business.md)。core 不得引用代理业务的 `user`、`subscription`、`quota`，含复数、蛇形和驼峰形式；CI 使用 `tools/check-core-boundary.py` 检查。系统账户与 SQLite 原生 API 仅允许检查器列出的具体表达式，不允许文件或整行豁免。
- 面板是按领域组织的一体化系统，分平台、领域和应用三层，不再有插件（[ADR 0128](docs/adr/0128-domain-architecture.md)；九个领域、依赖顺序与写入规则见[功能划分与逻辑结构](docs/architecture.md)，逐项功能的归属见[领域归属表](docs/domain-map.md)）：
  - 平台（`crates/panel-host`、`crates/cloud-api`）只放技术机制，工作区依赖只能是 `protocol`，不依赖任何领域；目录内禁止 sing-box 名称和上述代理业务词汇，例外表达式由检查器逐条列出。
  - 领域 crate（`crates/governance`、`crates/resources`、`crates/networking`、`crates/naming`、`crates/compiler` 与 `crates/traffic`（流量服务）、`crates/access`（服务权益）、`crates/costs`、`crates/observability`、`crates/operations`）只依赖平台、`protocol`、依赖顺序中排在前面的领域 crate 和同一领域的其他 crate。领域代码不得移回平台或应用 crate。仓库没有 `plugins/` 目录：运行时服务定义在 `deploy/`，诊断工具的打包脚本、策略与锁定文件在 `crates/observability/upstream/`（签名制品里仍按原名 `plugins/...` 记录，这是制品格式）。诊断代码（IPQuality、NodeQuality、TcpQuality）在观测 crate 的 `diagnostic_tools/`。
  - 只有应用 `apsp-panel` 组合平台与各领域（组合在 `crates/panel/src/domains.rs`：路由与后台任务各一张清单，每个领域 crate 只导出一个 `routes()`，路由的完整路径写在声明处）、接线依赖倒置接口（清单见 [architecture.md §5.5](docs/architecture.md#55-类型化接口)，接线在 `crates/panel/src/wiring.rs`），并保存唯一的数据库基线。不再新增插件式的全局钩子：需要逆着依赖顺序调用时，由调用方声明类型化接口。
  - 控制台接口地址按对象所属领域命名（`/api/traffic/…`、`/api/access/…`、`/api/naming/…`、`/api/costs/…`、`/api/observability/…`、`/api/resources/alicloud/…`；其余地址不变），路由的权限结论记在 `crates/panel/tests/fixtures/route-policy.tsv`，新增或改动路由后运行 `python3 tools/route-policy.py`（见 [ADR 0132](docs/adr/0132-platform-only-host-and-domain-addresses.md)）。
  - `tools/check-core-boundary.py` 检查每个 crate 的层级与依赖方向；新增 crate 先在检查器里登记层级。
- 每张表只有一个所有者（`tools/feature-boundaries.toml`）。提交前运行 `python3 tools/check-feature-boundaries.py`：现有越界记在 `tools/feature-boundaries.baseline`，只许减少；新增的跨领域 SQL（写其他领域的表、读其他领域的私有表或平台的表、逆着依赖顺序读公开只读表）、逆着依赖顺序的代码引用（平台引用领域，或前面的领域引用后面的领域、同组的领域，同一 crate 内与跨 crate 都算）、跨 crate `#[path]` 或前端跨领域引用会失败，应改为经所有者的接口，而不是加进清单。新增表必须在所有者清单里登记。
- 管理界面和使用文档按[功能地图](docs/product-map.md)的六个工作区组织（[ADR 0126](docs/adr/0126-workspaces-language-and-addresses.md)；侧栏已是六个工作区，尚未落地的几处见功能地图）：新增页面先确定所属领域与工作区，一台服务器的事放服务器详情。前端按领域目录组织，每个页面在所属领域的 `pages.tsx` 登记（地址、名称、工作区、权限），每个入口属于一个领域，侧栏分组由清单声明；同一对象允许多个入口，但只有一套模型和编辑逻辑。页面地址按用途命名、与入口一致；已发给代理用户或客户端的地址永不失效，控制台地址改名后旧地址改写到新地址并至少保留一个发布版本。降级功能在界面标“实验”，冻结功能默认隐藏、由面板设置显示，都放在所属工作区内；核心旅程不得依赖降级页面。
- 产品定位为自托管的服务器管理与配置面板，以节点、链路、用户与订阅为主线（[ADR 0087](docs/adr/0087-narrow-positioning-proxy-network.md)）。新功能必须服务节点、链路、用户与订阅，或运营这张网络所需的服务器、线路与费用感知；否则进入路线图候选池。按功能地图的分级处理现有功能：降级功能只修缺陷和安全问题，冻结功能只修安全问题，移除功能或数据需用户单独授权。
- 系统管理员与代理用户分别命名；服务器网卡总流量留在 core。计量 `epoch` 只标记计数器重置，不得用作套餐周期。业务搬迁保留用户 ID、令牌、旧订阅路径、节点凭据、授权和历史流量，数据库表先不改名。
- 诊断任务生命周期、资源预算、持久化、取消及历史由观测领域的共用服务管理；各诊断实现只转换参数、执行和解析报告，见 [ADR 0028](docs/adr/0028-shared-diagnostic-job-service.md)。后续诊断能力的接入代码可以经独立审查和相称验证后合入准备；NodeQuality 迁移及前置阶段的实机验收通过后，才能签收、正式发布或部署后续新增诊断能力。
- 特权操作必须经过 `Privileged` trait，服务管理经过 `ServiceManager` trait；外部运行时是独立的系统服务（Linux systemd/OpenRC、macOS launchd、FreeBSD rc.d、Windows 计划任务）。
- core 按 `identity`、`transport`、`reconcile`、`state`、`telemetry`、`usage`、`artifacts`、`system` 拆分，先用单文件，超过约 400 行再按需拆目录。

## 实现与检查

- 一类对象一套模型（[ADR 0131](docs/adr/0131-one-model-per-object.md)）：新功能只加在当前模型上；替代实现完成后，旧实现连同接口、页面、分支、测试、脚本与文档一起删除，调用方一起改，不保留旧函数名、旧模块路径或旧 crate 名的包装与再导出。
- 一种底层机制一套实现（[ADR 0133](docs/adr/0133-one-implementation-per-mechanism.md)）：后台任务一律经 `apsp_panel_host::maintenance::periodic` 运行并报心跳，心跳名登记在 `crates/panel/src/domains.rs` 的 `HEARTBEATS`；咨询锁一律用 `apsp_panel_host::locks`，锁键由所有者在自己的 `src/locks.rs` 声明。两者的守卫随 `cargo test` 运行，例外写明原因，只许减少。
- Rust stable、edition 2024；每个 crate 根文件（含二进制根、测试 crate 和构建脚本）使用 `#![forbid(unsafe_code)]`，禁止引入 `unsafe` 代码。
- 代码标识符、代码注释使用英文；`docs/`、README、PROGRESS 使用中文。界面以中文为源语言，英文是翻译（[ADR 0127](docs/adr/0127-chinese-source-language.md)）。
- 只使用任务说明列出的依赖；确需新增依赖时先在 `docs/adr/` 说明必要性和替代方案。
- sing-box 只通过 `tools/singbox-runtime/` 的补丁修改（2026-11-01 用户要求维护自己的构建版本，见 ADR 0104），不直接改上游 checkout；不得提交真实密钥、令牌、域名、IP 或其他环境凭证。示例使用占位符、保留示例域名以及明确要求的回环或监听地址。
- 不在核心路径留下仅有 TODO 的实现，不为范围外功能创建空目录或空模块。
- 按改动范围做相称的本地验证：代码改动提交前通过 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`、`python3 tools/check-core-boundary.py`、`python3 tools/check-feature-boundaries.py` 与 `python3 tools/route-policy.py`，改前端时另跑前端构建与受影响的测试；只改文档时检查文档内链接与引用。
- 测试必须覆盖协议兼容、编译确定性、账本事务与恢复、对账合并与回滚。只需要 PostgreSQL 的测试不标忽略；需要真实运行时、root 权限或测试机而环境不可用的测试使用带原因的 `#[ignore]`，在 `PROGRESS.md` 记录未验证范围与人工步骤，不得宣称已通过实机验收。
- 生成的 `web/dist` 不提交到 Git，面板编译前先构建前端。正式签名 Release 按原发布流程管理，本地构建不会自动上传 GitHub。

### 执行节奏（2026-10-01 用户要求）

用户要求“中途不要再进行测试，修改完了一个大步骤提交”。每个完整大步骤先集中完成相关实现、界面、协议、测试代码及文档修改；修改期间不运行测试、构建或穿插验证命令。整步修改完成并冻结输入后，再统一执行该步骤需要的验收、记录结果并提交；不得拆成反复测试和提交的小循环。最终验收发现的问题修复后，仅补验失败或受修复影响的范围。已有证据保留，未执行的测试明确记为待验，不替代原验收要求；临时CI暂停和完整验机许可门禁继续有效。本节覆盖旧文档中的中间验证节奏。

### 测试材料清理（2026-10-03 用户最新要求）

用户要求“验证完成就删除，之前的版本都不要了，保留最新的即可”。这覆盖旧记录中保留所有 target、缓存、失败快照和虚拟机磁盘的要求。每次集中验证完成后，先保存必要的结果摘要与对应源码身份，再停止并清理本轮所属测试进程、容器、临时数据库、挂载和虚拟机，删除编译目录、缓存副本、旧源码归档及过期测试制品；失败仅保留有界错误摘要，不积累整套失败环境。制品每种用途/架构只留最新一套，不保留多轮重复副本。

保留当前仓库源码、测试用例、已提交的验收记录、最新必要制品及有效凭据；不能删除生产服务数据、其他项目缓存或未提交工作。删除前核对精确路径/容器所有权与活动引用，不使用全局 Docker prune 或扩大目录范围。最新制品也须保留其签名与清单；清理不能将尚未执行的验收记为通过。已清理的历史原始材料在新的清理记录中注明，原记录只证明当时结果，不再承诺材料持续存在。

## 范围

功能是否在范围内，以功能地图的功能分级与「不做什么」为准（原 MVP 排除清单中仍然有效的各项已并入「不做什么」）。下面是各项已授权功能仍然有效的约束，授权的来历见各 ADR 与整理记录。

- **协议与证书**（ADR 0034）：Hysteria2、Shadowsocks 2022、TUIC v5、AnyTLS、Naive、Snell v6，以及 TLS 证书的自动申请和续期。
- **Agent 与平台**（ADR 0017、0021、0022）：OpenRC 服务；Agent 高频监控、任务、自动更新及非 Linux 常驻部署；特权 helper 仅保留 trait 边界，保留重复安装升级；制品签名与编译时发布信任根按 ADR 0017 执行。
- **Agent 下载与公开看板**（ADR 0038）：Agent 二进制必须从 GitHub 下载，不能由面板提供；安装和自更新保留独立验签，镜像请求不携带设备凭据；运行时与配置仍经面板；公开看板使用只读白名单及统一隐藏校验，流量矫正不改代理业务账本。
- **服务器成本与网卡配额**（ADR 0023、0036）：服务器成本、续费到期、按账单日计算的网卡配额与可配置的轻量周期拨测按 ADR 0023 分层。
- **代理业务**（ADR 0023、0035、0040、0043–0045、0076、0112）：代理用户配额、重置周期和到期属于服务权益。权限组与套餐分别分配，套餐使用不可变快照和原计量账本，不扩大为平台全局用户；不能把清空计量 epoch 当作重置套餐，也不能把订阅过滤当作运行时停用；保持旧用户/凭据兼容，不混跑新旧 publisher。链路保持单运行时、独立公开入口、线性无环路径、入口单次计量及内部秘密不进入用户订阅，不扩展自动出口池。外部节点沿用提供方凭据与独立身份版本，不伪造 Agent 流量或配额执行能力；来源导入先预览再采用，新增节点不自动授权；用户只经策略组获得资源。
- **订阅格式与专属地址**（ADR 0101、0103）：Clash Meta（mihomo）YAML 由与 sing-box 订阅相同的客户端出站转换，不能等价表达的节点不写入并在文件中说明；同一订阅地址按 User-Agent 识别 Clash 客户端，显式格式参数优先；不调用第三方转换服务。每个用户独立、可重置的随机主机名与路径默认关闭，旧 `/sub/{token}` 由管理员决定何时关闭；真实令牌只进入面板的加密凭据，不得写入仓库。
- **Passkey 与代理用户入口**（ADR 0059、0107）：通用 WebAuthn 服务与管理员包装留在面板，代理用户入口、开通链接及独立会话属于服务权益；不增加平台全局用户或公开注册；保留密码/TOTP 和旧订阅，开通/恢复不得使用订阅令牌。
- **管理员**（ADR 0082、0107）：具名管理员分所有者、运维与只读，各自使用自己的 Passkey。
- **监控与通知**（ADR 0039、0047）：复用现有 Agent 拨测协议；通知仅提醒，不执行付款、停用或远程命令；实时样本与持久化确认分开，历史聚合不改变网卡或代理业务账本；汇率使用可追溯来源、不编造缺失币种，匿名看板不公开成本；Telegram 与 Webhook 分渠道投递，测试不发送真实通知。
- **DDNS**（ADR 0048、0056、0060、0110）：属于名称与证书领域 `crates/naming`，规则自身的启用状态是唯一开关；复用 Agent 已上报的 IP，面板持有 API Token 并执行有限 DNS 对账，凭据不发给 Agent 或公开界面；无有效地址或设备离线时保留解析；一次表单以事务创建 A/AAAA 两条独立规则。
- **阿里云**（ADR 0057、0058、0093、0116、0121）：凭据留在面板；更换 EIP 并释放旧地址需要管理员确认；CDT 不增加每日汇总通知，CDT 总量已包含 IPv6，不另行实现 IPv6 流量采集。
- **平台扩展**（ADR 0077–0082）：各领域复用已有的领域能力、共用诊断生命周期、特权与服务管理边界；不能将节点代理用户移到 core。
