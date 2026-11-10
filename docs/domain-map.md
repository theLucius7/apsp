# 领域归属表

- 状态：现行（[ADR 0128](adr/0128-domain-architecture.md)，2026-11-08）。领域的定义、依赖顺序与写入规则见[功能划分与逻辑结构](architecture.md)；每张表与每个源码路径的所有者见 `tools/feature-boundaries.toml`。
- 用法：新增或修改功能前，先在这里找到它的领域；没有的，按“改变哪个对象、维护哪条规则”补一行，再动代码。权威领域就是小节标题；“协作”列只写需要调用的其他领域；“实现与入口”写现在的代码位置与界面位置，迁移中的写明目标。

## 资源与设备 `resources`

| 功能 | 意图 | 核心对象 | 关键约束 | 协作 | 实现与入口 |
| --- | --- | --- | --- | --- | --- |
| 服务器接入 | 把一台机器纳入管理并能执行任务 | 服务器、Agent 绑定 | 接入令牌一次性；资源身份独立于 IP 与名称 | 治理（接入令牌）；流量服务（安装运行时） | 宿主 `servers.rs`、`installation*`、`batch_enrollment.rs`；Agent `enroll`；服务器列表 › 添加服务器、批量接入（实验） |
| 服务器资产 | 记录地区、标签、到期与采购配置 | 服务器资产 | 一份资产记录（运维区的采购配置并入，路线图 4.4） | 成本（只读到期与成本） | 宿主 `server_assets.rs`、`server_lifecycle.rs`（资产档案、本机运维策略、生命周期与迁移，表 `fleet_profiles`）；服务器详情 › 资产与流量、运维 |
| 服务器退役 | 安全删除一台服务器 | 退役记录 | 在线退役以设备签名回执为准；离线退役单独标明 | 流量服务（停止服务）；作业（执行） | 宿主 `retirement.rs` |
| Agent 能力 | 知道每台 Agent 能做什么 | Agent 能力与绑定代次 | 迟到的回执不能覆盖新绑定代次 | 治理（版本策略） | 宿主 `agent_api.rs` 的能力登记 |
| 云实例与 EIP | 知道云上有哪些实例与 EIP 及其状态 | 云资源、云账号关联 | 只经官方接口读回确认 | 成本（账单）；基础网络（安全组） | `crates/resources` 的 `cloud/`（`api`、`client`、`inventory`、`addresses`、`model`）；`operations_cloud_links`；云账号 `alicloud_accounts`（其中的账单与额度字段由成本写入，阶段 F 拆表）；服务器 › 云资源 |
| 云实例启停规则 | 判断一台实例此刻能否开关机 | 实例电源状态 | 关机前确认不影响在用服务与保护策略 | 作业（执行与计划）；成本（额度触发） | 资源的 `cloud/model.rs` 中的 `Policy`（规则）与 `State`（电源状态）；按规则判断与读取电源状态的代码在作业的 `cloud/power/`（`PolicyRules`、`StateRules`），与执行在一起 |
| 资源替换 | 用新机器替换旧机器并保留关联 | 替换关系 | 显式替换，不凭相同 IP 或名称合并 | 名称；流量服务；成本 | 网络配置的服务器替换（实验）；`network_server_migration_previews` |
| Hetzner 资源（冻结） | 查看 Hetzner 资源 | 云资源 | 只读 | 成本 | 宿主 `cloud_resources/hetzner`（云资源关联与账单观测在 `cloud_resources.rs`） |

## 基础网络 `networking`

| 功能 | 意图 | 核心对象 | 关键约束 | 协作 | 实现与入口 |
| --- | --- | --- | --- | --- | --- |
| 主机网络参数、受管防火墙、私有组网、反向隧道（冻结） | 改变服务器的网络条件 | 主机网络规则 | 作用域明确、可回滚；需要确认 | 作业（执行）；观测（验证） | 宿主 `network_configuration/`；Agent `system_network`；服务器 › 网络配置 |
| 端口转发（实验） | 把一个端口的流量转到别处 | 转发规则 | 不与服务端点冲突 | 流量服务（计划重做为中转规则后归流量服务） | Agent `system_forwarding`；服务器 › 网络配置 |
| 端点台账（实验） | 记录服务器的网络端点 | 端点台账 | 只是台账，不代替服务端点 | 流量服务 | `network_documents` |
| 云安全组（冻结） | 管理云上的入站规则 | 安全组规则 | 保留云厂商的作用域语义 | 资源；作业（执行） | `crates/networking` 的 `security_groups/` |

