# 仓库目录与维护约定

本页说明现有代码与文档的位置；目标结构与迁移阶段见[功能划分与逻辑结构](architecture.md)（ADR 0128），逐项功能的领域见[领域归属表](domain-map.md)。使用方法从[文档导航](README.md)进入，架构和范围以 [AGENTS.md](../AGENTS.md) 与[各项 ADR](adr/README.md) 为准。

## 顶层目录

| 路径 | 职责 |
| --- | --- |
| `crates/` | Rust 工作区：公共协议、面板平台、领域 crate、面板应用、Agent、适配器及原生 TCP 工具 |
| `plugins/` | 尚未拆分的 sing-box 与阿里云 crate（阶段 C、D 拆回各领域）、诊断工具的面板代码与包装脚本、固定来源材料；不再新增插件 |
| `web/src/` | React 界面（中文为源语言，英文为翻译，[ADR 0127](adr/0127-chinese-source-language.md)）；前端依赖与构建由 Bun 管理 |
| `web/dist/` | 仅在本地或构建环境生成、由面板嵌入的产物；不提交 Git，不手工编辑 |
| `deploy/` | Compose、服务定义、安装模板、生成后的独立 bootstrap 入口和发布公钥 |
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
| `panel-host` | 平台：HTTP 外壳、数据库、Agent 通道、凭据库、通知发送、任务基础设施与平台钩子；工作区依赖仅 `protocol`，不依赖任何领域。尚未迁出的治理、资源、观测、作业、基础网络代码也在这里，已逐个路径登记所属领域（阶段 B 迁出） |
| `cloud-api` | 平台的云接口客户端：签名、传输、重试与错误码；工作区依赖仅 `protocol` |
| `naming` | 名称与证书领域：DNS 账号与记录、DDNS、DNS-01 验证记录 |
| `costs` | 成本领域：付费服务、付款、续费提醒与续费日历 |
| `plugins/singbox/panel`、`plugins/alicloud/panel` | 尚未拆分的 crate：前者装着流量服务与服务权益（阶段 C 拆开），后者装着云资源、安全组、CDT 计费流量与启停、换 IP 作业（阶段 D 分回各领域）；拆分前保持现有依赖。诊断工具（IPQuality、NodeQuality、TcpQuality）的面板代码暂由宿主经 `#[path]` 编入，不是独立 crate |
| `panel` | 应用：组合平台与各领域、接线依赖倒置接口、启动服务与后台任务；保存唯一的当前数据库初始化 SQL，保留原有 `apsp_panel::*` Rust 路径 |
| `agent-core` | 设备身份、传输、对账、持久状态、遥测、计量与制品；工作区依赖仅 `protocol`、`adapter-sdk` |
| `adapter-sdk` | 无状态适配器接口与共享模型 |
| `adapter-singbox`、`adapter-nodequality`、`adapter-ipquality`、`adapter-tcpquality` | 运行时或工具翻译；工作区依赖仅 `adapter-sdk` |
| `agent` | 注册具体适配器、启动 core 的二进制入口 |
| `tcp-probe` | 原生 TCP 检测工具 |

