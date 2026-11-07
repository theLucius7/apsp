# 工程整理子方案

- 状态：提议（2026-11-07）。
- 定位：采纳上一版方案中的文档、工具与局部代码整理建议，作为重新设计的第三部分。它在[功能归属](02-inventory.md)、[对象词典](03-objects.md)与[收敛方案](07-convergence.md)之后进行：先定职责边界，再决定哪些文件值得移动（R18）。**全仓 Rust 路径搬迁暂缓**，不是完成重构的前置条件。
- 依据：[设计依据](01-principles.md) R2、R6、R14、R16、R18。
- 与 [ADR 0125](../adr/0125-feature-modules.md) 的关系：代码与数据的目标结构、依赖规则与迁移阶段以它和 [architecture.md](../architecture.md) 为准。本篇补充那里没有展开的部分：上一版方案的源码纠正、路径迁移时要核对的使用者、文档有效性、协作规则整理、工具分类与测试入口；与 ADR 0125 表述不同的地方在 §7.1、§9、§10 说明。

## 1. 分类的维度

上一版方案按“谁使用”把仓库分为产品代码、插件、工程工具、文档四类。这适合作为仓库导览的四个阅读入口，但它们不是同一个维度：插件本身也是产品代码；运维脚本可能由部署者日常使用；开发者同时用代码、工具和文档。方案前面把工程工具定义为“只有维护者在开发、构建、发布、验收时运行”，后面又把用户安装面板用的 `scripts/panel.py` 移进 `tools/panel/`，口径并不一致。

本方案按问题选择维度，每个问题只用一个维度：

| 要解决的问题 | 分类依据 | 结果所在 |
| --- | --- | --- |
| 用户怎样找到功能 | 任务领域、操作对象 | [六工作区方案](04-workspaces.md) |
| 开发者怎样找到实现 | 业务归属、依赖方向 | §7、§8，模块说明 |
| 工具应该放哪里 | 使用者、执行环境、生命周期 | §5 |
| 文档应该放哪里 | 读者、用途、有效状态 | §3 |
| 功能是否显示、是否允许执行 | 权限、能力、成熟度、实际状态 | §8 能力清单 |

## 2. 对上一版方案的源码纠正

以下四条按 main `a32ec49b` 重新核对（上一版方案标注的 `cb62ac4` 是它的直接祖先，相关文件在两者之间没有实质变化）。

### 2.1 面板组装层的命令入口不是放错的业务代码

上一版方案认为 `crates/panel` 中的 `legacy_takeover.rs`、`source_migration.rs`、`publisher.rs` 说明业务或迁移代码留在了组装层。核对结果：

- `legacy_takeover.rs`（52 行）与 `source_migration.rs`（78 行）是二进制入口，由 `main.rs` 按第一个参数选择。它们只做进程层面的事：解析参数、把日志写到标准错误、读取配置、建立 4 个连接的数据库池、构造 `AppState` 并登记插件，然后调用 `plugins::singbox::legacy_takeover` 与 `source_migration` 的 `precheck`、`apply`、`rollback`，打印 JSON 报告并据此设置退出码。算法与事务都在插件中：`plugins/singbox/panel/legacy_takeover.rs`（418 行），`plugins/singbox/panel/source_migration/`（`import.rs`、`rehearsal.rs`、`rollback.rs`）。命令行里仅有的判断是：`apply` 只有在迁移完成且就绪时才算成功，`snapshot` 默认取当前时间，`compare` 在两份快照不同时以非零退出。
- `publisher.rs`（12 行）是库模块：一行兼容再导出（插件的 `publish_due`），加一个 `run()`。`run()` 运行宿主维护任务和**所有**插件的后台循环，并不只是发布器，名字容易误导；测试在使用它（`crates/panel/tests/e2e_support/mod.rs`、`tests/nexus_runtime_takeover.rs`）。真正的发布器是 `plugins/singbox/panel/publisher.rs`（408 行）。
- 同一 crate 中真正的策略代码在别处：`crates/panel/src/integration_policy.rs` 把插件接口前缀映射到功能区，判定哪些写操作属于高风险、哪些 POST 按读取处理、门户哪些路径使用独立登录，并有自己的测试。这是组装职责（宿主不能命名具体插件），但它让插件的权限规则离开了插件本身。

**结论：** 命令入口与进程组装留在应用层是合理的，插件继续拥有业务算法与事务。如需改善目录，可把命令入口归入组装 crate 的 `commands/`，不必把进程初始化搬进插件；`publisher.rs` 可以改名为后台任务的总入口（如 `workers.rs`），保留兼容再导出。`integration_policy.rs` 的去向 architecture.md §5.4 已经决定：各功能在 `Feature` 描述中声明路由与访问策略，装配 crate 不再手写路径前缀规则。迁移时逐条比对现有判定（高风险写操作、按读取处理的 POST、独立登录路径），保证权限结果不变，并单独评审。不能只凭文件名中的 `migration`、`publisher` 判断边界放错。

### 2.2 `compiler` 是代理配置编译库，不放进 `shared/`

- **依赖：** 只有 serde、serde_json、uuid、base64 与 thiserror，不访问数据库，也不发网络请求。
- **内容全部是代理配置语义：**
  - 用户凭据（`Access`）、节点与 Reality 密钥、计量名 `u{user}_n{node}`；
  - `compile_server` 与按代理用户的 `compile_client`，以及分享链接；
  - 八种协议（VLESS-Reality、VLESS 隧道、Hysteria2、SS-2022、TUIC、AnyTLS、Naive、Snell v6）与 TLS、ACME；
  - 节点设置与传输、逐用户限制（`limits.rs`）、两跳中转（`relays.rs`）；
  - 旧路径与有序多跳路径两个模块（经 `#[path]` 映射）；
  - 外部出站的校验与合并、Clash Meta（mihomo）配置、Nexus 接管的保留配置（`preserved.rs`）。
