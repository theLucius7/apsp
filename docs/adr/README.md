# 架构决策索引

[文档导航](../README.md) · [目录与维护约定](../repository.md) · [协作约束](../../AGENTS.md)

按主题查找 ADR；编号保留决策顺序，文件名和历史链接保持稳定。早期范围限制可能由后续用户授权覆盖，判断现行约束须结合 AGENTS.md 与对应后续 ADR。例如 Agent 二进制下载以 ADR 0038 为准，替代 ADR 0010 中的面板同源限制；链路能力由 ADR 0040/0045 扩展。设计决策不代表已经通过实机验收。

每份 ADR 开头写明状态（已接受、部分被取代、被取代）及取代它的 ADR；下面的条目在括号中注明替代关系。ADR 0128 起不再有插件：较早 ADR 中的 `plugins/ddns/panel`、`plugins/expenses/panel`、`plugins/cloud_api/panel` 是当时的位置，现在分别是 `crates/naming`、`crates/costs`、`crates/cloud-api`；设备上的路径不变，`/api/plugins/...` 接口前缀自 ADR 0132（2026-11-13）起按领域改名，对应表见 [ADR 0132](0132-platform-only-host-and-domain-addresses.md) 第 9 条。阶段 B2a（2026-11-08）起，较早 ADR 中宿主的 `fleet/`、`commands/`、`operations/` 与 `runtime_operations.rs` 在 `crates/operations`（`operations/` 改名 `jobs/`），其中属于其他领域的部分分别移到宿主的 `server_lifecycle.rs`、`batch_enrollment.rs`、`server_monitoring.rs`、`cloud_resources/`、`incidents.rs`、`backups/` 与 `crates/costs`，接口路径不变。阶段 B2b 起，宿主的遥测、拨测、延迟检测、诊断、IP 查询、网络检测、告警（原 `notifications/` 的规则、评估与事件，现为 `alerts/`）、故障事件、统计与看板在 `crates/observability`，`plugins/{ipquality,nodequality,tcpquality}/panel` 的诊断工具代码在它的 `diagnostic_tools/` 下（ADR 0132 之前叫 `diagnostic_plugins/`；打包脚本与上游锁定文件现在在 `crates/observability/upstream/`），sing-box 的 `statistics.rs` 是它的 `statistics/proxy_usage.rs`。阶段 B2c（2026-11-09）起，宿主的 `exchange.rs`（汇率）在 `crates/costs`，`device_config.rs`、`runtime_control/` 与 `runtime_validations.rs` 在 `plugins/singbox/panel`（阶段 C 起是 `crates/traffic`），接口路径不变。ADR 0132（2026-11-13）起，宿主 `crates/panel-host` 只剩平台机制，较早 ADR 中宿主的其余模块在各自的领域 crate：`auth/`、`passkeys/`、`control_center/`、`releases/` 与 `artifacts.rs` 在 `crates/governance`（其中的搜索在应用层 `crates/panel/src/search.rs`）；`servers.rs`、`server_assets.rs`、`retirement.rs`、`installation*`、`batch_enrollment.rs`、`server_lifecycle.rs`、`cloud_resources/` 在 `crates/resources`，Agent 通道 `agent_api.rs` 是它的 `device_channel.rs`；`network_configuration/` 在 `crates/networking`，其中的 ACME、证书、证书部署与 DNS-01 验证在 `crates/naming`；`backups/` 与 `agent_updates.rs` 在 `crates/operations`。编号说明：0043 有两份文件（`0043-runtime-operations.md` 与 `0043-nodequality-offline-rootfs.md`，后者与 0049 主题相同、内容略有差异，索引列 0049）；0052 与 0066、0053 与 0067 主题相同，分别来自主线与作者独立分支的整合，内容有差异，引用时以各自文件为准。编号保持不变。

## 配置、协议与计量基础