## 流量服务 `traffic`

| 功能 | 意图 | 核心对象 | 关键约束 | 协作 | 实现与入口 |
| --- | --- | --- | --- | --- | --- |
| 运行时安装与升级 | 让服务器运行 sing-box | 服务实例、运行时版本 | 只用已导入的签名制品；安装完成以 Agent 回报为准 | 资源；治理（制品可信）；作业（升级执行） | `crates/traffic` `settings.rs`（启用记录 `server_plugins`）、运行时升级记录；服务器详情 › 网络服务 |
| 受管节点 | 开一个代理入口 | 受管节点（服务端点） | 端口不冲突；被链路引用时锁定部分配置；没有授权用户时不监听 | 名称（公开地址、证书）；服务权益（加入策略组） | `crates/traffic` `nodes*`；网络服务 › 节点 |
| 链路 | 让流量按顺序经过多个端点（唯一的链路模型，ADR 0131） | 链路与不可变版本 | 只引用存在的端点；线性无环；入口单次计量；内部凭据不进订阅 | 外部来源 | `crates/traffic` `ordered_paths/`；网络服务 › 链路 |
| 外部来源与外部节点 | 引入机场等外部节点 | 外部来源、外部节点版本 | 先预览再采用；失败保留上一个成功修订 | 服务权益（策略组引用）；成本（报告的到期只读） | `crates/traffic` `subscription_sources/`、`subscription_parser/`（唯一的来源模型，ADR 0131）；网络服务 › 外部来源 |
| 配置编译与发布 | 把期望配置变成每台服务器的完整快照并下发 | 配置版本、部署 | 同一输入同一字节；字节相同不产生新修订；只有回执才算已应用 | 服务权益（授权凭据，经依赖倒置接口）；平台（Agent 通道） | `crates/compiler`；`crates/traffic` `publisher.rs`、`deployments.rs`、`device_config.rs`（设备的清单与配置包、已应用修订、应用结果，阶段 B2c 从宿主迁入） |
| 运行时检查点与路径验证 | 确认目标修订真的在运行、路径真的通 | 已应用状态与验证证据 | 证据有有效期；旧代次的回执不覆盖新代次 | 平台（Agent 通道） | `crates/traffic` `runtime_control/`（检查点、回执与路径探测）、`runtime_validations.rs`（阶段 B2c 从宿主迁入；混合链路退出后只为在用 Agent 保留空的轮询接口，随下一次 Agent 协议变更删除） |
| 运行时运维 | 检查、重启运行时或重试部署 | 运行时操作的合法性 | 只对受管运行时；结果未知不自动重试 | 作业（执行记录） | `crates/operations` 的 `runtime_operations.rs`（设备队列与回执）；`crates/traffic` `runtime_operations.rs` |
| 服务器端逐用户限制的执行 | 把限速与连接数编进配置 | 运行时能力 `user-limits` | 运行时不支持时拒绝而不是忽略 | 服务权益（限制的值） | `crates/compiler/src/limits.rs` |
| Nexus 运行时兼容 | 让接管的 Nexus 主机在兼容桥退出前继续用保留配置 | 保留配置、受保护节点 | 只经接管流程进入；兼容桥退出（路线图 1.5）后删除 | 治理（迁移授权）；服务权益（兼容桥） | `crates/traffic` `nexus_nodes/`、`crates/compiler/src/preserved.rs`；系统 › 迁移与兼容 |

## 服务权益与交付 `access`

