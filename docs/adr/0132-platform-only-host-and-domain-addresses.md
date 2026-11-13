# ADR 0132：宿主只剩平台机制，设备连接归资源，接口地址按领域，路由权限一张表

状态：已接受（2026-11-13）。完成 [ADR 0128](0128-domain-architecture.md) 迁移阶段 B2 的其余部分，调整它第 5 条中 Agent 通道的归属与类型化接口的清单；取代 [ADR 0131](0131-one-model-per-object.md) 第 4 条的“接口前缀”一条。

2026-11-13 用户要求：“查看 apsp 根据仓库的内容，继续重构”，“激进一点修改，删掉重复的内容，用底层统一，不再使用之前的 plugin 模式，扫描”。随要求给出的归属表：

| 对象 | 领域 | 职责 |
| --- | --- | --- |
| 服务器、Agent 接入 | `resources` | 资源身份、设备连接 |
| 节点、链路、运行时配置 | `traffic` | 网络服务的定义、编译和部署 |
| 用户、策略组、套餐、用量 | `access` | 谁可以使用、额度与期限 |
| DNS、证书 | `naming` | 名称与证书管理 |
| 指标、检测、告警 | `observability` | 当前发生了什么 |
| 命令、升级、执行任务 | `operations` | 执行变更并记录结果 |

## 背景

2026-11-13 的 main（`91059f8c`）里，与这张表和“不再有插件”不一致的有四类：

1. **宿主还装着四个领域的代码。** `crates/panel-host` 约 2.0 万行，除平台机制外还有治理（登录、Passkey、管理与安全、制品与版本、备份）、资源（服务器、接入、退役、生命周期）、基础网络（网络配置）与名称的 ACME 和证书。它们只是在边界清单里逐个路径登记了所有者，依赖方向与可见性都管不到同一个 crate 里的代码。设备通道与设备认证算作平台，接入令牌的表登记在治理，备份的表名是 `operations_backup_*` 却登记在治理。
2. **还留着一个插件式的钩子。** 治理声明的 `Contributions` 一个接口六个方法（路由权限、审计脱敏键、审计快照、搜索类别、后台任务名、签名工具包），由应用层的 `integration_policy.rs` 与流量服务的 `host_support.rs` 回答，路由的权限规则因此分在治理的守卫与应用层两处。应用层自己也分两处组合：`lib.rs` 与 `domains.rs` 各拼一部分路由，代理控制台的路由靠前缀嵌套得到完整路径；后台任务一部分在 `main.rs` 登记，一部分在 `domains.rs`。
3. **控制台接口地址与内部名称仍按插件命名。** `/api/plugins/{sing-box,ddns,expenses,tcpquality,alicloud}/…` 共 185 条路由；代码里还有 `diagnostic_plugins/`、`notifications/plugin.rs`、`PluginServer` 等名称。
4. **同一个机制有几份实现。** JSON 内容摘要 7 份，“普通服务器地址”的判断 2 份，证书名称的通配符覆盖规则 2 份，安装脚本地址的生成 2 份。

## 决策

### 1. 这张表怎样读

- **九个领域不变。** 表列出的是六类主要对象，不重新分配表里没有提到的平台治理、基础网络与成本，它们保持 ADR 0128 的对象。
- **代码与表不一致的地方，以表为准。** 由此改变归属的有三处：设备连接（原来算平台）与接入令牌（原来登记在治理）归资源；Agent 的自更新检查与面板备份的计划、执行和记录（原来登记在治理）归作业。
- **这一步不改外部契约与数据库。** 数据库基线、Agent 协议、设备上的路径、签名制品里的名字与已保存的标识都不动（见第 9 条）。

### 2. 宿主只剩平台机制

`crates/panel-host` 只放技术机制：配置（`config`）、HTTP 外壳与内嵌前端（`shell`、`frontend`）、基线核对（`schema`）、共享状态与各领域运行时状态的槽（`state`）、类型化接口的机制（`ports`）、向设备派发任务与接收观测证据的两个接口（`device_work`、`evidence`）、后台任务守护（`maintenance`）、设置存储（`settings`）、通知的渠道、模板、投递、重试与发件箱（`notifications`）、令牌、文本与摘要检查（`tokens`、`text`、`digest`）、地址与名称的分类（`net`）、X.509 读取（`x509`）、错误类型（`error`）。`AppState` 只有连接池、启动时间、配置、三个进程级的并发许可、编译进来的发布信任根与 `domain_states`；它不再创建管理员，也不再持有 Passkey 服务、设备连接表与设备生命周期锁。`crates/panel-host/src/domains/` 删除，Cargo 依赖减到平台自己用到的。