- [ADR 0001：声明式全量快照](0001-declarative-snapshots.md)
- [ADR 0002：Agent 三层结构与无状态适配器](0002-agent-layering.md)
- [ADR 0004：单端计量与持久化差值账本](0004-durable-usage-accounting.md)
- [ADR 0005：统计用户名按用户与节点唯一](0005-user-node-stat-name.md)
- [ADR 0007：应用前写入意图日志](0007-intent-journal.md)
- [ADR 0008：串行应用并合并为最新版本](0008-latest-revision-coalescing.md)
- [ADR 0009：统一信封与前向兼容](0009-forward-compatible-envelope.md)
- [ADR 0011：本地 API 仅监听回环地址](0011-loopback-local-api.md)
- [ADR 0012：异步流工具和日志订阅器](0012-async-streams-and-log-subscriber.md)
- [ADR 0014：在构建时生成统计协议消息](0014-protobuf-build-tooling.md)

## 部署、身份、服务与运维

- [ADR 0003：运行时使用独立 systemd 服务](0003-independent-runtime-service.md)
- [ADR 0006：特权操作经由统一 trait](0006-privileged-trait.md)
- [ADR 0010：Agent 只从面板下载](0010-panel-only-downloads.md)（Agent 二进制部分已由 ADR 0038 取代）
- [ADR 0013：在进程内安全解包运行时制品](0013-archive-decoding.md)
- [ADR 0015：扩展 Agent 编译产物的平台](0015-agent-build-platforms.md)
- [ADR 0017：minisign 发布制品、构建时信任根与 Release 导入](0017-signed-release-artifacts.md)
- [ADR 0018：单管理员登录限速与 TOTP](0018-login-security.md)
- [ADR 0019：删除服务器前完成在线退役](0019-server-retirement.md)
- [ADR 0021：增加 OpenRC 独立服务管理](0021-openrc-services.md)
- [ADR 0022：补齐 Agent 监控、任务、升级和多系统部署](0022-agent-capability-alignment.md)
- [ADR 0037：可复制接入入口与按服务器架构导入](0037-bootstrap-and-selective-import.md)
- [ADR 0038：服务器运营设置、看板访问和 GitHub Agent 下载](0038-server-operations-and-public-dashboard.md)
- [ADR 0041：跨平台单行 Agent 接入](0041-cross-platform-enrollment.md)
- [ADR 0075：节点高级参数与后台刷新呈现](0075-node-options-and-background-refresh.md)
- [ADR 0042：节点配置与面板部署管理](0042-node-settings-and-panel-operations.md)
- [ADR 0043：运行时状态、脱敏日志与明确运维操作](0043-runtime-operations.md)
- [ADR 0044：远程命令的持久状态和确认取消](0044-command-lifecycle.md)
- [ADR 0059：管理员与代理用户 Passkey](0059-passkeys-and-proxy-user-access.md)

## 代理业务、协议与混合链路

- [ADR 0020：链式中转的内部凭证、入口计量与依赖发布](0020-chained-transit.md)
- [ADR 0023：服务器核心与 sing-box 代理业务分层](0023-proxy-business-boundary.md)
- [ADR 0030：sing-box 业务搬迁与服务器启用证据](0030-singbox-plugin-business.md)
- [ADR 0034：现代代理协议与托管证书](0034-modern-protocols-and-certificates.md)
- [ADR 0035：sing-box 策略组、套餐快照与受控两跳链路](0035-singbox-policy-package-groups.md)（单独授权已由 ADR 0112 取消，两台服务器的限制已由 ADR 0040 取代）
- [ADR 0040：有序混合链路与机场订阅来源](0040-mixed-chains-and-subscriptions.md)
- [ADR 0045：混合链路的执行与验证](0045-mixed-path-execution.md)
- [ADR 0054：链路写入时复核资源快照与原选择](0054-chain-resource-snapshot-writes.md)
- [ADR 0076：节点库、来源预览与外部订阅授权](0076-node-catalog-and-external-access.md)（外部节点按人分配已由 ADR 0112 改为放进策略组）
- [ADR 0083：代理流量按天汇总与数据保留](0083-usage-daily-rollup-and-retention.md)
- [ADR 0084：管理界面重排、节点页分区与离线时间](0084-admin-layout-and-node-sections.md)（导航分组已由 ADR 0086 取代）
- [ADR 0085：重新架构：插件边界、订阅来源与链路模型（已接受，分阶段实施；三层结构被 ADR 0128 取代）](0085-rearchitecture-plugins-sources-chains.md)

