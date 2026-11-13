# 仓库目录与维护约定

本页说明现有代码与文档的位置；领域划分、依赖规则与迁移阶段见[功能划分与逻辑结构](architecture.md)（ADR 0128、ADR 0132），逐项功能的领域见[领域归属表](domain-map.md)。使用方法从[文档导航](README.md)进入，架构和范围以 [AGENTS.md](../AGENTS.md) 与[各项 ADR](adr/README.md) 为准。

## 顶层目录

| 路径 | 职责 |
| --- | --- |
| `crates/` | Rust 工作区：公共协议、面板平台、领域 crate、面板应用、Agent、适配器及原生 TCP 工具 |
| `web/src/` | React 界面（中文为源语言，英文为翻译，[ADR 0127](adr/0127-chinese-source-language.md)）；前端依赖与构建由 Bun 管理 |
| `web/dist/` | 仅在本地或构建环境生成、由面板嵌入的产物；不提交 Git，不手工编辑 |
| `deploy/` | Compose、Agent 与 sing-box 运行时的服务定义、安装模板、生成后的独立 bootstrap 入口和发布公钥 |
| `scripts/` | [面板运维、接入与验收入口](../scripts/README.md)；包含已有 CI 包装脚本 |
| `tools/` | [构建、签名、发布、规则检查与隔离测试工具](../tools/README.md) |
| `tests/` | 仓库工具的 Python 回归；Rust 和前端测试各自随所属代码放置 |
| `docs/` | 使用说明、接口契约、ADR、验收记录与证据 |
| `.github/workflows/` | 远端构建和验证定义；当前暂停，不因本轮整理恢复 |
| `PROGRESS.md` | 历史完成项、证据和未验证范围；保留既有记录与链接 |

`target/`、`web/node_modules/` 和本机临时目录属于忽略的本地状态，不作为源码整理对象。数据库仅维护当前基线，历史开发库手动重建，见 [ADR 0122](adr/0122-unreleased-schema-baseline.md)；公开安装脚本和签名制品结构仍按发布约定管理。

## Rust 工作区与分层