| 功能 | 意图 | 核心对象 | 关键约束 | 协作 | 实现与入口 |
| --- | --- | --- | --- | --- | --- |
| 服务用户 | 给一个人开通服务 | 用户聚合（用户、凭据、限制） | 删除时撤销全部凭据，历史用量保留 | 流量服务（凭据编入配置） | `crates/access` `proxy_users.rs`、`accesses.rs`；网络服务 › 用户 |
| 策略组 | 决定用户能用哪些资源 | 策略组、成员关系 | 用户只经策略组获得资源 | 流量服务（资源目录） | `crates/access` `policies.rs`、`external_access/`；网络服务 › 策略与套餐 |
| 套餐与资格 | 约定额度、有效期与重置周期 | 套餐模板、套餐快照、资格 | 快照不可变；资格变化才触发重新发布 | 流量服务（重新发布） | `crates/access` `packages.rs`、`entitlements.rs` |
| 用量账本与周期重置 | 记账并判断还能不能用 | 用量批次与记录、额度补偿 | 批次幂等；只在入口计量；重置记为补偿，不删记录 | 平台（Agent 通道收批次） | `crates/access` `usage.rs`；用户详情 |
| 速率与连接数限制 | 给用户设上限 | 用户限制 | 0 表示不限；按服务器执行 | 流量服务（编进配置） | `crates/access` `user_limits.rs` |
| 客户端订阅 | 让用户的客户端拿到可用配置 | 订阅令牌、订阅输出 | 只包含有效权益覆盖的服务；不调用第三方转换；令牌可重置 | 流量服务（可交付服务描述） | `crates/access` `subscriptions.rs`、`crates/compiler` 的客户端部分；`/sub/{token}` |
| 专属地址 | 每个用户独立、可重置的主机名与路径 | 专属地址 | 默认关闭；令牌只进加密凭据 | 名称（DNS 记录，应经名称的接口；现在自带 Cloudflare 客户端，阶段 C 改） | `crates/access` `subscription_hosts.rs`；用户 › 门户设置 |
| 门户与登录 | 让用户自己查看订阅、用量与到期 | 门户账号、会话、Passkey | 与管理员会话分开；开通不得使用订阅令牌 | 治理（复用 WebAuthn 机制） | `crates/access` `portal/`、`user_accounts/`；`/#/account/{id}`、`/#/member` |
| 邀请 | 已有用户邀请熟人 | 邀请名额、邀请码 | 不收费、无佣金；套餐与策略组取邀请的默认值 | — | `crates/access` `user_accounts/invitations.rs`；`/#/join` |
| 公告、指南与提醒 | 告诉用户怎么用、何时到期 | 公告、指南、提醒 | 提醒只告知，不改权益 | 平台（邮件与通知通道） | `crates/access` `user_accounts/` |
| 用户操作流程 | 批量分配、换套餐、重置额度、轮换凭据 | 权益变更计划 | 先预览再应用；轮换凭据要等设备确认 | 流量服务（下发）；作业（执行记录） | `crates/access` `operations_workflows/` |
| 用户诊断 | 回答“这个用户为什么用不了” | 诊断结论（引用其他领域的证据） | 每一步写明依据与时间 | 流量服务（下发状态）；观测（线路质量） | `crates/access` `operations_workflows/diagnosis.rs`；用户详情 › 诊断 |
| Nexus 迁移用户与兼容桥 | 让迁移用户继续使用旧订阅 | 兼容桥成员、旧订阅渲染 | 有退出条件（路线图 1.4、1.5） | 流量服务（兼容运行时） | `crates/access` `nexus/bridge.rs`（兼容桥只在这里出现）与 `nexus/sessions.rs`（设备会话）；系统 › 迁移与兼容 |

## 名称与证书 `naming`

| 功能 | 意图 | 核心对象 | 关键约束 | 协作 | 实现与入口 |
| --- | --- | --- | --- | --- | --- |
| DNS 账户与记录 | 管理域名的权威记录 | DNS 账户、记录 | 一次更新只改目标记录；凭据不发给 Agent 或公开界面 | 平台（凭据库、供应商接口） | `crates/naming`（原 DDNS 插件）；服务器 › 地址与域名 |
| DDNS | 让名称持续指向选定的地址来源 | DDNS 规则 | 无有效地址或设备离线时保留解析；A 与 AAAA 是两条规则 | 资源（地址事实）；作业（执行） | `crates/naming` 的同步任务；地址与域名、服务器详情 › 地址与资源 |
| 域名验证（DNS-01） | 为证书申请证明域名控制权 | 验证记录 | 用完即删 | 证书申请 | `crates/naming` 的 DNS-01 接口 |
| 证书版本与部署（实验） | 申请、保存与部署证书 | 证书版本、证书与端点的绑定 | 签发、加载与外部可达分别记录 | 流量服务（节点证书）；作业（部署执行）；观测（握手验证） | 宿主 `network_configuration/` 的 ACME 与证书部分（证书文档存在 `network_documents`，表归基础网络，阶段 F 拆分）；服务器 › 网络配置 |
| 节点证书（运行时自行签发） | 让节点自己申请与续期证书 | 节点证书设置 | 控制者是运行时，面板只配置 | 流量服务 | `crates/compiler/src/certificates.rs` |
| 域名台账（实验） | 记录拥有的域名 | 域名台账 | 只是台账 | 成本（续费项目） | `network_documents` 的域名文档（表归基础网络，阶段 F 拆分） |