- **使用方：** 实际只有 sing-box 插件（42 个源文件引用它），组装 crate 只把它作为测试依赖。协作规则与分层检查把它写成“任何插件都可以依赖”的一层，但现在只有一个真实的使用方。

**结论：** `protocol`、`adapter-sdk` 可以解释为公共契约，`plugins/cloud_api/panel` 是多个集成共用的公共库，`compiler` 应明确解释为**代理配置编译库**。“被多个模块调用”不足以成为归入通用 `shared/` 的理由（共同复用原则）。ADR 0125 已把它归入 `proxy-nodes`（architecture.md §4.2）；在代理服务拆分（阶段 4）之前保留 `crates/compiler` 并补职责说明，移动时按 §2.3 的清单处理它的路径使用者。

### 2.3 “包名不变，下游引用不受影响”承诺过强

保持 Cargo 包名与公开 Rust API 只保住了按包名的引用；物理目录还被下面这些机制使用：

| 机制 | 现有用法 | 目录移动后 |
| --- | --- | --- |
| 编译期的基线路径 | `sqlx::migrate!("../panel/migrations")`：`crates/panel-host/src/schema.rs`（生产启动时的基线校验）及 7 处测试；`#[sqlx::test(migrations = …)]`：宿主与 IPQuality 共 44 处写 `"../panel/migrations"`，阿里云、DDNS、sing-box 插件共 165 处写 `"../../../crates/panel/migrations"`，组装 crate 自身 180 处写 `"./migrations"` | 编译失败或测试连到错误的基线 |
| 跨目录包含 | `schema.rs` 包含 `../../panel/schema_fingerprint.sql`；`installation.rs` 包含 `deploy/bootstrap.sh`（Windows 另含 `bootstrap.ps1`）；编译器测试包含 `tools/nexus-legacy-export/` 的夹具；`rust-embed` 的 `#[folder = "../../web/dist/"]` | 编译失败 |
| 跨 crate 的 `#[path]` | `crates/panel-host/src/diagnostic_plugins.rs` 把 `plugins/{ipquality,nodequality,tcpquality}/panel/mod.rs` 直接编进宿主（它们不是独立 crate）；77 处引用 `crates/protocol/tests/support/release.rs`；2 处引用 `crates/adapter-singbox/tests/support/mod.rs` | 编译失败 |
| 构建脚本 | 宿主的 `build.rs` 监视 `web/dist` 并断言 `index.html` 存在，监视基线目录并读取 `Cargo.lock` | 构建失败或不再触发重建 |
| 容器构建 | 根 `Dockerfile` 逐个复制 `Cargo.toml`、`Cargo.lock`、`crates/`、`deploy/`、`plugins/`、`web/dist`；`deploy/docker-compose.yml` 的构建上下文；`.dockerignore`；`tools/singbox-builder.Dockerfile`、`tools/openrc-test.Dockerfile` | 新的顶层目录不会进入镜像 |
| CI 工作流 | `ci.yml` 按路径运行 44 个 `tools/test-*.py` 与 4 个 `scripts/test-*.py`，缓存键包含 `tools/build-singbox.sh` 与 `tools/singbox-runtime/**`；`release.yml` 引用 `deploy/install.sh.tmpl`、`deploy/apsp-agent.service` 与 `plugins/sing-box/apsp-singbox@.service` | 工作流失败，或缓存不再随内容失效 |
| 脚本与工具 | `plugins/sing-box/` 的服务定义被 `tools/release.py`、`render-bootstrap.py`、`openrc-smoke.py` 等引用；`plugins/nodequality` 被 44 个工具文件引用；部分测试用 `Path(__file__).with_name(...)` 加载被测文件 | 运行失败；测试必须与被测文件一起移动 |
| **随安装器分发的清单** | `deploy/bootstrap.sh` 内嵌一份按仓库路径列出的文件清单（约 20 个 `tools/*.py`、33 个 `plugins/nodequality` 文件、6 个 `plugins/ipquality` 文件与 `LICENSE`），由 `tools/render-bootstrap.py` 生成；`tests/test_bootstrap.py` 断言它与重新生成的结果逐字节相同；宿主以 `include_bytes!` 内嵌安装器 | **已分发的安装器内容随之改变** |
| 分层检查 | `tools/check-core-boundary.py` 写死扫描 `crates/agent-core` 与 `crates/panel-host`，例外表达式以相对这两个目录的路径为键；适配器与插件的清单用通配符匹配（`crates/adapter-*/Cargo.toml`、`plugins/*/panel/Cargo.toml`）；`tests/test_core_boundary.py` 写死同样的路径 | 见下 |
| 仓库外 | 生产部署器读取 `scripts/baseline-adoption-*.sql`；协作规则写死 `plugins/singbox/panel/` 与 `crates/panel/migrations/0001_current.sql` 的位置 | 生产部署找不到接入脚本 |

分层检查在目录移动后的行为已逐项核对：**被扫描的目录移走后，检查静默通过**（对不存在的路径递归匹配得到空集合）；固定的清单文件移走后，检查报错；适配器或插件移走后，它们静默地退出依赖规则。另有一个现存缺口：经 `#[path]` 编进宿主的三个诊断插件目录从未被扫描。

**结论：** 表述应改为“保持包名与公开 Rust API；物理路径的使用者、构建与发布入口、脚本调用和外部路径依赖另行迁移并验证”。还必须核对**检查器是否仍覆盖原来的对象**：目录移动后，固定路径或通配符匹配不到目标时，检查会照常通过却少查了一部分。