| 模块 | 职责与依赖方向 |
| --- | --- |
| `protocol` | 面板与 Agent 的共享消息、能力、签名制品契约 |
| `compiler` | 流量服务的配置编译：从业务模型确定性地生成完整运行时配置包；客户端配置部分供服务权益使用 |
| `panel-host` | 平台：HTTP 外壳与内嵌前端、数据库基线核对、共享状态与各领域运行时状态的槽、类型化接口机制、设备任务与观测证据的接口、后台任务守护、设置存储、通知发送与发件箱，以及通用的令牌、文本、摘要、地址与名称检查和 X.509 读取；工作区依赖仅 `protocol`，不依赖任何领域，也不含领域代码 |
| `cloud-api` | 平台的云接口客户端：签名、传输、重试与错误码；工作区依赖仅 `protocol` |
| `governance` | 平台治理领域：登录与会话、TOTP 与 Passkey（`auth/`、`passkeys/`），管理员与权限、请求守卫（路由权限与审计）、受管凭据、面板设置的管理接口、系统健康与工具安全（`control_center/`），签名发布与制品目录（`releases/`、`artifacts.rs`）；排在依赖顺序最前，只依赖平台 |
| `resources` | 资源领域：服务器、接入令牌与安装（`servers`、`installation`、`bootstrap`、`batch_enrollment`），设备通道与设备下载制品（`device_channel`、`device_artifacts`），资产、生命周期与退役（`server_assets`、`server_lifecycle`、`retirement`），云资源关联与 Hetzner（`cloud_resources`），云账号与云资源（`cloud/`：登记与凭据、实例与地址的读取、刷新流程的资源部分，以及向作业询问进行中操作的 `work` 接口） |
| `networking` | 基础网络领域：网络配置的文档、执行与服务器替换（`network_configuration/`，实验）、云安全组（`security_groups/`，冻结） |
| `naming` | 名称与证书领域：DNS 账号与记录、DDNS、DNS-01 验证记录，证书的 ACME 申请、续期、版本与部署；证书文档存放在基础网络的文档存储里，所以排在 `networking` 之后并依赖它 |
| `costs` | 成本领域：付费服务、付款、续费提醒与续费日历、供应商对照、计划退订提醒、汇率与统计币种，云的 CDT 计费流量、账单、余额与实例账单（`cloud/`）；直接读流量服务的外部来源到期 |
| `observability` | 观测领域：遥测与历史、网卡计数与矫正、拨测与延迟检测、共用诊断任务服务与诊断工具（IP 质量、NodeQuality、TCP 质量、网络检测）、告警规则与事件、故障事件、服务器健康与事件时间线、统计与看板、中国可达性、云事件通知（`cloud_notices`）；直接调用流量服务的运行活动；诊断工具的代码在 `diagnostic_tools/`，IPQuality 与 NodeQuality 的打包脚本、策略与上游锁定文件在 `upstream/`，签名制品与引导脚本里仍按原名 `plugins/...` 记录 |
| `operations` | 作业领域：远程命令、终端、固定设备操作及其核对、运行时操作，作业、计划、维护窗口与有界自动处置，Agent 的自更新检查（`agent_updates`），面板备份的计划、执行与恢复记录（`backups/`），云的带宽变更、启停、换 IP 与账号刷新（`cloud/`）；排在依赖顺序最后，直接调用前面的领域 |
| `traffic` | 流量服务领域：节点、链路、外部来源与节点库，服务器运行时的配置编译与发布、部署检查与发布批次、运行时控制与校验，Nexus 兼容桥的流量部分（`nexus_nodes/`）；用户的限速、设备数与运维记录经自己声明的 `users` 接口由服务权益回答 |
| `access` | 服务权益领域：代理用户、策略组与套餐、资格与配额、订阅与专属地址、成员门户与账号、用户限速与会话、代理运维流程（预览与应用）、Nexus 兼容（`nexus/`：兼容桥 `bridge` 与设备会话 `sessions`，兼容桥的表只在 `bridge` 中出现）；排在流量服务之后，直接调用它 |
| `panel` | 应用：组合平台与各领域（`domains`：一张路由清单、一张后台任务清单）、接线依赖倒置接口（`wiring`）、运行跨流量服务与服务权益的代理发布周期（`publisher`）与 30 秒维护循环（`maintenance`）、组合云页面总览与全局搜索（`cloud_overview`、`search`）、把旧接口地址改写到现地址（`moved_paths`）、启动服务与后台任务；保存唯一的当前数据库初始化 SQL。只导出自己定义的内容（`router`、`state`、`domains`、`maintenance`、`publisher`）；平台与各领域经各自的 crate 使用（`apsp_panel_host`、`apsp_governance`、`apsp_traffic` 等），不再有兼容的再导出 |
| `agent-core` | 设备身份、传输、对账、持久状态、遥测、计量与制品；工作区依赖仅 `protocol`、`adapter-sdk` |
| `adapter-sdk` | 无状态适配器接口与共享模型 |
| `adapter-singbox`、`adapter-nodequality`、`adapter-ipquality`、`adapter-tcpquality` | 运行时或工具翻译；工作区依赖仅 `adapter-sdk` |
| `agent` | 注册具体适配器、启动 core 的二进制入口 |
| `tcp-probe` | 原生 TCP 检测工具 |