## 诊断、IP 查询与执行保护

- [ADR 0016：IP 质量与节点诊断外插](0016-nodequality-diagnostics.md)
- [ADR 0024：IP 查询错误的类型与来源](0024-ip-provider-error-classification.md)
- [ADR 0025：按 IP 与入口保留逐数据库成功快照](0025-ip-provider-cache.md)
- [ADR 0026：诊断取消由设备确认清理](0026-confirmed-diagnostic-cancellation.md)
- [ADR 0027：IP 查询入口适配与正式凭据](0027-ip-provider-adapters.md)
- [ADR 0028：共用诊断任务服务](0028-shared-diagnostic-job-service.md)
- [ADR 0029：TcpQuality 许可核对与原生 Rust 路径](0029-tcpquality-license-and-native-probes.md)
- [ADR 0031：暂缓 NodeQuality 不受控完整任务的新执行](0031-nodequality-full-start-gate.md)
- [ADR 0032：原生 TCP 工具的固定源码与完整签名制品](0032-native-tcp-artifacts.md)
- [ADR 0033：TCP 诊断登记与配置目标快照](0033-tcpquality-panel-registration.md)
- [ADR 0046：有授权范围的轻量三网周期监控](0046-authorized-periodic-network-monitoring.md)
- [ADR 0049：NodeQuality 的离线 Debian 12 rootfs 准备链](0049-nodequality-offline-rootfs.md)
- [ADR 0050：使用节点私有凭证的正式 IP 查询](0050-official-node-ip-query.md)
- [ADR 0051：带目标授权的轻量三网周期监控](0051-authorized-carrier-monitoring.md)
- [ADR 0052：诊断终态以停止和清理证明为准](0052-confirmed-diagnostic-completion.md)
- [ADR 0053：周期拨测使用可撤销的短期执行许可](0053-authorized-probe-leases.md)
- [ADR 0055：诊断材料收集的下载失败证据与工厂容量](0055-diagnostic-material-download-evidence.md)

## 资产、监控与通知

- [ADR 0036：服务器资产配置与账单周期网卡用量](0036-server-assets-and-traffic.md)
- [ADR 0039：统一延迟任务与完整服务器通知](0039-latency-tasks-and-notification-rules.md)
- [ADR 0047：实时监控、历史粒度、每日汇率与多渠道通知](0047-monitoring-refresh-history-and-channels.md)

## DDNS 与云资源

- [ADR 0048：使用 Agent 已上报地址的 Cloudflare DDNS](0048-cloudflare-ddns.md)
- [ADR 0056：多云 DDNS 插件](0056-multicloud-ddns.md)
- [ADR 0060：DDNS 双栈配置入口](0060-ddns-dual-stack-creation.md)
- [ADR 0057：阿里云 CDT 与公网带宽管理](0057-alicloud-cdt-management.md)
- [ADR 0058：阿里云 ECS 启停、自动策略与费用缓存](0058-alicloud-power-and-billing-cache.md)

## PR151 追加决策及来源

以下来自作者独立实现，按主线追加编号（作者原分支编号比文件编号小 2）；整合验证另记。