理由：平台被每个领域依赖，它认识任何一个领域都会形成环；领域代码留在平台 crate 里时，“平台不依赖领域”只能靠清单记账，编译器与层级检查都看不见。

### 3. 治理是排在最前的领域 crate

新建 `crates/governance`（`apsp-governance`）：登录与会话、TOTP 与 Passkey（`auth`、`passkeys`），管理员与权限、请求守卫与审计、受管凭据、面板设置、系统健康与工具安全（`control_center`），签名发布与制品目录（`releases`、`artifacts`）。`apsp_governance::state(pool, config)` 在平台状态之上准备治理自己的部分（第一个管理员、Passkey 服务）。

理由：每个领域的处理函数都要用治理的权限判断与再次验证，所以治理排在所有领域之前，只依赖平台。它需要后面领域的数据时声明接口（第 6 条的审计快照），不反过来引用。

### 4. 设备连接、设备认证与设备下载制品归资源；Agent 自更新检查与面板备份归作业

资源（表的第一行“设备连接”）：

- `crates/resources/src/device_channel.rs` 是原来平台的 `agent_api.rs`：WebSocket 认证、设备会话、在线状态、在线连接的登记、消息分派与向设备发通知；`DeviceReports` 接口改由资源声明，仍由应用层分派给各所有者；设备请求的身份检查 `require_agent` 与要求设备支持验签的 `require_signed_agent` 也在这里。
- `device_artifacts.rs` 是设备下载制品的接口 `/api/agent/v1/artifacts/{name}/{version}/{arch}`；`bootstrap.rs` 是安装中的机器凭接入令牌询问的内容（脚本与可选版本）。
- 表 `enrollment_tokens` 登记给资源。
- 宿主里其余的资源代码一并迁入：服务器、资产、退役、安装、批量接入、生命周期、服务器观测接口、云资源关联与 Hetzner。

理由：设备的密钥绑定在服务器记录上，设备会话、在线状态和连接与退役的先后次序都是“服务器”这个对象的状态；通道在认证时读写 `servers`，留在平台就是平台读写领域的表。消息的定义仍在 `protocol`，向设备派发任务的接口 `device_work` 仍由平台声明。

作业（表的最后一行“命令、升级、执行任务”）：

- `crates/operations/src/agent_updates.rs`：Agent 的自更新检查 `/api/agent/v1/update`。哪些版本可信可用仍由治理的发布目录回答。
- `backups.rs`、`backups/executor.rs`、`backups/executor/connection.rs`、`backups/recovery.rs`：面板备份的计划、执行、导入的备份与恢复演练，接口 `/api/operations/backups*` 与 `/api/operations/backup-schedules*`；表 `operations_backup_*` 与 `operations_restore_drills` 登记给作业。

