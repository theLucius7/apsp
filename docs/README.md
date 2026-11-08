# 文档导航

先读[功能地图](product-map.md)了解 apsp 的定位（自托管的服务器管理与配置面板）、功能分级、六个工作区和各自边界；对象与用语见[对象词典](redesign/03-objects.md)。下面的使用文档按工作区排列（[ADR 0126](adr/0126-workspaces-language-and-addresses.md)），标“实验”的文档对应已降级或冻结的功能。界面在重新设计步骤 4 之前仍是现有的五个分组，对照见功能地图。代码位置见[目录说明](repository.md)，近期实现与验证见 [PROGRESS](../PROGRESS.md)。本文只是导航，具体限制和验收结果以对应专题为准。

## 开始使用

| 需要做什么 | 文档 |
| --- | --- |
| 了解产品定位、功能分级、工作区、界面用语和不做的事 | [功能地图](product-map.md)、[对象词典](redesign/03-objects.md)、[定位决策](adr/0087-narrow-positioning-proxy-network.md) |
| 了解后续里程碑、工作包、排期与待决定事项 | [后续路线图](roadmap.md)（提议） |
| 了解功能划分、核心对象、主线流程、状态与发布的重新设计及执行顺序 | [重新设计方案](redesign/README.md)（第一部分已由 ADR 0126 接受）、[执行计划](redesign/09-execution.md) |
| 安装面板、备份升级、导入签名制品、接入 Agent | [部署与节点接入](deploy.md) |
| 确认系统、架构、服务管理与升级能力 | [设备平台与能力](platforms.md) |
| 从旧名称迁移既有安装 | [改名说明](apsp-renaming.md) |

## 按工作区

### 网络服务（核心）

| 需要做什么 | 文档 |
| --- | --- |
| 为服务器安装运行时（sing-box） | [安装与控制流程](singbox-installation.md) |
| 配置代理协议、端点和证书 | [协议与证书](proxy-protocols.md) |
| 整理节点库，导入外部来源并把外部节点放进策略组 | [节点库与外部来源](node-catalog.md) |
| 配置策略组、套餐与重置周期 | [策略与套餐](singbox-groups.md) |
| 查看运行时状态，导入外部来源并创建链路 | [Agent 运维与链路](agent-runtime-and-chains.md) |
| 为代理用户开通自助门户与 Passkey | [Passkey 登录与开通](passkeys.md) |
| 了解运维与代理用户的完整流程 | [运维与代理用户流程](singbox-operations-workflows.md)、[读取与编辑保护](singbox-snapshot-editing.md) |

### 服务器

| 需要做什么 | 文档 |
| --- | --- |
| 配置地区、成本、到期、续费及网卡流量额度 | [服务器资产](server-assets.md) |
| 查看服务器实时状态、历史曲线，开启只读公开看板 | [服务器看板](server-display.md)、[展示数据说明](server-display-data.md) |
| 让域名跟随服务器公网 IP 自动更新（地址与域名） | [DDNS 插件](ddns.md) |
| 管理 CDT、ECS 启停、自动策略、带宽及费用缓存（云资源） | [阿里云插件](alicloud.md)、[RAM 权限](alicloud-ram.md)、[原 CDT 脚本迁移](cdt-script-migration.md) |
| 使用终端、日志与文件、资产生命周期和批量登记（实验） | [日常运维](fleet-operations.md) |
| 批量任务与定时计划（实验）；面板备份与恢复见「系统」 | [备份恢复与自动化](operations-recovery.md) |
| 维护域名台账、证书、端点、转发、隧道、组网与防火墙（实验） | [网络配置](network-configuration.md) |

### 监测与诊断

| 需要做什么 | 文档 |
| --- | --- |
| 配置告警阈值、Telegram 与 Webhook；延迟检测与中国可达性 | [延迟检测与通知](monitoring.md) |
| 测量连通性、路由与 IP 质量；吞吐测试（实验）与硬件测试（冻结） | [网络检测](network-workbench.md) |
| 回顾网卡与代理流量趋势，理解统计口径 | [统计报表](statistics.md) |

### 费用与续费