- [ADR 0061：sing-box 两端插件与服务器安装流程](0061-singbox-plugin-lifecycle.md)
- [ADR 0062：NodeQuality 整合的制品身份与未授权查询](0062-nodequality-artifact-lineages.md)
- [ADR 0063：收集实际 Debian 诊断输入](0063-nodequality-input-collection.md)
- [ADR 0064：精确运行确认与持久恢复屏障](0064-runtime-checkpoints-and-recovery-barriers.md)
- [ADR 0065：NodeQuality 离线工厂的容量与失败证据](0065-nodequality-factory-capacity.md)
- [ADR 0066：诊断结束也等待设备清理确认](0066-confirmed-diagnostic-completion.md)
- [ADR 0067：周期拨测来源与可撤销执行许可](0067-authorized-probe-leases.md)
- [ADR 0068：独立节点出口 IPQuality](0068-independent-node-ipquality.md)
- [ADR 0069：从认证 Debian 缓存派生最小 IPQuality 输入](0069-ipquality-derived-debian-inputs.md)
- [ADR 0070：IPQuality 最小闭包证明贯穿工厂与制品](0070-ipquality-minimal-profile-chain.md)
- [ADR 0071：统一代理资源与原子链路创建](0071-proxy-resource-batch-lifecycle.md)
- [ADR 0072：有界订阅导入与不可变来源版本](0072-subscription-source-lifecycle.md)
- [ADR 0073：有序路径发布与具体出站验证](0073-ordered-path-publication-and-native-probe.md)
- [ADR 0074：Agent 的可选私有面板 CA](0074-private-panel-certificate-authorities.md)

## 平台扩展（2026-10-03）

- [0077：服务器接入与日常运维](0077-fleet-operations.md)
- [0078：网络与验机工作台](0078-network-hardware-workbench.md)
- [0079：DNS、证书与服务器网络配置](0079-network-configuration-and-dns-lifecycle.md)
- [0080：sing-box 运维业务流程](0080-singbox-operations-workflows.md)
- [0081：批量运维与独立恢复](0081-operations-recovery-and-cloud-ledger.md)
- [0082：管理员权限、凭据与操作上下文](0082-control-center.md)

## 产品定位与界面（2026-10-24）

- [ADR 0086：产品定位、功能分区与管理界面导航](0086-product-map-and-navigation.md)：七个功能区、服务器详情分页、设置拆分与前端目录；定位与归类规则见[功能地图](../product-map.md)（分组先后由 ADR 0098、0102、0126 取代）
- [ADR 0087：收窄定位为代理网络控制平面：功能分级、用户门户与费用续费账本](0087-narrow-positioning-proxy-network.md)：核心、支撑、可选插件、降级与冻结清单，Xboard 迁移边界，新功能准入规则
- [ADR 0088：界面重构：shadcn/ui Nova、Tailwind CSS v4 与统一交互库](0088-ui-component-system.md)：组件、设计变量、图标、动画、分栏、拖拽与界面状态的统一约定

## Nexus 与 CDT 迁移（2026-10-24）