这张表也是 ADR 0125 阶段 1–4 的路径使用者清单：`plugins/sing-box` 并入 `plugins/singbox/assets`、诊断插件去掉 `#[path]`、宿主中的功能移出、代理服务拆为六个 crate，每一步都会碰到上面的机制。新的 `tools/check-feature-boundaries.py` 同样按路径判断所有者（`tools/feature-boundaries.toml` 的 `[rust]` 与 `[web]` 两节），移动后要同步更新归属清单，并比对移动前后它实际检查的文件集合。

### 2.4 测试命名需要统一，但不能据命名推断测试没有执行

截至 2026-11-07，仓库的四个工作流都处于手动停用状态；下文的“CI 执行”指工作流文件会执行它，不表示最近执行过。

- **按路径显式执行：** `ci.yml` 按路径运行 44 个 `tools/test-*.py` 与 4 个 `scripts/test-*.py`；`tools/test-openrc-smoke.py` 由 `tools/openrc-test.Dockerfile` 运行。
- **自动发现只用于 `tests/`：** `python3 -m unittest discover -s tests -p 'test_*.py'`。带连字符的文件名不能被 unittest 自动发现（文件名必须匹配 `[_a-z]\w*\.py`，子目录还需要 `__init__.py`），所以它们只能显式执行，但“不能自动发现”不等于“没有执行”。68 个带连字符的测试文件中有 64 个调用 `unittest.main()`，单独运行都能工作。
- **确实从未进入 CI 的：**
  - 标为人工执行的：NodeQuality 的 8 个测试、`tools/test-p0-joint-load.py`、`plugins/ipquality/test-policy.py`、`scripts/test-recovery.py`、7 个 `tools/test_*.py`（Nexus 与 CDT 迁移）、3 个 `tools/test-*.cjs`；
  - **没有任何引用的**：`tools/test-network-recovery-safety.py`（覆盖 Agent 内嵌的设备侧恢复脚本）、`tools/test-panel-manager.py`（覆盖 `scripts/panel.py`）、`scripts/test-operations-clis.py`（覆盖 `panel.py`、`recovery.py`、`node-backup.py`、`observe-panel.py`）、`tools/test-probe-lease-acceptance.py`、`tools/singbox-runtime/test-user-limits.py`。也就是说，日常运维工具与设备侧恢复脚本的测试从未进入 CI。
  - `tools/test-managed-paths-linux.py` 与 `tools/test-ordered-paths-native.py` 名为测试，实际是验收驱动；CI 中的契约测试把它们当模块导入。

**结论：** 先整理“测试类别—执行命令—环境要求—CI 是否执行”清单（§6），再决定是否统一命名与运行器；重命名后比较迁移前后实际执行的测试集合。

## 3. 文档按有效性整理

### 3.1 先判断有效性，再决定位置

上一版方案提出“带日期的内容一律进 journal 或 archive”。这个规则过于简单：带日期的内容可能仍是当前有效的架构决定、尚未解除的执行门禁、当前数据库的接入要求、尚未关闭的问题，或对当前版本仍有效的验收证据。每份文档先补齐下面四项，再决定位置：

| 信息 | 取值 | 示例 |
| --- | --- | --- |
| 状态 | 现行、提议、已被取代、历史记录 | ADR 0098 的分组部分已被 ADR 0102 取代 |
| 适用范围 | 当前开发基线、某个发布版本、某类部署 | `scripts/baseline-adoption-20261107.sql` 只适用于 2026-11-07 部署的生产库 |
| 替代关系 | 被哪个 ADR 或现行规范取代 | ADR 0010 的 Agent 下载部分被 ADR 0038 取代 |
| 未完成条件 | 仍待验收、仍待退出兼容、仍有运行依赖 | NodeQuality 完整验机门禁未解除 |

### 3.2 分层

| 层 | 读者 | 内容 | 写法（Diátaxis） |
| --- | --- | --- | --- |
| 现行规则 | 所有开发会话 | AGENTS.md、功能地图、对象词典、现行 ADR | 规范：每条规则一个出处 |
| 使用文档 | 管理员、部署者 | 安装、接入、各工作区的操作说明 | 操作指南与教程 |
| 开发参考 | 开发会话 | 接口、协议、仓库说明、模块说明、开发与检查命令 | 参考 |
| 设计说明 | 开发会话、评审 | ADR、设计方案、迁移方案 | 解释 |
| 历史记录 | 追溯 | 已完成的进度、验收证据、已执行的一次性脚本 | 记录：注明当时的源码与适用范围，不再承诺持续有效 |

现有文档按这五层大致归类如下（`docs/` 顶层）：

| 层 | 文档 |
| --- | --- |
| 现行规则 | `product-map`、`database-baseline`、`api`、`protocol`、`glossary`、`platforms`、`release`、`repository` |
| 使用文档 | `deploy`、`singbox-installation`、`proxy-protocols`、`node-catalog`、`singbox-groups`、`agent-runtime-and-chains`、`passkeys`、`singbox-operations-workflows`、`singbox-snapshot-editing`、`server-display`、`server-display-data`、`statistics`、`server-assets`、`fleet-operations`、`operations-recovery`、`monitoring`、`network-workbench`、`network-configuration`、`ddns`、`plugin-catalog`、`alicloud`、`alicloud-ram`、`cdt-script-migration`、`control-center`、`control-center-search`、`apsp-renaming` |
| 开发参考与设计说明 | `dev`、`e2e`、`node-chain-design`、`chain-subscription-sources`、`subscription-source-implementation-notes`、`nodequality-rootfs-collection`、`nodequality-rootfs-factory`；ADR；`roadmap`、`rearchitecture-phase3-plan`、`operations-expansion`（计划与提议） |
| 历史记录 | `PLAN`、`requirements`、`acceptance-bounded-usage`、`nexus-migration`、`nexus-agent-migration`、`open-questions`（按日期的决定记录）、`acceptance/` |