## 成本与费用 `costs`

| 功能 | 意图 | 核心对象 | 关键约束 | 协作 | 实现与入口 |
| --- | --- | --- | --- | --- | --- |
| 续费项目与付费服务 | 记下要周期付费的服务 | 续费项目、付款记录 | 金额、周期与币种清楚；预测与实际分开 | — | `crates/costs`（原费用插件）；费用与续费 |
| 服务器费用 | 记录服务器的购买、续费与退款 | 服务器费用记录 | 服务器到期以资源的资产为准，这里只读 | 资源 | 宿主 `server_lifecycle.rs` 的费用记录、`fleet_cost_records`（阶段 B 并入成本） |
| 供应商计费流量与免费额度 | 按供应商口径看额度用了多少 | 计费流量历史、额度状态 | 保留供应商口径，不与网卡计数或用户用量混算 | 资源（实例）；作业（超额处置） | `crates/costs` 的 `cloud/`（`billing`、`cost_cache`、`traffic_history`；类型在资源的 `cloud/model.rs`，随所在的行） |
| 汇率与统计币种 | 把不同币种折算成统计币种 | 每日汇率 | 来源可追溯，不编造缺失币种 | — | `crates/costs` 的 `exchange.rs`（阶段 B2c 从宿主迁入）；`/api/exchange-rates` |
| 续费提醒与日历 | 按时提醒续费 | 提醒、日历输出 | 提醒只告知，不付款 | 平台（通知通道） | `crates/costs` 的后台任务；`/api/plugins/expenses/calendar.ics` |
| 退订提醒与供应商 | 记得在合适的时候退订 | 退订提醒、供应商 | — | 资源；观测（提醒记为故障事件） | `crates/costs` 的 `cancellation_reminders.rs`、`suppliers.rs` |
| 外部来源报告的到期 | 机场订阅的到期一起看 | 到期引用 | 只读，仅供参考 | 流量服务 | `crates/costs` 的 `renewals.rs` 直接读 sing-box 的外部来源（成本排在流量服务之后，不需要接口） |
| Wallos 导入 | 从 Wallos 迁入 | 导入批次 | 先预览后导入 | — | `tools/export_wallos.py`、`crates/costs` 的导入接口 |
| 云资源与费用对照（冻结） | 对比供应商价格 | 费用对照 | 只读 | 资源 | `crates/costs` 的 `suppliers.rs`；宿主 `cloud_resources.rs` 的账单与预算观测 |

## 平台治理 `governance`