- [ADR 0089：Nexus 成员业务原生接入](0089-nexus-member-commerce.md)（售卖部分已由 ADR 0113 删除）
- [ADR 0090：Wallos 费用订阅配置原生接入](0090-recurring-expense-subscriptions.md)
- [ADR 0091：成员账号与邮件](0091-member-account-and-mail.md)（账号与邮件保留，归入代理用户，见 ADR 0113）
- [ADR 0092：Nexus 实际迁移与旧订阅兼容](0092-nexus-live-cutover.md)
- [ADR 0093：原 CDT 脚本行为的原生守护模式](0093-cdt-script-guard.md)
- [ADR 0094：Nexus 节点信息导入与运行时接管分开](0094-nexus-node-inventory.md)
- [ADR 0095：保留客户端身份的 Nexus 运行接管](0095-nexus-runtime-takeover.md)：按主机准备、候选、启动、暂停与撤销准备；SS 2022 重写保留原密钥，旧倍率计量回执
- [ADR 0096：费用与续费：服务器项目、付款记录与总览入口](0096-costs-and-renewals-ledger.md)：在已部署的订阅费用插件上增量扩展，服务器按资产设置只读列入，续期记录付款，入口移到总览、地址不变
- [ADR 0097：费用与续费并入服务器费用记录、退订提醒与机场订阅到期](0097-ledger-server-records-and-source-references.md)：服务器费用记录与退订提醒统一在账本编辑（数据与接口不变），付款记录分页，宿主钩子提供机场订阅到期引用
- [ADR 0099：接管后的 Nexus 节点继续执行设备数限制](0099-nexus-device-limits.md)：与旧节点相同的按客户端 IP 判定，归属来自仅内存的运行时日志命名空间，回环控制接口关闭超额会话，设备计数经兼容桥与旧节点共享
- [ADR 0100：OpenRC 的会话日志仅保存在内存](0100-openrc-volatile-session-logs.md)：独立采集进程、受保护的本机读取接口、重连重放及旧签名 Agent 安装兼容
- [ADR 0101：订阅支持 Clash Meta（mihomo）格式](0101-clash-subscription-format.md)：`format=clash` 由与 sing-box 订阅相同的客户端出站转换，无法等价表达的节点列在文件注释中；同一订阅地址按 User-Agent 识别 Clash 客户端
- [ADR 0105：中国可达性监控](0105-china-reachability-monitor.md)：默认关闭，经 Globalping 比较中国与海外的 TCP 连接，用对照端口区分端口封锁与 IP 封锁，连续两轮失败才判定封锁，用量不超过额度的 80%

- [ADR 0106：成员门户的线路目录与探针](0106-member-lines-and-probes.md)：代理用户登录后查看 Nexus 线路的入口、落地、类型、状态与三网延迟；探针按主机显示测得的 ASN、用途类型与风险标记，家宽标为运营方声明，不显示地址、城市与坐标
- [ADR 0107：每位管理员各自的 Passkey](0107-administrator-passkeys.md)：具名管理员绑定、登录和再次验证都用本人的 Passkey，登录页按登录名验证
- [ADR 0108：统一的再次验证](0108-unified-reauthentication.md)：开通链接、本人 Passkey、二步验证和修改密码都改用五分钟证明与共用的「再次验证身份」，Passkey 或密码均可
- [ADR 0109：隧道后的 VLESS 源站](0109-vless-tunnel-origins.md)：新增 `vless-tunnel`，无 TLS 的源站只能监听回环，客户端用 TLS 连边缘并以隧道主机名作 SNI/Host；运行接管可迁 Alice 等 CF 源站，可达性监测不再误报源站端口
- [ADR 0110：域名解析不再按服务器启用，页面按使用顺序重排](0110-ddns-without-server-switch.md)：所有接入 Agent 的服务器可直接绑定解析，规则自身的自动同步是唯一开关；迁移 0084 把原本不会运行的规则改为暂停；任意提供方的账号一步添加；添加解析只需域名、服务器和 IP 类型


## 界面重组（2026-10-25）

- [ADR 0098：按使用逻辑重组界面：功能分级呈现、运维工具并入服务器详情与概览首页](0098-usage-driven-navigation-and-tiers.md)：六组 22 个入口、概览首页、实验与冻结的界面处理、服务器详情六个分页、备份归入系统、单一编辑位置（分组先后由 ADR 0102、0126 取代，地址规则由 ADR 0120 取代）

## 界面重新设计（2026-11-01）

- [ADR 0102：连接韧性与控制台重新设计](0102-connection-resilience-and-console-redesign.md)：连接失败统一报告、读取超时、后台轮询不再使写入快照失效、错误边界、发版后自动恢复、单一页面顶栏、代理用户页三件事同屏与用户概况接口（侧栏分组由 ADR 0126 取代）。
- [ADR 0103：用户专属地址与 Cloudflare 账号保存](0103-per-user-subscription-hosts.md)：每个用户独立的订阅与登录主机名、重置即释放、一步保存 Cloudflare 令牌。

## 自维护运行时（2026-11-01）