现状中与有效性有关的问题：

- **没有一份文档带“现行、被取代、历史”这类状态字段。** 状态只出现在少数文件头或文档索引中：`docs/README.md` 把 `requirements`、`PLAN` 标为历史基线、把路线图标为提议，文件本身却没有写，`requirements.md` 读起来仍像一份现行任务书。
- **文件头过时。** 阶段三方案的文件头仍写 S1 进行中，正文已写 S1 全部完成；`fleet-operations`、`network-workbench`、`network-configuration`、`operations-recovery`、`operations-expansion` 的开头仍是“本轮”的口吻（如“写作时未运行测试”）。
- **验收记录**：`docs/acceptance/` 有 158 份 Markdown、9 份 JSON，`evidence/` 另有 37 份 JSON。其中 `ordered-remediation.md` 仍是生效中的门禁状态文档，不能随其他验收记录一起归档。
- **ADR**：共 125 份，其中 38 份没有状态行（包括 0119、0120、0122、0124）。替代关系通常只写在后继 ADR 或索引里，被取代的 ADR 自己很少注明（只有 0010 在状态行写明）。编号有两处卫生问题：0043 有两份不同的文件；0043 与 0049、0052 与 0066、0053 与 0067 是内容相近的重复决定（[执行计划 C8](09-execution.md#3-当前规范冲突清单)）。

### 3.3 归档规则

- 历史验收记录与一次性生产接入 SQL **默认归档保留**，标明适用基线与执行记录。是否删除历史证据不是本次整理的必选问题。
- PROGRESS 按路线图 2.4 按月归档到 `docs/history/`；主文件只保留当前状态。
- 被取代的 ADR 不移动、不删除，只在文件头和索引中写明被谁取代（编号保持稳定）。

## 4. AGENTS.md 整理为现行规则

单纯把长段文字挪到另一个文件，仍会留下“哪条生效”的解释成本。整理的目标是一份**现行规则**，旧条款显式退出：

1. **现行规则只写现在生效的约束**，按主题组织：提交身份、交付方式、生产边界（自动部署、数据库基线、接入脚本）、分层与依赖、实现与检查、范围（引用功能地图“不做什么”）、文档规则。
2. **每条旧条款有去向**：保留（并入现行规则）、合并（写明并入哪条）、退出（写明被哪个 ADR 或用户决定取代、何时）。去向表放在历史记录中，现行规则只链接它。
3. **授权记录与规则分开。** “2026-10-0x 用户授权某功能”是范围变更的来源，属于历史记录；现行规则只保留由此产生的约束（例如“凭据留在面板插件，真实云操作不属于开发验证”）。
4. **未确认失效的条款不删除。** 例如 CI 发版例外中没有写明结束的几条（[执行计划 C12](09-execution.md#3-当前规范冲突清单)），核对实际状态后再记为结束。

按 main `a32ec49b` 逐条核对的结果（去向是建议，整理时以用户确认为准）：

| 条款 | 现状 | 建议去向 |
| --- | --- | --- |
| 提交身份 | 现行 | 保留 |
| 2026-11-07 未发布阶段收敛、自动部署与接入脚本 | 现行 | 保留；发布部分改为引用候选版本门禁（[收敛方案 §5](07-convergence.md#5-候选版本身份与发布门禁)） |
| 临时 CI 暂停 | 仍生效：2026-11-07 核对，四个工作流均为手动停用 | 保留到 CI 恢复（路线图 2.1），届时整节退出 |
| CI 发版例外 0.3.3、0.3.4、0.3.5 | 没有写“本例外结束”，但对应标签都已发布，工作流已重新停用（0.3.8 的例外已于 2026-11-07 补记结束） | 补记结束后移入历史记录 |
| 本轮交付方式（2026-10-01：集成分支、一次交付） | 已被 2026-11-07“直接在 main 完成”及 #249–#256 的实践取代 | 退出；现行交付规则由用户确认（C9） |
| 2026-10-03 平台扩展与集中交付 | 范围授权仍有效；“执行阶段不运行验证、整体一个 PR 交付”只适用于那一轮 | 范围并入“当前范围”，过程规则退出 |
| 本轮执行节奏（中途不测试、大步骤提交） | 是否仍适用不明确 | 由用户确认；保留则并入“实现与检查” |
| 分层：“旧地址必须在 `web/src/app/paths.ts` 中改写、不得删除” | 已被 ADR 0122 与 2026-11-07 的“清理旧页面别名”取代；文件已删除，测试断言旧地址返回不存在 | 改为地址策略（[六工作区方案 §6](04-workspaces.md#6-地址)，C2） |
| 分层：业务插件列表（sing-box、DDNS、阿里云） | 缺费用插件；`adapter-ipquality` 在协作规则与仓库说明中都没有出现 | 补齐（C19） |
| 分层：功能划分（ADR 0125）与功能边界检查（2026-11-07 加入） | 现行 | 保留；它与“前端目录与功能区对应”一句的关系按 §7.1 写明 |
| 分层：“管理界面…按功能地图的侧栏分组组织（六组，降级功能集中在「实验功能」组）…前端目录与功能区对应” | “六组”与代码、功能地图都不一致（C1）；“前端目录与功能区对应”需按入口级理解（§7.1，C22） | 按六工作区方案改写分组；前端一句改为 §7.1 的表述 |
| 实现与验证：“按 `docs/PLAN.md` 中 G1–G9 顺序推进”“每阶段一个提交” | 历史 | 退出（C11） |
| 实现与验证：“界面只使用中文” | 与 2026-10-06 起的英文界面翻译矛盾，没有对应 ADR | 由用户确认（C18） |
| 禁止扩大 MVP 范围：排除清单 | 多数已被后续授权覆盖，包括多管理员（ADR 0107）与多语言界面 | 改为“当前范围与明确不做”（C10） |
| “整改清单每一项独立 PR” | 已被 2026-10-01 的一次交付取代 | 退出 |
| 2026-10-02 Passkey 段中的“不增加…多管理员” | 已被 ADR 0107 取代 | 并入范围说明 |
| 各项授权段落（2026-10-01 至 11-06） | 范围来源，仍有效 | 授权记录移入历史记录；由此产生的约束并入现行规则 |

## 5. 工具按用途分类

按使用者、执行环境与生命周期分类，而不是按“谁在仓库里看到它”：

| 类别 | 使用者 | 执行环境 | 生命周期 | 现有示例 |
| --- | --- | --- | --- | --- |
| 日常运维入口 | 部署者、管理员 | 面板主机或独立主机 | 长期 | `scripts/panel.py`、`recovery.py`、`node-backup.py`、`observe-panel.py`（在独立主机运行）、`init-env.py` |
| **设备侧载荷** | Agent 安装器 | 每台服务器 | 长期，随安装器分发 | `tools/bootstrap.py`、`legacy_agent_checkpoint.py`、`release.py`、`artifact_manifest.py`、各诊断制品模块（`tcp_probe_*`、`nodequality_*`、`ipquality*`）、`network-workbench.py`；由 `tools/render-bootstrap.py` 嵌入 `deploy/bootstrap.sh` |
| 构建与发布工具 | 发布者、CI | 构建机 | 长期，与发布流程绑定 | `build-agent.py`/`.sh`、`build-singbox.sh` 与 `singbox-builder.Dockerfile`、`build-runtime-native.py`、`singbox-runtime/`、`build-tcp-probe.py`、`build-nodequality*`、`publish.py`、`render-bootstrap*.py`、`verify-release-runtime.py`、`licenses/` |
| 检查与 CI 包装 | CI、开发会话 | CI、开发机 | 长期 | `tools/ci-run.py`、`check-core-boundary.py`、`check-source-artifacts.py`、`agent-smoke.py`、`native-service-smoke.py`、`openrc-smoke.py`、`openrc-test.Dockerfile`、`freebsd-ci-prepare.sh`；`scripts/ci-*`、`verify-e2e-runtime.py` |
| 验收驱动 | 验收者 | 专用测试机 | 长期，需要授权 | `scripts/e2e-driver.py`、`e2e-real.sh`、`e2e-http-fixture.py`、`e2e-traffic-evidence.py`、`diagnostic-baseline.py`；`tools/acme-smoke.py`、`p0-joint-load.py`、`probe-lease-acceptance.py`、`carrier-monitoring-acceptance.py`、`prepare-*-acceptance.py`、`managed-paths-*`，以及名为测试、实为驱动的 `test-managed-paths-linux.py`、`test-ordered-paths-native.py` |
| 一次性迁移 | 操作者 | 生产（需授权） | 执行后归档 | `scripts/baseline-adoption-*.sql`；`tools/export_wallos.py`、`migrate-cdt-guards.py`、`migration_snapshot.py`、`nexus_agent_migration.py`、`nexus_node_inventory.py`、`nexus_runtime_snapshot.py`、`nexus_runtime_takeover.py`（也用于后续追加）、`nexus-legacy-export/` |

两个例外单独标明：`tools/nexus_compatibility.py` 是长期运行的同步循环而不是一次性迁移（是否仍在运行需核对）；`tools/web-backdrop.py` 是一次性的素材生成工具。按 architecture.md §6，属于某个功能的工具随功能移动（如 Wallos 导入脚本移入 `plugins/expenses`，Nexus 工具归 `proxy-legacy`），`tools/` 只留跨功能的构建、检查与发布工具。

“设备侧载荷”一类说明上一版方案的前提并不成立：`tools/` 不只是维护者在开发时运行的工具，其中一批文件随安装器分发到每台服务器，移动它们会改变已分发的安装器（§2.3）。现有两份 README 也不完整：`tools/README.md` 漏列 37 个非测试文件（全部 `nexus_*`、`export_wallos`、`migrate-cdt-guards`、`ipquality-*`、`managed-paths-*` 等），`scripts/README.md` 漏列 `recovery.py`、`node-backup.py`、`observe-panel.py` 与两份接入 SQL。

- `scripts/` 与 `tools/` 的 README 各自按上表列出每个文件的类别、执行环境与是否需要授权。
- **统一检查入口**只收开发与检查工具（格式、分层、测试、前端构建），不收验收驱动与一次性迁移，避免无差别执行实机或生产操作。
- 一次性迁移执行后不删除，移入归档并注明执行时间、适用基线和结果记录。

## 6. 测试入口与发现规则

“CI”一列表示工作流文件会执行它；四个工作流目前都已停用（§2.4）。

| 类别 | 规模 | 命令 | CI |
| --- | --- | --- | --- |
| Rust 测试（不需要数据库） | 585 个 `#[test]`、439 个 `#[tokio::test]` | `cargo test --locked` | 是 |
| Rust PostgreSQL 测试（`#[sqlx::test]`，需要能建库的 `DATABASE_URL`） | 604 个：组装 395、阿里云 89、sing-box 47、宿主 38、DDNS 29、IPQuality 6 | 同上 | 是（PostgreSQL 16 服务） |
| Rust 忽略测试 | 30 个：真实 sing-box 17、root 与 systemd 8、ICMP 1、浏览器 1、真实 Globalping 1、只需 PostgreSQL 1、PostgreSQL 加运行时 1 | `cargo test -p … -- --ignored`，按 `docs/dev.md` 设置环境变量 | 只有 8 个（6 个 systemd 诊断、2 个运行时） |
| `tests/test_*.py` | 16 | `python3 -m unittest discover -s tests -p 'test_*.py'` | 是 |
| `tools/test-*.py` | 60 | `python3 tools/test-X.py`（部分需要 sudo） | 45 个 |
| `tools/test_*.py`（Nexus 与 CDT 迁移） | 7 | `python3 -m unittest discover -s tools -p 'test_nexus*.py'` | 否 |
| `scripts/test-*.py` | 6 | 直接运行 | 4 个 |
| 其他 Python 测试 | 2：`plugins/ipquality/test-policy.py`、`tools/singbox-runtime/test-user-limits.py` | 直接运行 | 否 |
| 前端单元测试 | 44 个文件、247 个用例 | `cd web && bun test` | **否**：`web/package.json` 没有测试脚本，CI 只运行 `bun run build` |
| 浏览器回归 | 约 68 个套件与 6 个辅助文件（`web/tests/*.mjs`） | 构建后 `bun web/tests/<套件>.mjs`，需要 Playwright 与浏览器路径；没有统一的运行脚本 | 否 |
| 旧浏览器测试 | 3 个 `tools/*.cjs` | `node tools/test-….cjs <地址>` | 否 |
| Go 测试 | 1 个（一次性导出工具 `tools/nexus-legacy-export/`） | 复制进旧源码后 `go test` | 否 |
| 冒烟与端到端 | Compose、OpenRC、Agent（Linux、原生、FreeBSD）、原生服务、运行时构建、完整安装（只在 PR 与 main） | 各自的脚本 | 是；另有人工执行的端到端、ACME、负载与拨测许可验收 |
| 静态检查 | `cargo fmt`、`clippy`、分层检查、`check-source-artifacts.py` | — | 是 |

清单暴露的缺口：

- `crates/panel/tests/user_limits.rs`（ADR 0124 的逐用户限制）只需要 PostgreSQL，却标为忽略；CI 的主测试步骤不传 `--ignored`，所以它从未在 CI 运行，而 CI 本来就有 PostgreSQL。
- 前端单元测试与浏览器回归都不在 CI 中。
- 日常运维工具与设备侧恢复脚本的测试没有任何执行入口（§2.4）。

整理规则：

- 前端单元测试（`bun test`）与浏览器回归（`web/tests/*.mjs`）分开运行与报告：前者快、无需构建；后者依赖构建产物与浏览器。在 `web/package.json` 中加上对应脚本，CI 恢复时纳入。
- 统一命名之前，先固定“类别—命令—环境要求—CI 是否执行”清单；重命名或更换运行器后，比较前后实际执行的测试集合。
- 只需要 PostgreSQL 的测试不标忽略（CI 有 PostgreSQL 服务，现有 `#[sqlx::test]` 都是这样运行的）；需要真实运行时、root 权限或测试机的测试继续使用带原因的忽略标记，并在清单中写明人工步骤。协作规则“需要 PostgreSQL 或真实运行时而环境不可用的测试使用带原因的 `#[ignore]`”按此改写。

## 7. 前端按稳定领域组织

### 7.1 入口属于功能，工作区只是分组

**侧栏是访问功能的方式，业务目录表达对象和逻辑归属。** 同一个领域完全可能出现在多个页面中。以域名解析为例：

- 全局页面查看全部规则；
- 服务器详情查看这台服务器的规则；
- 告警中进入发生异常的规则；
- 概览显示同步失败的摘要。

这些入口应当复用同一份实现。导航受角色、能力、屏幕与布局影响，代码不应随每次导航调整重新搬家；architecture.md §2.1 同样把“界面布局”列为应当隐藏的易变决定。

[ADR 0125](../adr/0125-feature-modules.md) 的前端设计（architecture.md §5.5）已经给出机制：每个功能目录 `features/<功能>/` 用 `index.ts` 导出清单（路由、侧栏入口与分页、所需权限、待办提供者、文案），外壳据此生成路由与侧栏；服务器详情的分页由各功能登记。它写的“与侧栏一一对应”应当按**入口级**理解，这样与评审意见一致：

- 每个入口（页面、分页）只属于一个功能目录，由该功能实现；
- 侧栏分组（工作区）由几个功能的入口组成，分组是清单中的一个属性，调整分组只改清单，不搬目录；
- 同一对象出现在多个位置时（全局页面、服务器详情的分页、概览摘要），都使用所属功能导出的组件与查询。

现行规则“页面目录与功能区一一对应”（[执行计划 C5](09-execution.md#3-当前规范冲突清单)）据此改为：

> 前端按功能目录组织（ADR 0125）；每个入口属于一个功能，侧栏分组由清单声明；页面和侧栏组合功能导出的组件；同一对象允许多个入口，但只有一套模型和编辑逻辑。

### 7.2 目录的准入条件

把 `api.ts`、`hooks.ts`、`types.ts`、全部组件与全部 store 放进一个 `shared/`，会形成新的公共杂物箱。共享基于稳定语义与依赖方向，而不是文件被导入了几次。按 architecture.md §5.1 的目录名：

| 类别 | 目录 | 放什么 | 准入条件 | 依赖方向 |
| --- | --- | --- | --- | --- |
| 外壳 | `app/` | 会话、路由、外框与布局、导航（读取各功能清单） | 只做组合，不含功能规则 | 可依赖下面所有层 |
| 功能 | `features/<功能>/` | 一个功能的模型、查询、编辑组件、页面与清单 | 对外只经 `index.ts` | 只依赖界面组件库与通用工具；功能之间只经对方的 `index.ts` |
| 界面组件库 | `ui/` | 按钮、输入、弹窗、表格、状态组件 | 不含任何业务名词 | 只依赖通用工具 |
| 通用工具 | `lib/` | 格式化、请求原语、时间与数字处理 | 不含任何业务名词 | 不依赖其他层 |

现有 `web/src` 顶层目录与 ADR 0125 功能的对应（与 `tools/feature-boundaries.toml` 的 `[web]` 一节一致）：

| 层 | 现有目录（文件数） |
| --- | --- |
| 外壳 | `app/`（10）、`stores/`（6）、根目录文件（16，其中 `types.ts`、`i18n.tsx` 混有各功能的私有内容） |
| 内核的界面 | `servers/`（21）、`system/`（9）、`control-center/`（9）、`plugins/` 根目录的插件中心文件（5） |
| `proxy` | `plugins/singbox/`（71） |
| `dns`、`cloud`、`costs` | `plugins/ddns/`（9）；`plugins/alicloud/`（14）；`plugins/expenses/`（11）与 `subscriptions/`（2） |
| `alerts`、`lines` | `monitoring/`（10，其中延迟检测与中国可达性属 `lines`）；`network-workbench/`（4）、`server-diagnostics/`（5） |
| `overview` | `display/`（24）、`statistics/`（4） |
| `lab` | `operations/`（10）、`network-configuration/`（8）、`fleet/`（8） |
| 界面组件库 | `components/`（34，含 `components/ui`）与根目录的 `components.tsx`；`display/ui.tsx` 是另一套，architecture.md §5.5 计划合为 `ui/` |
| 通用工具 | `lib/`（11） |

需要处理的现状：

- `plugins/singbox/` 是 56 个文件的平铺目录，另有 `accounts/`（12）与 `lines/`（3）。页面、表单片段、类型模块与请求辅助混在一起；按字节计最大的文件是 `Chains.tsx`（约 41 KB）、`NodeCatalog.tsx`（33 KB）、`OrderedResources.tsx`（28 KB）、`SubscriptionSources.tsx`（27 KB）、`Nodes.tsx`（25 KB）。这些文件的行很长，行数会低估规模，应按字节衡量。
- 目录外有 5 个文件直接导入 `plugins/singbox/` 的内部文件；`monitoring/ChinaReachability.tsx` 与 `statistics/Statistics.tsx` 直接调用 `/api/plugins/sing-box/` 接口，绕过了领域的公开查询。边界检查目前记录了 57 处前端跨功能引用（architecture.md §7）。
- `subscriptions/` 是管理员自己的付费服务看板，与代理用户的订阅同名（[对象词典 §5](03-objects.md#5-多义词的处理)）；迁到 `features/costs/` 时改用“付费服务”。

### 7.3 代理服务前端按子模块与对象分组

采纳上一版方案的方向，并与 ADR 0125 的 `proxy` 子模块对齐：`features/proxy/` 下按 nodes（节点、链路、外部来源）、users（用户、策略与套餐）、delivery（客户端订阅、专属地址）、portal（门户、邀请）、runtime（运行时、部署）、legacy（迁移与兼容）分组，同一对象复用同一组查询与编辑组件。服务器详情中的“网络服务”分页、概览中的摘要都引用这些组件，不另写。

## 8. 模块说明与能力清单

### 8.1 模块说明

architecture.md §5.1 已规定每个功能的说明放在 `docs/features/<功能>.md`；crate 根文件的 `//!` 写一句职责并链接到这份说明。说明固定包含：

| 项 | 写什么 |
| --- | --- |
| 职责 | 一句话；对应对象词典中的哪些对象 |
| 写入权 | 它拥有哪些表与对象，谁可以改 |
| 依赖方向 | 依赖谁、被谁依赖，禁止的依赖 |
| 执行位置 | 面板、Agent、两者，或构建与发布环境 |
| 支持条件 | 平台、权限、门禁（如完整验机门禁）、需要的制品 |
| 入口 | 接口前缀、命令、页面、后台任务 |
| 测试 | 测试位置与运行命令 |

现状：

- **没有任何 crate 或插件有 README**，`Cargo.toml` 也都没有 `description`。现有的说明类文件只有 `tools/README.md`、`scripts/README.md`、`tools/singbox-runtime/README.md`、`plugins/ipquality/SOURCE.md` 与 `plugins/nodequality/UPSTREAM.txt`。
- crate 根文件的 `//!` 职责说明：宿主、组装 crate，以及 sing-box、DDNS、阿里云、费用、云接口插件有；`protocol`、`compiler`、`agent-core`、`agent`、`adapter-sdk`、四个适配器、`tcp-probe` 与三个诊断插件没有。
- 集中的职责表在 `docs/repository.md` 与 AGENTS.md「分层」，两处都漏了费用插件与 `adapter-ipquality`（[执行计划 C19](09-execution.md#3-当前规范冲突清单)）。
- 上游来源说明只有 IPQuality 与 NodeQuality 两处；其余参考了上游的模块按[功能盘点 §9](02-inventory.md#9-上游参考与自维护)补齐。

### 8.2 能力清单

方向正确，但字段要分开记录，不能用一个“级别”同时表达几件事：

| 字段 | 含义 | 取值 |
| --- | --- | --- |
| 功能归属 | 属于哪个领域与工作区 | 六工作区之一，及所属对象 |
| 成熟度 | 实现到哪一步 | 规划、源码合入、本地验证、实机签收、已发布 |
| 维护级别 | 投入方式与展示 | 核心、支撑、可选、实验、冻结 |
| 执行位置 | 在哪里运行 | 面板、Agent、两者 |
| 权限 | 需要的能力 | 如 `proxy:write`、`servers:read` |
| 门禁 | 额外的执行条件 | 如“本机授权远程命令”“完整验机门禁” |
| 界面入口 | 页面与地址 | 来自导航定义 |
| 文档 | 使用文档与设计依据 | 链接 |

能力清单是机器可读的，导航的分级标记、功能地图的分级表与文档索引都从它生成或与它核对，减少多处手写。ADR 0125 的 `Feature` 描述（标识、权限、路由与访问策略、后台任务、表结构片段）与前端 `index.ts` 清单（路由、侧栏入口、权限、待办、文案）已经覆盖其中的功能归属、权限与界面入口；成熟度、维护级别、执行位置与门禁作为同一描述中的字段补上即可，不另建一份文件。

## 9. 局部归位

两个目录名只差一个连字符，内容完全不同：

- `plugins/sing-box/` 不是 crate，只有三个设备服务定义：`apsp-singbox@.service`（systemd 模板）、`apsp-singbox.openrc` 与 `apsp-singbox-legacy.openrc`。目录名与设备上的安装路径（`/opt/apsp/plugins/sing-box`、`/var/lib/apsp/plugins/sing-box@<实例>`）及接口前缀 `/api/plugins/sing-box/` 一致；发布与安装工具按仓库路径引用这些文件（`tools/release.py`、`render-bootstrap.py`、`openrc-smoke.py`、`release.yml` 等，§2.3）。
- `plugins/singbox/panel/` 是 crate `apsp-plugin-singbox`（134 个源文件），协作规则要求它留在这里。

建议：

- 按 architecture.md §5.1，三个服务定义并入 `plugins/singbox/assets`。移动时按 §2.3 的清单更新全部引用（发布脚本、安装器生成、`release.yml`、OpenRC 冒烟测试），并核对签名清单与安装模板中的路径；`tools/feature-boundaries.toml` 中 `plugins/sing-box` 一行同步删除。**设备上的路径与接口前缀不变**，它们是外部契约。收益：仓库里只剩一个 sing-box 目录名，服务定义与它所属的功能在一起。
- 面板组装 crate 中的命令入口可以集中到 `commands/`（§2.1），保持命令名与退出码不变；`publisher.rs` 改为后台任务总入口的名字。
- 服务单元、构建材料与说明放在各自所属的位置，移动时附收益说明（R18）。

## 10. 暂缓与不做

| 建议 | 处理 | 理由 |
| --- | --- | --- |
| 所有 Rust crate 按新目录树整体移动（新增 `shared/`、`panel/app/`、`agent/bin/` 等层级） | 不做；由 ADR 0125 的分阶段迁移取代 | 上一版方案的目录树没有说明移动解决什么耦合。ADR 0125 只按功能边界移动（宿主收口、诊断工具改为登记、代理服务拆为六个子模块），每个阶段有完成标准，并由边界检查守住；§2.3 是这些阶段要处理的路径使用者清单 |
| 所有实验能力搬入 `labs` 并默认关闭编译 | 重新设计 | 维护级别是元数据（R6）。转正时还要再搬代码；编译关闭会影响已有对象的读取、进行中的任务与恢复能力；通用任务、取消确认、服务管理、特权边界仍被正式功能使用。ADR 0125 的 `lab` 不关闭编译，只要求它是叶子（核心功能不依赖它），与 R6 一致；其他实验与冻结能力留在所属功能（完整验机在 `lines`，Hetzner 在 `cloud`）。`lab` 中某一项转正时，按 R18 评估归入所属功能还是给模块改名，不因级别变化来回搬代码 |
| `subscriptions` 并入 `features/overview` | 不做 | 会固化“订阅”的语义混乱。完整功能归费用与续费 › 付费服务，概览只引用摘要（[六工作区方案 §4](04-workspaces.md#4-订阅一词的三个对象)） |
| 以“所有文件已经搬到新位置”作为完成标准 | 不采用 | 完成标准见[执行计划](09-execution.md)，以可检验的行为与一致性为准 |

## 11. 数据库基线与证据文件

### 11.1 拆分基线 SQL 是独立改动

按领域维护 SQL 源文件、再生成一份基线，可以讨论。但当前系统同时核对数据库结构与基线 SQL 的校验和（`crates/panel-host/src/schema.rs`）：校验和按 SQL 正文计算，**即使建出来的表完全相同，正文的任何字节变化都会让现有库被拒绝启动**。因此：

- **整理源文件，生成的基线逐字节不变**：按工程整理处理。
- **生成的基线发生变化**：按数据库基线变更处理，附带接入脚本，并绑定接入、部署与恢复验证（[收敛方案 §5](07-convergence.md#5-候选版本身份与发布门禁)）。

基线现有 6,674 行，这只说明规模较大。是否拆分取决于审阅困难、对象所有权与生成方式，不单独由行数决定。ADR 0125 阶段 3 已决定按功能分片、构建时合成一份并以指纹核验；上面两条正是分片时的判定标准：第一次分片应做到合成结果与现有基线逐字节相同，这样不需要接入脚本。

### 11.2 证据文件

证据 JSON 是否迁出仓库，先看实际体积、增长速度、变更频率与测试依赖。一次性 SQL 与历史验收记录默认保留，并标明适用状态。

## 12. 验收

1. §2 四条纠正写入现行文档（仓库说明、工程记录），不再作为待决问题出现。
2. 每份文档有状态、适用范围与替代关系；AGENTS.md 只含现行规则，每条旧条款的去向可查。
3. `scripts/`、`tools/` 的每个文件有类别、执行环境与是否需要授权；统一检查入口不包含验收驱动与一次性迁移。
4. 测试清单覆盖全部测试，标明 CI 是否执行；任何重命名前后，实际执行的测试集合一致。
5. 每个功能有 `docs/features/<功能>.md`，每个 crate 根文件有职责说明；功能描述与前端清单中的级别、权限和入口与导航、功能地图一致。
6. 每次文件移动附收益说明；路径使用者与检查器覆盖对象在移动前后一致。