| 功能 | 意图 | 核心对象 | 关键约束 | 协作 | 实现与入口 |
| --- | --- | --- | --- | --- | --- |
| 管理员、权限与会话 | 决定谁能管理什么 | 管理员、角色、能力、会话 | “允许管理节点”与服务用户的“允许使用节点”分开计算 | — | 宿主 `control_center/administrators.rs`、`auth/`；系统 › 管理员与权限 |
| 二步验证、Passkey 与再次验证 | 保护管理员登录与敏感操作 | 认证因素、再次验证证明 | 五分钟证明；不复用订阅令牌 | — | 宿主 `auth/`、`passkeys/`；账号菜单 |
| 管理 API 令牌 | 让程序代表管理员调用 | API 令牌 | 令牌有用途与权限范围 | — | `management_api_tokens` |
| 接入令牌 | 让一台机器被信任接入 | 接入令牌 | 一次性、有效期 | 资源 | `enrollment_tokens` |
| 凭据保管与审计 | 秘密安全保存，操作可追溯 | 凭据保管策略、审计记录 | 秘密不进日志与前端 | 平台（加密凭据库） | 宿主 `control_center/` |
| 平台配置与功能显示 | 面板本身的设置 | 面板设置 | 领域设置在各自领域编辑 | 各领域只读 | 系统 › 面板设置；管理接口在宿主 `control_center/panel_settings.rs`，保存在平台的设置存储 `panel_settings`（宿主 `settings.rs`），其中告警阈值与通知渠道等各项的规则归各自领域 |
| 制品与版本 | 决定哪些面板与 Agent 版本可信、可用 | 签名制品目录、版本策略 | 独立验签；Agent 只从 GitHub 或镜像下载 | 流量服务（运行时制品） | 宿主 `artifacts.rs`；系统 › 制品与版本 |
| 平台备份与恢复 | 平台出事能恢复 | 恢复点、恢复演练 | 同基线、同 PostgreSQL 主版本恢复；凭据密钥独立保管 | 作业（执行） | 宿主 `backups.rs`（计划与执行）、`backups/executor.rs`、`backups/recovery.rs`；作业中的面板备份步骤在 `crates/operations` 的 `jobs/panel_backup.rs`；系统 › 备份与恢复 |
| 平台自身的健康 | 面板与后台任务是否正常 | 独立观察者报告、工具通告 | — | 观测（展示）；平台（任务心跳 `system_worker_heartbeats`） | `system_observer_reports`、`tool_advisory_observations`；系统自检 |

## 观测证据与事件 `observability`

| 功能 | 意图 | 核心对象 | 关键约束 | 协作 | 实现与入口 |
| --- | --- | --- | --- | --- | --- |
| 遥测与历史 | 知道机器现在与过去怎样 | 采集计划、样本、历史聚合、服务器事件时间线 | 实时样本与持久化确认分开 | 资源（服务器视图经资源的 `server_observation` 取得流量摘要与实时遥测；事件经平台的 `evidence` 记录） | `crates/observability` 的 `telemetry/`、`server_monitoring.rs`（健康、事件时间线与两台比较）；Agent 遥测；服务器详情 › 概况、Agent |
| 服务器看板与公开看板 | 一眼看清所有服务器 | 看板视图（汇总，不拥有数据） | 公开看板只读白名单，匿名不公开成本 | 资源；成本（看板折算用的汇率经观测声明的 `dashboard::ExchangeRates` 取得） | `crates/observability` 的 `dashboard.rs`；服务器列表的看板视图、`/#/dashboard` |
| 网卡流量计数与矫正 | 记下机器实际的网络活动 | 网卡计数、矫正记录 | 不改代理用量账本 | 资源（网卡额度设置；视图的流量摘要类型属于资源） | `crates/observability` 的 `server_traffic.rs`、`traffic_correction.rs`；表 `server_network_*`、`server_traffic_corrections` |
| 告警与通知 | 出事时告诉管理员 | 告警规则、告警事件、处置记录 | 通知只提醒，不执行付款、停用或远程命令 | 平台（通知通道） | `crates/observability` 的 `alerts/`（规则、评估与告警事件；发件箱与渠道归平台的 `notifications`）；云事件与投递在 `cloud_notices.rs`；监测与诊断 › 告警 |
| 故障事件（实验） | 聚合告警并记录处置 | 故障事件、处置记录 | 计划与告警合并（路线图 4.3） | 作业；资源、治理与成本经平台的 `evidence` 打开与恢复 | `crates/observability` 的 `incidents.rs` |
| 延迟检测与周期拨测 | 测三网延迟与丢包 | 拨测、拨测结果 | 执行许可可撤销；只测授权目标 | 资源（新服务器与接入模板的拨测经资源的 `server_observation`） | `crates/observability` 的 `probes.rs`、`latency_tasks.rs`；Agent 拨测；线路质量 |
| 中国可达性 | 区分端口封锁、IP 封锁与服务故障 | 可达性观测与事件 | 默认关闭；用量不超额度的 80% | 流量服务（节点端点；Nexus 切换前经 `china_reachability` 读取最近测量） | `crates/observability` `reachability/` |
| 网络检测 | 测连通性、路由与 IP 资料 | 检测方案、运行与结果 | 只测量，不改动 | 资源 | `crates/observability` 的 `network_workbench/`；线路质量 |
| IP 质量、TCP 诊断、服务器验机 | 判断一台服务器的出口与线路质量 | 诊断任务与报告 | 共用诊断任务生命周期（ADR 0028）；完整验机门禁未解除 | 资源；服务权益（门户经 `HostNetwork` 显示成员可见的网络属性） | `crates/observability` 的 `diagnostics/`、`ip_quality/`、`diagnostic_plugins/{ipquality,nodequality,tcpquality}/`（打包脚本、策略与上游锁定文件在 `crates/observability/upstream/{ipquality,nodequality}/`）；Agent 诊断适配器；服务器详情 › 诊断 |
| 趋势与报表 | 回顾一段时间的整体运行 | 汇总视图（网卡计数与代理用量两本账） | 两本账分开显示，各自标明口径 | 服务权益（用量，经接口） | `crates/observability` 的 `statistics.rs`（网卡计数）、`statistics/proxy_usage.rs`（代理用量，原 sing-box `statistics.rs`） |