| 需要做什么 | 文档 |
| --- | --- |
| 续费项目、即将续费、支出与付费服务 | [功能地图](product-map.md#支撑)、[ADR 0118](adr/0118-costs-and-renewals-page.md) |
| 从 Wallos 导入 | [Nexus 与 Wallos 迁移](nexus-migration.md) |

### 系统

| 需要做什么 | 文档 |
| --- | --- |
| 管理员与权限、凭据、API 令牌、会话与审计 | [管理与安全](control-center.md)、[全局搜索与可见性](control-center-search.md) |
| 管理管理员及代理用户的通行密钥 | [Passkey 登录与开通](passkeys.md) |
| 查看插件说明、已验证版本与执行服务器（制品与版本） | [插件中心与执行边界](plugin-catalog.md) |
| 面板备份与恢复 | [备份恢复与自动化](operations-recovery.md)、[部署与节点接入](deploy.md) |

### 迁移与兼容

| 需要做什么 | 文档 |
| --- | --- |
| Nexus 用户、节点与运行时迁移，旧订阅兼容桥 | [Nexus 与 Wallos 迁移](nexus-migration.md)、[Nexus 服务器向 apsp Agent 迁移](nexus-agent-migration.md) |
| 来源与链路统一（数字编号来源、两跳旧链路、混合链路） | [阶段三迁移方案](rearchitecture-phase3-plan.md)、[收敛方案](redesign/07-convergence.md#3-兼容流程独立) |

## 开发与设计

- [协作规则](../AGENTS.md)：现行规则；2026-11-08 整理前的原文与各条去向见[整理记录](history/agents-rules-20261108.md)。
- [功能划分与逻辑结构](architecture.md)：九个领域、依赖与写入规则、运行模型、代码结构、上游代码归属与迁移阶段（ADR 0128）。
- [领域归属表](domain-map.md)：每项功能属于哪个领域、核心对象、关键约束、协作领域与实现位置。
- [重新设计方案](redesign/README.md)：设计依据、功能盘点、对象词典、工作区、主线流程、状态设计（已接受）；收敛方案、工程整理与执行计划。
- [仓库目录与维护约定](repository.md)：修改入口、模块归属、脚本及测试的位置。
- [本地开发、构建与检查](dev.md)：Rust、PostgreSQL、Bun 与条件测试。
- [HTTP API](api.md)、[面板与 Agent 协议](protocol.md)、[术语表](glossary.md)：公共契约与技术术语。
- [架构决策索引](adr/README.md)、[执行中的问题与选择](open-questions.md)：设计依据及范围变更。
- [数据库基线](database-baseline.md)：当前基线、旧库处理与生产接入。
- [混合链路设计](node-chain-design.md)、[订阅来源设计](chain-subscription-sources.md)、[订阅实现与参考记录](subscription-source-implementation-notes.md)。
- [NodeQuality Debian 输入收集](nodequality-rootfs-collection.md)、[NodeQuality 离线工厂操作边界](nodequality-rootfs-factory.md)。
- 历史基线：[原始 MVP 任务](requirements.md)、[G1–G9 计划](PLAN.md)；后续授权和现行约束以 [AGENTS.md](../AGENTS.md) 及相应 ADR 为准。

## 发布与验收

- [签名发布与信任根](release.md)：签署、发布身份、公钥轮换与运行时回退。
- [真实 Reality 验收](e2e.md)：专用环境操作及证据要求。
- [验收索引](acceptance/README.md)：按主题定位源码检查、隔离测试与实机证据。
- [整改顺序与验收状态](acceptance/ordered-remediation.md)、[NodeQuality 安全门禁](acceptance/nodequality-full-start-gate.md)：签收与发布前置条件。
- [2026-10-04 其余分支与 main 整合](acceptance/branch-main-integration-20261004.md)：迁移编号、架构与安全合并取舍，集中验证另记。
- [平台扩展 21 类任务与入口](operations-expansion.md)、[当轮集中验收](acceptance/operations-platform-expansion.md)。
- [流量 outbox 有界读取](acceptance-bounded-usage.md)：保留原独立验收入口。

当前 GitHub Actions 暂停；本地测试、历史 CI、源码合入与实机能力签收分别记录，不互相替代。