理由：备份的表名、接口地址与心跳名本来就写的是作业，作业里的面板备份步骤早已直接调用它；登记在治理时，作业与治理互相读写对方的表。[功能划分与逻辑结构 §4](../architecture.md#4-典型交叉的归属)原来就写着恢复与升级“执行部分都由作业完成”。谁可以备份与恢复仍由治理的权限与再次验证决定。

### 5. 基础网络与名称是先后两组；X.509 读取是平台机制

- 网络配置迁入 `crates/networking`；ACME、证书、证书部署与 DNS-01 验证迁入 `crates/naming`。证书与域名的文档仍存放在基础网络的文档存储里，名称直接读它，所以依赖顺序里名称排在基础网络之后（原来两者同组），为同组而设的 `Dns01` 接口删除。
- `apsp_panel_host::x509` 读取 PEM 证书链（指纹、有效期、名称）并判断名称是否被覆盖（`covers`）。它是纯解析，名称、基础网络与流量服务都要用：放进其中任何一个领域，排在它前面的领域就得逆着顺序引用。流量服务部署预检里自带的那份通配符覆盖规则删除。

### 6. `Contributions` 拆掉，不再有钩子

| 原来的钩子 | 现在 |
| --- | --- |
| 路由权限 | 治理请求守卫里的一张表（第 7 条），应用层不再有任何权限规则 |
| 审计脱敏键 | 守卫 `redact` 里的一份清单（原治理清单与代理运行时凭据字段名的并集） |
| 审计的变更前后快照 | 接口 `guard::AuditSnapshots`（一个方法），治理声明，资源的 `servers::audit_snapshot` 与流量服务的 `nodes::audit_snapshot` 回答，在 `wiring.rs` 接线；治理不再自己读 `servers` |
| 搜索类别“节点”“代理用户” | 写在应用层的搜索里，与其他类别一样；搜索本身（原 `control_center/search.rs`）联结几个所有者的表，是应用层的组合视图 `crates/panel/src/search.rs` |
| 后台任务名 | 应用层随它启动的任务一起声明（`domains.rs` 的 `HEARTBEATS`），经 `control_center::system::expect_workers` 交给健康视图 |
| 签名工具包的上游包 | 治理 `tool_security.rs` 里的一张表 |

应用层的 `integration_policy.rs` 与流量服务的 `host_support.rs` 随之删除。

理由：六件事里只有审计快照确实要逆着依赖顺序取数据，按 ADR 0128 的规则由调用方声明一个接口；其余五件是静态的知识，写成治理或应用层里的一张表，比让各领域向一个钩子登记更容易审查，也不再需要“谁来回答”的约定。

### 7. 路由权限一张表，由审阅过的结论表守住

- 路由的权限规则只在 `crates/governance/src/control_center/guard.rs`：按地址判断所属能力与范围的几个函数（`proxy_console`、`names`、`alicloud`、`expenses`、`independent_identity`、`feature`、`guarded_change`、`reads_only`、`any_scope`）。
- `guard::describe(path, method)` 给出守卫对一个路由的结论：谁可以调用、服务器范围、是否要求近期的再次验证。`crates/panel/tests/fixtures/route-policy.tsv` 按“方法、路由、结论”一行一条记录，Rust 测试 `crates/panel/tests/route_policy.rs` 逐行重新求结论并与表比对；`tools/route-policy.py` 比对表里的路由清单与源码登记的路由（`--write` 增删行，`APSP_BLESS_ROUTE_POLICY=1` 重新填入结论），`tests/test_route_policy.py` 测这个工具。
- 这张表先由改动前的守卫与钩子生成（499 行），重构后的守卫必须逐行得出相同的结论。工具起初不认 `use axum::routing::{get as route_get}` 这样的别名，外部来源的 6 条 GET 路由因此没有进表；补上别名识别后表共 505 行，补入的 6 行按同一组规则得出结论（`proxy:read`，与改动前对这些地址的判断相同）。

理由：权限是治理的规则，放在一处才能通读与审计；按领域分散声明等于再做一个登记钩子。规则合并后，任何一条规则的改动都会在表里显示为逐条路由的差异，由人审阅后才接受。

### 8. 应用层一处组合，不嵌套前缀

- `crates/panel/src/domains.rs` 有一张路由清单与一张后台任务清单。每个领域 crate 只导出一个 `routes()`，应用层按依赖顺序合并，再加上两个跨领域的组合视图（云页面的总览 `cloud_overview.rs` 与搜索 `search.rs`）；全部后台任务由 `domains::run` 分别守护。
- 不再使用 `.nest(...)`：每条路由的完整路径写在声明它的地方，能用文本检索找到，第 7 条的工具也因此能从源码读出完整的路由清单。
- `main.rs` 只建立状态、路由与后台任务；`apsp_panel::state(pool, config)` 给出接口已接线、治理已准备好、在线设备的登记已建立的状态，测试也用它。

### 9. 控制台接口地址按对象所属的领域

`/api/plugins/<插件>/…` 不再登记为路由。地址的第一段是对象所属的领域，按前缀对应，其后的路径、查询参数与路由参数原样保留：

| 原地址 | 现地址 |
| --- | --- |
| `/api/plugins/sing-box/` 下的 `chains`、`nexus-nodes`、`node-catalog`、`nodes`、`ordered-proxy-resources`、`ordered-subscription-source-jobs`、`ordered-subscription-source-previews`、`ordered-subscription-sources`、`runtime-inventory`、`runtime-rollouts`、`servers` | `/api/traffic/…` |
| `/api/plugins/sing-box/` 下的 `account-mail`、`external-node-candidates`、`invitation-settings`、`operations`、`package-groups`、`policy-groups`、`portal`、`portal-guides`、`portal-public`、`subscription-domain`、`usage`、`users` | `/api/access/…` |
| `/api/plugins/sing-box/reachability…` | `/api/observability/reachability…` |
| `/api/plugins/sing-box/statistics` | `/api/access/statistics` |
| `/api/plugins/ddns…` | `/api/naming…` |
| `/api/plugins/expenses…` | `/api/costs…` |
| `/api/plugins/tcpquality…` | `/api/observability/tcp-quality…` |
| `/api/plugins/alicloud…` | `/api/resources/alicloud…` |

- **地址说的是对象属于谁，不是处理函数在哪个 crate。** 与 `/api/servers/{id}/commands` 由作业实现一样：阿里云的云账号与云资源是资源的对象，整组接口放在 `/api/resources/alicloud`，其中的 26 条路由由资源（6）、作业（18）、成本（1）与应用层的总览（1）实现；`/api/access/statistics` 仍由观测的处理函数回答。
- **旧地址在一个发布版本内照常应答。** 请求在匹配路由、进入专属地址网关与权限守卫之前改写到现地址（`crates/panel/src/moved_paths.rs`，集成测试 `crates/panel/tests/moved_api_paths.rs`），让升级前已经打开的页面（含成员门户）与运维脚本继续可用；2026-11-13 之后的下一个发布版本后删除这段改写。例外是续费日历的订阅地址 `/api/plugins/expenses/calendar.ics`：它登记为外部契约（管理员的日历程序按这个地址订阅），旧地址长期保留，不随其余改写一起删除。此后写入的审计记录带现地址，旧记录保持原样。
- **不改名的：** Agent 协议的 `/api/agent/v1/*`、客户端订阅 `/sub/{token}` 与其余 `/api/...` 地址。数据库基线（含表名 `server_plugins` 与 `singbox_*`）、Agent 协议（`DiagnosticJob.plugin` 等消息字段）、设备上的路径（`/opt/apsp/plugins`、`/var/lib/apsp/plugins`）、签名制品里的名字（`plugins/...`）、已保存的标识（后台任务心跳名 `sing-box`、`ddns`、`alicloud`、`expenses`，诊断任务里的 `plugin` 键）与接口响应里的 `plugin_*` 字段都不动：基线、协议与制品格式各有自己的变更流程，设备上的路径与协议字段只随 Agent 发布改变；已保存的标识在控制台里显示为各自的名称；响应字段要服务端与前端一起改。清单见[功能划分与逻辑结构 §12](../architecture.md#12-扫描结论与余下工作2026-11-13)。

这一条取代 ADR 0131 第 4 条的“接口前缀”。那一条把改名推迟到兼容桥退出之后，理由是在那之前还有仓库外的调用方。逐个核对后这个理由不成立：兼容桥只经数据库函数与 apsp 交换数据，不调用 HTTP 接口（ADR 0131 的背景已经写明）；设备只用 `/api/agent/v1/*`，订阅客户端只用 `/sub/{token}` 与专属地址，安装脚本只用 `/install.sh`、`/install.ps1` 与 `/api/bootstrap/*`，都不带这个前缀。带这个前缀的调用方是控制台与成员门户的页面（与接口同一版本发布）以及仓库里的运维脚本与测试（一起改了）；升级前已打开的页面，和可能存在的、持管理员会话或管理 API 令牌的仓库外脚本，由一个发布版本的改写覆盖；按旧地址订阅续费日历的日历程序由长期保留的那一条覆盖。已发给代理用户与客户端的地址（订阅、专属地址、门户与邀请链接）不在这个前缀下，没有变化。

### 10. 内部名称与重复实现

- 名称不再带“插件”：观测的 `diagnostic_plugins/` 改为 `diagnostic_tools/`（`DiagnosticPlugin` 改为 `DiagnosticTool`，各工具的 `…Plugin` 改为 `…Tool`，`PluginReadiness`、`PLUGIN_VERSION` 改为 `ToolReadiness`、`TOOL_VERSION`）；平台的 `notifications/plugin.rs` 改为 `notifications/delivery.rs`；流量服务的 `PluginServer` 改为 `RuntimeServer`；前端的 `PluginServer`、`PluginInstallation` 改为 `RuntimeServer`、`RuntimeInstallation`，界面上四处“插件”字样去掉。
- 行为相同的重复实现各合为一处：JSON 内容摘要（`apsp_panel_host::digest::json`）、普通服务器地址的判断（`net::server_ip`）、证书名称与网络文档的域名检查（`net::dns_name`）、安装脚本地址（`installation::blob_url`）、ISO 时间格式（`apsp_cloud_api::signing::iso_time`）。只有一个调用方的函数移到调用方：锁定用户的两个函数从流量服务移到服务权益；`servers.agent_settings` 由资源的 `servers::save_agent_settings` 写入（观测只做校验）；作业判断管理员能否看见全部服务器改用治理的 `control_center::actor_all_servers`。前端的统计条、请求编号、资源快照、日期文本与操作执行器各合为一个共用模块。
- 合并会改变行为的重复实现没有在这一步合并（两个 Globalping 客户端、三处 Cloudflare 接口、几个通知队列等），清单与取舍见[功能划分与逻辑结构 §12](../architecture.md#12-扫描结论与余下工作2026-11-13)。

## 取代与调整

- [ADR 0128](0128-domain-architecture.md)：迁移阶段“B2 其余领域代码迁出宿主”完成。第 5 条把“Agent 通道与消息”列为平台机制，现在设备通道属于资源，消息定义仍在 `protocol`。依赖顺序中基础网络与名称由同组改为先后两组。类型化接口的清单去掉 `Contributions` 与 `Dns01`，增加 `guard::AuditSnapshots`，`DeviceReports` 改由资源声明（现行清单见[功能划分与逻辑结构 §5.5](../architecture.md#55-类型化接口)）。治理的对象不再包括接入令牌与备份的执行和记录。
- [ADR 0131](0131-one-model-per-object.md) 第 4 条“接口前缀”：由本决策第 9 条取代，其中“设备上的路径不改”继续有效。其余各条不变。
- [ADR 0126](0126-workspaces-language-and-addresses.md) 的地址策略（控制台地址改名后旧地址改写到新地址并至少保留一个发布版本）原来针对页面地址，本决策对控制台接口地址采用同样的做法。

## 后果

- 宿主从约 2.0 万行减到 3,066 行；新的治理 crate 约 6,600 行，资源 crate 6,333 行。
- 越界清单从 458 条减到 392 条（SQL 298、前端引用 94，代码引用与 `#[path]` 为零）：基础网络与名称迁出之后是 432 条，其余改动之后是 392 条。后一段里 44 条消失，4 条因为代码或表换了所有者而新记：治理的系统健康视图读作业的三张备份表（3 条），资源的设备通道读治理的 `sessions`（1 条，管理员会话与设备会话仍共用一张表）。前端跨领域引用从 112 条减到 94 条。
- 平台 crate 读领域表的 SQL 还剩 5 条，都在告警发件箱（`notifications/outbox.rs` 4 条、`settings.rs` 1 条）：发件箱实际是观测的告警投递队列，迁出需要新的接口，没有在这一步做。
- 边界清单的代码部分变为每个所有者一个 crate 一行，不再有宿主里逐个文件的登记；层级检查把 `crates/governance` 登记为第一个领域 crate，宿主的例外表达式只剩 `src/config.rs` 一条。
- 新增或改动路由要同步路由权限表：先运行 `python3 tools/route-policy.py`，规则见 [AGENTS.md](../../AGENTS.md)「分层」与「实现与检查」。
- 这一步有意没有做的归属调整、重复实现与仍带“插件”字样的契约，连同各自需要的条件，列在[功能划分与逻辑结构 §12](../architecture.md#12-扫描结论与余下工作2026-11-13)。