- [ADR 0104：自维护 sing-box 运行时，修复 REALITY 可被区分的行为](0104-self-maintained-singbox-runtime.md)（运行时版本先后由 ADR 0119、0124 取代，补丁队列的做法继续有效）

## 服务器监控面板与界面视觉（2026-11-05）

- [ADR 0111：服务器页即监控面板，界面改用「墨与玉」视觉](0111-server-monitor-and-ink-jade-visuals.md)：一台服务器一个页面，概况即监控面板；删去重复的统计、表单与外壳；概览绘制代理网络；轮询接口不再携带进程明细（视觉部分由 ADR 0114 取代）。
- [ADR 0114：「极光」配色、背景图、按钮与交互、新标识](0114-aurora-palette-backdrop-and-controls.md)：取代 0111 的视觉部分；半透明表面透出自绘背景图，按钮按 4px 递进并统一悬停、按下与聚焦反馈，新增 favicon。

## 代理用户收敛（2026-11-05）

- [ADR 0112：代理用户只通过策略组获得节点](0112-group-only-access.md)：删除单独授权与按人分配外部节点，策略组可含外部节点，原单独授权原样转成策略组。
- [ADR 0113：去掉成员运营，邀请改为名额与邀请码](0113-invitation-quota-without-commerce.md)：删除钱包、销售套餐、订单与充值；邮箱密码登录、提醒、公告指南归入代理用户；邀请改为名额制邀请码，余额换算为名额。
- [ADR 0115：原权限组并入策略组](0115-original-permission-groups-as-policy-groups.md)：每个原权限组成为一个策略组，迁移线路放进策略组但保留原客户端身份，成员按迁移时的套餐映射分组，原面板的组号改由策略组推出。

## 阿里云更换公网 IP（2026-11-06）

- [ADR 0116：阿里云 ECS 更换公网 IP](0116-alicloud-eip-rotation.md)：绑定 EIP 的 ECS 从国内连不上时，更换到国内可用的新地址并释放旧地址；后台持久执行，云端调用不占用账户锁，失败绑回原地址，进度与结果并入「操作记录」。

## 订阅看板（2026-11-06）

- [ADR 0117：订阅看板](0117-subscription-board.md)：概览的第四个分页，只放管理员自己付费的订阅（「费用与续费」中的 Wallos 式项目），每个一张卡片；第一版的代理用户订阅与订阅来源按用户要求去掉，转发拉取的记录保留（位置由 ADR 0126 改为费用与续费中的「付费服务」）。

## 费用与续费页面（2026-11-06）

- [ADR 0118：费用与续费页面重排](0118-costs-and-renewals-page.md)：8 个分页合成「项目」「即将续费」「支出」三个视图与一个设置对话框；服务与服务器同一张表，每项详情有自己的地址，退订提醒并入服务器详情，每月费用按周期平摊。

## 旧服务同一份内核（2026-11-06）

- [ADR 0119：运行时改用旧服务同一份内核](0119-legacy-service-kernel.md)：`1.14.0-apsp.1` 由上游 `v1.14.0-alpha.2` 加 cedar2025 分支 `2e665cb7` 的两个提交重建，依赖固定为旧服务二进制的版本，纯 Go 构建、标签与旧服务相同（另加统计接口）；去掉 ADR 0104 的 REALITY 仿真补丁；原生节点的 Snell v6、ACME 证书提供者等新配置不再支持（内核选择已由 ADR 0124 取代）。
- [ADR 0120：页面地址按功能命名](0120-page-addresses-by-function.md)：地址与侧栏一致（`/#/home`、`/#/nodes`、`/#/users`、`/#/settings` 等），旧地址打开时改写为新地址（旧地址改写规则先后由 ADR 0122、0126 取代）。

## 未发布阶段收敛与功能划分（2026-11-07）