领域 crate 只依赖平台、`protocol` 与依赖顺序中排在前面的领域（[功能划分与逻辑结构 §5](architecture.md#5-依赖与写入规则)）。服务器网卡总流量属于 Agent core 与观测；代理用户、授权、订阅、套餐与代理流量属于服务权益（现在在 sing-box crate 中）。`agent-core` 与平台 crate 都不引入运行时名称或代理业务类型。分层检查入口为 `tools/check-core-boundary.py`：检查 Agent core 与平台 crate 的禁用词，以及每个 crate 的层级与 `Cargo.toml` 依赖方向；新增 crate 先在其中登记层级。

## 面板入口与领域接线

面板分为平台、领域和应用三层（[ADR 0128](adr/0128-domain-architecture.md)）。`crates/panel-host/src/lib.rs` 声明平台模块与尚未迁出的领域代码；`state.rs` 创建共享状态；`shell.rs` 是 HTTP 外壳（健康检查、前端兜底与全局请求体限制）；`agent_api.rs` 是 Agent 通道（传输、设备认证、会话与在线状态、协议分派）；`ports.rs` 提供类型化接口的安装机制。`crates/panel/src/` 是应用层：`lib.rs` 组合各领域的路由、套上治理的权限守卫与专属地址网关，并再导出平台模块、保留原有公共路径；`wiring.rs` 在启动时接线全部类型化接口；`maintenance.rs` 运行 30 秒维护循环；`plugins.rs` 登记领域 crate 的路由与后台任务；`main.rs` 启动服务。`passkeys/` 提供共用 WebAuthn 验证、挑战和凭据存储，管理员包装在 `auth/`，代理用户入口、邀请和独立会话在 `plugins/singbox/panel/portal/`。

宿主里各领域的路由由该领域在 `crates/panel-host/src/domains/` 声明：

| 文件 | 注册的接口 |
| --- | --- |
| `domains/governance.rs` | 登录/TOTP/管理员 Passkey、面板设置、通知渠道与 Webhook 设置、管理与安全、制品目录与导入、公开安装入口、Agent 更新与制品下载 |
| `domains/resources.rs` | 服务器、接入令牌、Agent 接入与退役回执 |
| `domains/traffic.rs` | 运行时检查点、设备的清单与配置包、运行时校验 |
| `domains/observability.rs` | 统计、告警事件与规则、网络检测、看板、遥测与历史、流量矫正、周期拨测与延迟检测、IP 查询与共用诊断服务、Agent 遥测与拨测与诊断上报 |
| `domains/operations.rs` | 运维（fleet）、自动化与维护、远程命令与 Agent 的命令通道、运行时操作 |
| `domains/networking.rs` | 网络配置 |
| `domains/costs.rs` | 汇率 |

路由文件只组合处理函数，鉴权和业务校验仍由原处理函数负责。`apsp_panel::router`、`AppState`、`AgentConnection`、`apsp_panel::agent_api` 及旧代理业务 Rust 导出继续可用。新增接口放进所属领域的声明，不加到平台。

领域与平台的接线：

- 领域之间需要逆着依赖顺序调用，或调用方还在宿主里时，由调用方声明类型化接口、实现方回答、应用层接线，清单见[功能划分与逻辑结构 §5.5](architecture.md#55-类型化接口)。Agent 通道把设备回报经 `DeviceReports` 交给各领域，不再有插件接口。
- 领域路由由应用层组合后放进平台的外壳；各领域的后台任务由应用层分别守护，一个崩溃不会停掉其他任务。
- `crates/naming/`、`crates/costs/`、`plugins/singbox/panel/`、`plugins/alicloud/panel/` 各自是独立 crate，只经平台公开接口使用面板能力；`crates/cloud-api/` 是平台的云接口客户端。
- 诊断代码（IP 质量、NodeQuality、TCP 质量）暂时仍由宿主的 `diagnostic_plugins.rs` 通过 path 编入，共用诊断服务继续负责生命周期、预算、取消和历史。它们与宿主的 IP 质量和诊断类型互相引用，阶段 B2 随观测的代码一起迁出宿主并去掉 `#[path]`。

数据库只有一份当前基线 `crates/panel/migrations/0001_current.sql`（[ADR 0122](adr/0122-unreleased-schema-baseline.md)）；平台通过 `sqlx::migrate!("../panel/migrations")` 嵌入它，领域 crate 的数据库测试也指向它（如 `crates/naming` 的 `#[sqlx::test(migrations = "../panel/migrations")]`）。修改基线是高风险改动，必须附带 `scripts/baseline-adoption-*.sql`，写法与部署验证见[数据库基线](database-baseline.md)「生产安装」；整理 SQL 而生成的基线逐字节不变时不算基线变更。

## 前端入口

| 路径 | 职责 |
| --- | --- |
| `web/src/App.tsx` | 会话读取、失效处理、公开看板访问控制与顶层页面组合 |
| `web/src/app/routes.ts` | 解析 URL，复用看板/插件的严格路径解析及旧链接兼容 |
| `web/src/app/navigation.ts`、`tiers.ts` | 后台侧栏的分组、入口名称、当前页面归属与功能分级标记。现有界面是五个分组，目标是 ADR 0126 的六个工作区，规则与对照见[功能地图](product-map.md) |
| `web/src/app/AdminPage.tsx` | 按解析结果选择页面，保留服务器筛选和组件重挂载边界 |
| `web/src/app/AdminShell.tsx`、`Login.tsx` | 后台布局与登录表单 |
| `web/src/display/`、`statistics/` | 总览：服务器看板与统计报表 |
| `web/src/servers/` | 服务器列表、接入与编辑、服务器详情分页（概况、资产与流量、Agent 与任务、插件、线路诊断、运维）及其组成部分 |
| `web/src/server-diagnostics/` | 服务器详情「线路诊断」中的 IP 信息、TCP 诊断与节点验机 |
| `web/src/fleet/`、`operations/` | 服务器详情「运维」与批量接入；实验功能「自动化与维护」 |
| `web/src/monitoring/` | 告警事件、延迟检测、告警设置及其规则、Webhook 和渠道状态 |
| `web/src/network-workbench/`、`network-configuration/` | 监控告警「网络检测」；实验功能「网络配置」 |
| `web/src/plugins/` | 插件中心（目录与 sing-box 安装）及 sing-box、DDNS、阿里云、费用界面（阶段 E 按领域拆开，现在每个文件已登记所属领域） |
| `web/src/system/`、`control-center/` | 面板设置（含功能显示）、备份与恢复、账号安全；管理与安全及全局搜索、外观偏好 |
| `web/src/api.ts`、`hooks.ts`、`components.tsx` | 共享请求、状态钩子和通用组件（页头、通知、弹窗、确认框、字段、复制） |
| `web/src/index.css` | Tailwind CSS v4 入口与全部设计参数：浅色/暗色颜色、圆角、密度、基础样式 |
| `web/src/components/ui/` | shadcn/ui Nova 组件源码（Radix UI 底层），按普通源码维护 |
| `web/src/components/` | 面板、分段导航、主从分栏（`list-detail.tsx`）、拖拽列表（`sortable.tsx`）与行内“更多操作”菜单（`action-menu.tsx`） |
| `web/src/lib/` | `cn()`、Motion 时长与预设、媒体查询及本地存储工具 |
| `web/src/stores/` | Zustand 跨组件界面状态：语言、外观、外框、会话与看板货币 |
| `web/src/i18n.tsx` | 语言上下文、浏览器语言选择、管理员偏好同步及静态界面词典 |

前端按领域目录组织（[ADR 0128](adr/0128-domain-architecture.md)、[ADR 0126](adr/0126-workspaces-language-and-addresses.md)）：每个入口属于一个领域，新增页面放进它所属领域的目录；侧栏分组由清单声明，调整分组不搬目录；同一对象允许多个入口，但只有一套模型和编辑逻辑。目标目录结构（`web/src/<领域标识>/`、`app/` 与平台的 `components/`、`lib/`、`stores/`）见[功能划分与逻辑结构 §5.4](architecture.md#54-代码结构)；迁移前，现有目录的每个文件已在 `tools/feature-boundaries.toml` 登记所属领域，跨领域共用的数据模型暂留在 `web/src/` 根目录（如 `probes.ts`、`server-assets.ts`）。页面标题与入口同名，不加眉题，说明文字先讲“在这里做什么”。

界面约定见 [ADR 0088](adr/0088-ui-component-system.md)：组件使用 `components/ui/` 中的 shadcn/ui Nova 组件，样式只写 Tailwind 类并引用 `index.css` 的设计变量，不再新增 CSS 文件或颜色字面量；图标来自 `lucide-react`，图标按钮必须有 `aria-label`；动画只用 `motion/react` 的 `m.*` 与 `lib/motion.ts` 预设，需要阅读的内容进入时只做位移、不得从透明或隐藏开始；可调分栏通过 `components/ui/resizable.tsx`，拖拽排序通过 `components/sortable.tsx`；跨组件界面状态放入 `stores/`，服务端数据仍经 `useResource` 与写保护。分区导航用 `SegmentedNav`，按钮通过 `aria-pressed`、页面链接通过 `aria-current="page"` 标记当前项；测试钩子使用 `data-slot` 与 `data-dialog`。首次加载用 `Loading` 的 `table`、`cards`、`panel` 形态显示与内容一致的骨架；开关类设置行用 `SettingRow`；用量条用 `Meter`（默认 75% 警告、90% 危险）；页面跳转与对象搜索集中在 `control-center/CommandPalette.tsx`，可跳转页面取自侧栏并按当前账号权限过滤。行操作只直接显示常用项，其余放进 `ActionMenu`（“更多操作”）；内容丰富的宽表格用容器查询在窄面板中堆叠为卡片，仍保持同一张表。页面跳转用 `navigate()` 或普通链接，不直接写 `location.hash`；有未保存输入的表单用 `useLeaveGuard`（或 `useFormDraft`）登记，离开检查集中在 `lib/leave-guard.ts`。响应式布局只增删外围元素，不替换承载页面内容的组件树；随数据变化的状态文字用 `t()`。按条件显示的弹窗包在 `DialogPresence` 中，让关闭动画有机会执行；列表的次要筛选与视图设置放进 `ToolbarPanel`。

Hash URL 是书签与兼容入口，地址按 ADR 0126 的地址策略管理：已发给代理用户的地址永不失效，控制台地址改名后至少保留一个发布版本的改写。新增页面要同时考虑路由、导航、管理员访问边界；公开访问只放行明确的看板路由，不据路径前缀推断权限。

## 验证与文件维护

- Rust 单元测试随模块放置；HTTP/PostgreSQL、协议与编译集成测试位于对应 crate 的 `tests/`。
- `web/tests/*.test.ts` 使用 `bun test`；`web/tests/*.mjs` 是构建页面的 Playwright 回归，API 使用隔离夹具。
- 仓库工具回归位于 `tests/test_*.py`、`tools/test-*.py` 和 `scripts/test-*.py`；具体入口见脚本导航，避免无差别执行实机验收驱动。
- 修改前端后先生成 `web/dist`，再编译/测试嵌入它的面板。完整本地检查命令见[开发指南](dev.md)。
- 使用文档放在 `docs/`，新的设计决策追加 `docs/adr/`，验证记录放在 `docs/acceptance/` 并标明源码和未验证范围；更新相应索引。
- 部署模板及生成入口有摘要、签名和旧版本依赖；通过既有生成工具更新，不按普通重复文件删除。生产数据、真实凭据和临时日志不进入仓库。

现有脚本、安装入口和历史验收记录继续使用原路径；移动任何路径前先核对它的使用者（构建、发布、安装器清单、测试与检查器），见[工程整理 §2.3](redesign/08-engineering.md#23-包名不变下游引用不受影响承诺过强)。