## 变更与作业 `operations`

| 功能 | 意图 | 核心对象 | 关键约束 | 协作 | 实现与入口 |
| --- | --- | --- | --- | --- | --- |
| 远程命令（实验） | 在服务器上执行一条命令 | 命令与执行结果 | 本机授权、默认关闭；超时为未知；不自动重放 | 资源 | `crates/operations` 的 `commands.rs`；服务器详情 › Agent |
| 终端、日志与文件（实验） | 交互式处理一台服务器 | 终端会话、文件操作 | 本机运维策略授权 | 资源 | `crates/operations` 的 `fleet/terminal.rs`、`fleet/operations.rs`；服务器详情 › 运维 |
| 批量任务、定时计划、维护窗口、有界自动处置（实验） | 跨服务器按计划执行固定步骤 | 作业、执行尝试、计划 | 步骤由所属领域判断合法；结果未知要核对 | 所属领域 | `crates/operations` 的 `jobs/`；服务器 › 自动化与维护 |
| 固定类型的设备操作 | 下发有类型的设备操作并收回执 | 设备操作 | 过期与未知分开；迟到结果仍接受 | 资源；平台（其他领域经 `device_work` 提交） | `crates/operations` 的 `fleet/`（含能否接收新任务的判断） |
| 云实例启停与换 IP 工作流 | 按计划或阈值开关机、更换公网 IP | 启停作业、换 IP 作业 | 只经官方接口读回确认；失败绑回原地址 | 资源（是否允许）；成本（阈值）；观测（可达性验证） | `crates/operations` 的 `cloud/`（`power/`、`eip_rotation/`、`bandwidth`（带宽变更）、`worker`（账号刷新与自动降速））；资源经 `cloud::work::CloudWork` 询问进行中的操作 |
| 备份、升级与部署的执行 | 把治理或流量服务批准的变更执行出来 | 执行记录 | 只执行，不决定是否合法 | 治理；流量服务 | `crates/operations` 的 `jobs/`（部署步骤直接调用流量服务） |

## 平台 `platform` 与应用 `app`

| 部分 | 内容 | 实现 |
| --- | --- | --- |
| 平台 | 数据库连接与事务、基线核对、后台任务与心跳、Agent 通道、设备任务与观测证据的接口、各领域运行时状态的槽、加密凭据库、制品存储、通知发送通道、HTTP 外壳与中间件、设置存储、类型化接口的机制、文本与摘要检查、地址分类 | 宿主 `crates/panel-host` 的 `shell`、`agent_api`、`device_work`、`evidence`、`state`（`DomainStates`）、`ports`、`tokens`、`text`、`digest`、`net`、`settings`、`notifications` 的发送部分等技术模块（阶段 B1 起不引用领域，B2 迁出其余领域代码）；`crates/cloud-api`（供应商接口的签名、分页与重试） |
| 应用 | 组合各领域、HTTP 路由、后台任务登记、跨领域流程、依赖倒置接口的接线、命令行入口 | `crates/panel`：`lib.rs` 套上权限守卫，`domains.rs` 组合各领域的路由与后台任务，`wiring.rs` 接线类型化接口，`maintenance.rs` 运行 30 秒维护循环，`publisher.rs` 运行代理发布周期，`cloud_overview.rs` 是云页面的组合视图 |
| Agent | 本地执行、采集与状态协调；运行时与诊断工具的适配器 | `crates/agent-core`、`crates/adapter-*`、`crates/agent` |