领域 crate 只依赖平台、`protocol` 与依赖顺序中排在前面的领域（[功能划分与逻辑结构 §5](architecture.md#5-依赖与写入规则)）。服务器网卡总流量属于 Agent core 与观测；代理用户、授权、订阅、套餐与代理流量属于服务权益（`crates/access`）。`agent-core` 与平台 crate 都不引入运行时名称或代理业务类型。分层检查入口为 `tools/check-core-boundary.py`：检查 Agent core 与平台 crate 的禁用词，以及每个 crate 的层级与 `Cargo.toml` 依赖方向；新增 crate 先在其中登记层级。表与代码的归属由 `tools/check-feature-boundaries.py` 检查，路由权限由 `tools/route-policy.py` 与 Rust 测试 `crates/panel/tests/route_policy.rs` 对照结论表 `crates/panel/tests/fixtures/route-policy.tsv` 检查，三项检查的说明见[功能划分与逻辑结构 §5.3](architecture.md#53-自动守护)。

## 面板入口与领域接线

面板分为平台、领域和应用三层（[ADR 0128](adr/0128-domain-architecture.md)、[ADR 0132](adr/0132-platform-only-host-and-domain-addresses.md)）。平台 `crates/panel-host` 只有技术模块：`state.rs` 创建共享状态，`shell.rs` 是 HTTP 外壳（健康检查、前端兜底与全局请求体限制），`ports.rs` 提供类型化接口的安装机制。`crates/panel/src/` 是应用层：`domains.rs` 按依赖顺序合并各领域的路由，加上云页面总览与全局搜索两个组合视图，并逐个守护后台任务；`lib.rs` 给外壳套上治理的权限守卫与专属地址网关，旧接口地址先由 `moved_paths.rs` 改写到现地址；`wiring.rs` 在启动时接线全部类型化接口；`maintenance.rs` 运行 30 秒维护循环；`main.rs` 启动服务。设备通道（传输、设备认证、会话与在线状态、协议分派）是资源的 `device_channel.rs`。治理的 `passkeys/` 提供共用 WebAuthn 验证、挑战和凭据存储，管理员包装在它的 `auth/`，代理用户入口、邀请和独立会话在 `crates/access` 的 `portal/`。

每个领域 crate 导出一个 `routes()`，登记自己的全部接口，路径写完整，不用前缀嵌套：

| crate | 注册的接口 |
| --- | --- |
| `governance` | 登录、TOTP 与管理员 Passkey，面板设置，通知渠道与 Webhook 设置，管理与安全，制品目录与导入 |
| `resources` | 服务器、接入令牌、Agent 接入与公开安装入口，设备通道与设备下载制品，退役回执，接入模板与批量接入，服务器资产档案、生命周期、迁移与费用记录，云资源关联与 Hetzner 清单，云账号与云资源 |
| `networking` | 网络配置、云安全组 |
| `naming` | DNS 账号与记录、解析规则，证书、ACME、证书部署与 DNS-01 验证 |
| `traffic` | 节点、链路、外部来源与节点库，运行时安装与部署、发布批次，运行时检查点、设备的清单与配置包、运行时校验 |
| `access` | 代理用户、策略组与套餐、订阅与专属地址、成员门户与账号，以及客户端订阅 `/sub/{token}` |
| `costs` | 续费项目、付费服务与续费日历，汇率，供应商对照，CDT 流量历史 |
| `observability` | 统计、告警事件与规则、故障事件、网络检测、看板、遥测与历史、服务器健康与比较、流量矫正、拨测与延迟检测、IP 查询与共用诊断服务、中国可达性，以及 Agent 的遥测、拨测与诊断上报 |
| `operations` | 设备操作与终端、自动化与维护、远程命令与 Agent 的命令通道、运行时操作，Agent 的自更新检查，面板备份计划、导入的备份与恢复演练，云的带宽变更、启停与换 IP |

路由只组合处理函数，鉴权和业务校验由治理的请求守卫与处理函数负责。接口地址按对象所属的领域命名（[ADR 0132](adr/0132-platform-only-host-and-domain-addresses.md)）；新增接口放进所属领域的 `routes()`，并同步路由权限表（`python3 tools/route-policy.py`）。应用 crate 只导出 `apsp_panel::router`、`apsp_panel::state` 等自己定义的部分；`AppState` 与平台模块从 `apsp_panel_host` 引用，设备连接从 `apsp_resources::device_channel` 引用，领域模块从各自的 crate 引用。

领域与平台的接线：

- 领域之间需要逆着依赖顺序调用时，由调用方声明类型化接口、实现方回答、应用层接线，清单见[功能划分与逻辑结构 §5.5](architecture.md#55-类型化接口)。资源的设备通道把设备回报经 `DeviceReports` 交给各领域；不再有插件接口，也没有由各领域登记的钩子。
- 领域路由由应用层组合后放进平台的外壳；各领域的后台任务由应用层分别守护，一个崩溃不会停掉其他任务。
- `crates/governance/`、`crates/resources/`、`crates/networking/`、`crates/naming/`、`crates/traffic/`、`crates/access/`、`crates/costs/`、`crates/observability/`、`crates/operations/` 各自是独立 crate，只经平台公开接口与排在前面的领域使用面板能力；`crates/cloud-api/` 是平台的云接口客户端。排在前面的领域要向设备派发任务时经平台的 `device_work`（设备能否接收新任务、提交固定设备操作），作业实现、应用层接线。
- 诊断工具（IP 质量、NodeQuality、TCP 质量）是观测 crate 的 `diagnostic_tools/` 子模块，共用诊断服务负责生命周期、预算、取消和历史；不再有跨 crate 的 `#[path]`。
- 领域在进程内的运行时状态（实时遥测缓存、IP 查询提供方、在线设备连接、Passkey 服务）放在 `AppState` 的 `domain_states` 槽里，类型属于领域，平台不认识；测试用 `with` 换成替身。资源与成本经平台的 `evidence` 记录服务器事件与故障，资源的服务器视图与新服务器的拨测、延迟检测经资源声明的 `server_observation` 取得观测的数据；观测与成本同组、互不依赖，看板折算用的汇率经观测声明的 `dashboard::ExchangeRates` 取得。

数据库只有一份当前基线 `crates/panel/migrations/0001_current.sql`（[ADR 0122](adr/0122-unreleased-schema-baseline.md)）；平台通过 `sqlx::migrate!("../panel/migrations")` 嵌入它，领域 crate 的数据库测试也指向它（如 `crates/naming` 的 `#[sqlx::test(migrations = "../panel/migrations")]`）。修改基线是高风险改动，必须附带 `scripts/baseline-adoption-*.sql`，写法与部署验证见[数据库基线](database-baseline.md)「生产安装」；整理 SQL 而生成的基线逐字节不变时不算基线变更。

## 前端入口

| 路径 | 职责 |
| --- | --- |
| `web/src/main.tsx` | 入口：挂载 `app/App.tsx` |
| `web/src/app/App.tsx` | 会话读取、失效处理、公开看板访问控制与顶层页面组合 |
| `web/src/<领域>/pages.tsx` | 该领域的页面清单：每个页面的地址与旧地址、名称与图标、所在分组与位置、所属入口、打开权限（能力、是否须看见全部服务器）与按需加载的组件。新增页面只改这里 |
| `web/src/lib/pages.tsx` | 页面清单的类型与按需加载（平台层） |
| `web/src/app/routes.ts` | 解析 URL：带参数的地址（服务器详情、节点、代理用户、费用条目、看板）在这里解析，其余按各领域清单登记的地址与旧地址查找 |
| `web/src/app/navigation.ts` | 汇总各领域的页面清单：分组的名称与顺序、当前页面归属；功能分级标记在平台的 `lib/tiers.ts`。分组是 ADR 0126 的六个工作区，规则与对照见[功能地图](product-map.md) |
| `web/src/app/AdminPage.tsx` | 按解析结果选择页面：普通页面由清单绘制，带参数的页面在这里传入参数，保留服务器筛选和组件重挂载边界 |
| `web/src/app/AdminShell.tsx`、`Login.tsx` | 后台布局与登录表单 |
| `web/src/app/Overview.tsx`、`ServerDetail.tsx`、`CommandPalette.tsx` | 组合几个领域的页面：概览、服务器详情、命令面板（页面跳转与对象搜索） |
| `web/src/resources/` | 资源：服务器列表、接入、编辑与资产（`servers/`），云账号与云资源（`cloud/`） |
| `web/src/networking/` | 基础网络：网络配置（`configuration/`，实验）、云安全组 |
| `web/src/naming/` | 名称与证书：DNS 账号、记录与解析规则（`dns/`），证书与 ACME（`certificates/`） |
| `web/src/traffic/` | 流量服务：节点、链路、外部来源与节点库，部署检查与发布 |
| `web/src/access/` | 服务权益：代理用户、策略组与套餐、订阅与专属地址，成员门户与账号（`accounts/`）、成员线路（`lines/`） |
| `web/src/costs/` | 成本：费用与续费（`expenses/`）、订阅看板（`subscriptions/`）、货币与汇率（`currency/`）、CDT 计费流量（`cloud/`）、供应商对照 |
| `web/src/observability/` | 观测：服务器看板（`display/`）、统计（`statistics/`）、告警与线路质量（`monitoring/`）、网络检测（`network-workbench/`）、线路诊断（`server-diagnostics/`）、服务器的拨测与遥测设置（`servers/`）、云事件（`cloud/`）、故障事件 |
| `web/src/operations/` | 作业：自动化与维护（实验）、服务器的运维（`fleet/`）、远程命令，云的启停、换 IP 与 CDT 守护（`cloud/`） |
| `web/src/governance/` | 平台治理：管理与安全、外观偏好（`control-center/`），面板设置、制品与版本、备份与恢复、账号安全（`system/`），通知渠道与 Webhook（`notifications/`），Passkey |
| `web/src/api.ts`、`hooks.ts`、`components.tsx` | 共享请求、状态钩子和通用组件（页头、通知、弹窗、确认框、字段、复制） |
| `web/src/index.css` | Tailwind CSS v4 入口与全部设计参数：浅色/暗色颜色、圆角、密度、基础样式 |
| `web/src/components/ui/` | shadcn/ui Nova 组件源码（Radix UI 底层），按普通源码维护 |
| `web/src/components/` | 面板、分段导航、主从分栏（`list-detail.tsx`）、拖拽列表（`sortable.tsx`）与行内“更多操作”菜单（`action-menu.tsx`） |
| `web/src/lib/` | `cn()`、Motion 时长与预设、媒体查询及本地存储工具 |
| `web/src/stores/` | Zustand 跨组件界面状态：语言、外观、外框、会话与看板货币 |
| `web/src/i18n.tsx` | 语言上下文、浏览器语言选择、管理员偏好同步及静态界面词典 |

前端按领域目录组织（[ADR 0128](adr/0128-domain-architecture.md)、[ADR 0126](adr/0126-workspaces-language-and-addresses.md)）：每个入口属于一个领域，新增页面放进它所属领域的目录，并在该领域的 `pages.tsx` 里登记；侧栏分组由清单声明，调整分组不搬目录；同一对象允许多个入口，但只有一套模型和编辑逻辑。目录结构是 `web/src/<领域标识>/`、应用层的 `app/` 与平台的 `components/`、`lib/`、`stores/` 和根目录的通用文件（[功能划分与逻辑结构 §5.4](architecture.md#54-代码结构)），每个目录在 `tools/feature-boundaries.toml` 登记为一行；跨领域共用的数据模型由所有者公开（如观测的 `probes.ts`、资源的 `server-assets.ts`），其他领域直接引用所有者的文件。页面标题与入口同名，不加眉题，说明文字先讲“在这里做什么”。

界面约定见 [ADR 0088](adr/0088-ui-component-system.md)：组件使用 `components/ui/` 中的 shadcn/ui Nova 组件，样式只写 Tailwind 类并引用 `index.css` 的设计变量，不再新增 CSS 文件或颜色字面量；图标来自 `lucide-react`，图标按钮必须有 `aria-label`；动画只用 `motion/react` 的 `m.*` 与 `lib/motion.ts` 预设，需要阅读的内容进入时只做位移、不得从透明或隐藏开始；可调分栏通过 `components/ui/resizable.tsx`，拖拽排序通过 `components/sortable.tsx`；跨组件界面状态放入 `stores/`，服务端数据仍经 `useResource` 与写保护。分区导航用 `SegmentedNav`，按钮通过 `aria-pressed`、页面链接通过 `aria-current="page"` 标记当前项；测试钩子使用 `data-slot` 与 `data-dialog`。首次加载用 `Loading` 的 `table`、`cards`、`panel` 形态显示与内容一致的骨架；开关类设置行用 `SettingRow`；用量条用 `Meter`（默认 75% 警告、90% 危险）；页面跳转与对象搜索集中在 `app/CommandPalette.tsx`，可跳转页面取自侧栏并按当前账号权限过滤。行操作只直接显示常用项，其余放进 `ActionMenu`（“更多操作”）；内容丰富的宽表格用容器查询在窄面板中堆叠为卡片，仍保持同一张表。页面跳转用 `navigate()` 或普通链接，不直接写 `location.hash`；有未保存输入的表单用 `useLeaveGuard`（或 `useFormDraft`）登记，离开检查集中在 `lib/leave-guard.ts`。响应式布局只增删外围元素，不替换承载页面内容的组件树；随数据变化的状态文字用 `t()`。按条件显示的弹窗包在 `DialogPresence` 中，让关闭动画有机会执行；列表的次要筛选与视图设置放进 `ToolbarPanel`。

Hash URL 是书签与兼容入口，地址按 ADR 0126 的地址策略管理：已发给代理用户的地址永不失效，控制台地址改名后至少保留一个发布版本的改写。新增页面要同时考虑路由、导航、管理员访问边界；公开访问只放行明确的看板路由，不据路径前缀推断权限。

## 验证与文件维护

- Rust 单元测试随模块放置；HTTP/PostgreSQL、协议与编译集成测试位于对应 crate 的 `tests/`。
- `web/tests/*.test.ts` 使用 `bun test`；`web/tests/*.mjs` 是构建页面的 Playwright 回归，API 使用隔离夹具。
- 仓库工具回归位于 `tests/test_*.py`、`tools/test-*.py` 和 `scripts/test-*.py`；具体入口见脚本导航，避免无差别执行实机验收驱动。
- 修改前端后先生成 `web/dist`，再编译/测试嵌入它的面板。完整本地检查命令见[开发指南](dev.md)。
- 使用文档放在 `docs/`，新的设计决策追加 `docs/adr/`，验证记录放在 `docs/acceptance/` 并标明源码和未验证范围；更新相应索引。
- 部署模板及生成入口有摘要、签名和旧版本依赖；通过既有生成工具更新，不按普通重复文件删除。生产数据、真实凭据和临时日志不进入仓库。

现有脚本、安装入口和历史验收记录继续使用原路径；移动任何路径前先核对它的使用者（构建、发布、安装器清单、测试与检查器），见[工程整理 §2.3](redesign/08-engineering.md#23-包名不变下游引用不受影响承诺过强)。
