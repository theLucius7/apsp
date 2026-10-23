# 文档导航

先读[功能地图](product-map.md)了解 apsp 的定位（自托管的代理网络控制平面）、功能分级和各自边界；下面的使用文档按功能区排列，代理服务在前，标“实验”的文档对应已降级或冻结的功能。代码位置见[目录说明](repository.md)，近期实现与验证见 [PROGRESS](../PROGRESS.md)。本文只是导航，具体限制和验收结果以对应专题为准。

## 开始使用

| 需要做什么 | 文档 |
| --- | --- |
| 了解产品定位、功能分级、界面用语和不做的事 | [功能地图](product-map.md)、[定位决策](adr/0087-narrow-positioning-proxy-network.md) |
| 了解接下来的版本计划与待决定事项 | [后续路线图](roadmap.md)（提议） |
| 安装面板、备份升级、导入签名制品、接入 Agent | [部署与节点接入](deploy.md) |
| 确认系统、架构、服务管理与升级能力 | [设备平台与能力](platforms.md) |
| 从旧名称迁移既有安装 | [改名说明](apsp-renaming.md) |

## 按功能区

### 代理服务（核心，sing-box 插件）

| 需要做什么 | 文档 |
| --- | --- |
| 为服务器启用并安装 sing-box | [安装与控制流程](singbox-installation.md) |
| 配置代理协议、端点和证书 | [协议与证书](proxy-protocols.md) |
| 整理节点库、导入外部订阅并直接授权 | [节点库与外部订阅](node-catalog.md) |
| 配置策略组、套餐、授权和周期 | [策略与套餐](singbox-groups.md) |
| 导入机场订阅并创建混合链路 | [Agent 运维与混合链路](agent-runtime-and-chains.md) |
| 为代理用户开通自助门户与 Passkey | [Passkey 登录与开通](passkeys.md) |
| 了解运维与代理用户的完整流程 | [运维与代理用户流程](singbox-operations-workflows.md)、[读取与编辑保护](singbox-snapshot-editing.md) |

### 总览

| 需要做什么 | 文档 |
| --- | --- |
| 查看服务器实时状态、历史曲线，开启只读公开看板 | [服务器看板](server-display.md)、[展示数据说明](server-display-data.md) |
| 回顾网卡与代理流量趋势，理解统计口径 | [统计报表](statistics.md) |

### 服务器

| 需要做什么 | 文档 |
| --- | --- |
| 配置地区、成本、到期、续费及网卡流量额度 | [服务器资产](server-assets.md) |
| 使用终端、日志与文件、资产生命周期和批量登记（实验） | [日常运维](fleet-operations.md) |
| 面板备份与恢复；批量任务、故障事件和云费用账本（实验） | [自动化与恢复](operations-recovery.md) |
| 查看 Agent 运行时状态、执行或取消远程命令（远程命令为实验） | [Agent 运维与混合链路](agent-runtime-and-chains.md) |

### 监控告警

| 需要做什么 | 文档 |
| --- | --- |
| 配置延迟检测、告警阈值、Telegram 与 Webhook | [延迟检测与通知](monitoring.md) |

### 网络

| 需要做什么 | 文档 |
| --- | --- |
| 测量连通性、路由与 IP 质量；吞吐与硬件测试（实验） | [网络检测](network-workbench.md) |
| 维护域名、证书、端点、转发、隧道、组网与防火墙（实验） | [网络配置](network-configuration.md) |
| 让域名跟随服务器公网 IP 自动更新 | [DDNS 插件](ddns.md) |

### 扩展

| 需要做什么 | 文档 |
| --- | --- |
| 查找插件、查看已验证版本、选择执行服务器 | [插件中心与执行边界](plugin-catalog.md) |
| 管理 CDT、ECS 启停、自动策略、带宽及费用缓存 | [阿里云插件](alicloud.md) |

### 系统

| 需要做什么 | 文档 |
| --- | --- |
| 管理员与权限、凭据、API 令牌、会话与审计 | [管理与安全](control-center.md)、[全局搜索与可见性](control-center-search.md) |
| 管理管理员及代理用户的通行密钥 | [Passkey 登录与开通](passkeys.md) |

## 开发与设计

- [仓库目录与维护约定](repository.md)：修改入口、模块归属、脚本及测试的位置。
- [本地开发、构建与检查](dev.md)：Rust、PostgreSQL、Bun 与条件测试。
- [HTTP API](api.md)、[面板与 Agent 协议](protocol.md)、[术语表](glossary.md)：公共契约。
- [架构决策索引](adr/README.md)、[执行中的问题与选择](open-questions.md)：设计依据及范围变更。
- [重新架构阶段三：来源与链路统一迁移方案](rearchitecture-phase3-plan.md)：S1 代码准备已实施；生产来源迁移、接管、转换与实机验收另行安排。
- [混合链路设计](node-chain-design.md)、[订阅来源设计](chain-subscription-sources.md)、[订阅实现与参考记录](subscription-source-implementation-notes.md)。
- [NodeQuality Debian 输入收集](nodequality-rootfs-collection.md)、[NodeQuality 离线工厂操作边界](nodequality-rootfs-factory.md)。
- [原始 MVP 任务](requirements.md)、[G1–G9 计划](PLAN.md)：历史基线；后续授权和现行约束以 [AGENTS.md](../AGENTS.md) 及相应 ADR 为准。

## 发布与验收

- [签名发布与信任根](release.md)：签署、发布身份和公钥轮换。
- [真实 Reality 验收](e2e.md)：专用环境操作及证据要求。
- [验收索引](acceptance/README.md)：按主题定位源码检查、隔离测试与实机证据。
- [整改顺序与验收状态](acceptance/ordered-remediation.md)、[NodeQuality 安全门禁](acceptance/nodequality-full-start-gate.md)：签收与发布前置条件。
- [2026-10-04 其余分支与 main 整合](acceptance/branch-main-integration-20261004.md)：迁移编号、架构与安全合并取舍，集中验证另记。
- [平台扩展 21 类任务与入口](operations-expansion.md)、[本轮集中验收](acceptance/operations-platform-expansion.md)。
- [流量 outbox 有界读取](acceptance-bounded-usage.md)：保留原独立验收入口。

当前 GitHub Actions 暂停；本地测试、历史 CI、源码合入与实机能力签收分别记录，不互相替代。