- [ADR 0121：CDT 监控补齐与源码制品分离](0121-cdt-monitor-alignment-and-source-artifacts.md)：分项采集时间、IPv6 地址、整体用量历史、Cron 与守护组合，生成的前端产物不再提交。
- [ADR 0122：未发布阶段统一当前结构](0122-unreleased-schema-baseline.md)：空库初始化、当前基线核验、旧开发库手动重建（外部地址部分由 ADR 0126 取代）。
- [ADR 0123：看板缓存一致性与分钟对比存储](0123-dashboard-cache-and-comparison-storage.md)
- [ADR 0124：稳定内核与逐用户限制](0124-stable-runtime-and-user-limits.md)
- [ADR 0125：按功能划分模块，内核只提供机制](0125-feature-modules.md)（部分被 ADR 0128 取代）：模块化单体、数据归属、上游归属；功能模块清单与登记口由 ADR 0128 改为九个领域。

## 工作区、统一用语与界面语言（2026-11-08）

- [ADR 0126：六个工作区、统一用语与地址策略](0126-workspaces-language-and-addresses.md)：接受[重新设计方案](../redesign/README.md)第一部分；外部地址永不失效，控制台地址改名后至少保留一个版本的改写；入口属于一个领域（第 6 条由 ADR 0128 调整）。
- [ADR 0127：界面以中文为源语言，英文为翻译](0127-chinese-source-language.md)：补记 2026-10-06 起的英文界面翻译，只提供中英两种界面语言。

## 按领域组织的一体化系统（2026-11-08）

- [ADR 0128：按领域组织的一体化系统，不再有插件](0128-domain-architecture.md)：七个对象与规则领域、两个运行支撑领域，平台只放技术机制；领域经公开接口协作、依赖无环，一个事实一个所有者；期望、执行、已应用与观测分别保存。领域定义与迁移阶段见[功能划分与逻辑结构](../architecture.md)，逐项功能见[领域归属表](../domain-map.md)。取代 ADR 0085 的三层与 ADR 0125 的模块清单。

## 代理管理能力（2026-11-07）

- [ADR 0129：按 S-UI 补齐代理管理能力](0129-sui-capability-alignment.md)：用户暂停与恢复、逐用户每服务器限速与连接数、认证连接查询与断开、客户端分流模板，服务端 DNS/路由/出口与 Cloudflared、Realm、VPN 端点；基线随 `scripts/baseline-adoption-sui-20261107.sql` 接入。

## 线路质量（2026-11-09）

- [ADR 0130：线路质量合为一个入口，三网延迟与中国可达性同表](0130-line-quality-overview.md)：侧栏「线路质量」分质量概况、检测设置、网络检测三个分页；每台服务器一行显示三网延迟、丢包与中国可达性，异常只看可用性、丢包与可达性；旧地址 `/#/latency/reachability` 打开质量概况。

## 一类对象一套模型（2026-11-09）

- [ADR 0131：每类对象只留一套模型，按本质整合功能，去掉插件外壳](0131-one-model-per-object.md)：有替代实现就整套删除，调用方一起改，旧数据转换后随基线删除；三条主链（供给、使用、证据与执行）与五个承载面重新整合按插件组织的功能；执行顺序改为三个大步骤。调整 ADR 0128 的迁移顺序，取代收敛方案的分阶段授权与重新设计执行计划的步骤顺序。

## 宿主只剩平台机制，接口地址按领域（2026-11-13）

- [ADR 0132：宿主只剩平台机制，设备连接归资源，接口地址按领域，路由权限一张表](0132-platform-only-host-and-domain-addresses.md)：治理成为 `crates/governance`，资源、基础网络与名称的代码迁出宿主；设备通道与接入令牌归资源，Agent 自更新检查与面板备份归作业；`Contributions` 钩子拆掉，路由权限合为治理守卫里的一张表，由审阅过的结论表守住；应用层一处组合；`/api/plugins/...` 按领域改名，旧地址改写一个发布版本。完成 ADR 0128 的阶段 B2，取代 ADR 0131 第 4 条的“接口前缀”一条。
