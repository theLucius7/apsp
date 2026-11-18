# 当前问题与决定

已完成的实施讨论不再作为待办保留，设计依据见 ADR，旧记录保存在 Git 历史。当前实现和实际生产切换分别记在 [PROGRESS](../PROGRESS.md)。

## 领域整合

2026-11-17 用户要求一次性完成、不再按插件维护、删除历史冗余。按 ADR 0140，内部使用领域模型、制品描述 `artifact_name`、诊断工具 `tool`（控制台呈现边界投影 `ReportRecord.job.tool`）与公开端点 `runtime_enabled`；已发布 Agent wire 字段、数据库任务记录、持久化检查点和签名格式只留在序列化边界。普通 `/api/plugins/*` 控制台过渡期改写已移除，仅永久保留外部续费日历订阅契约 `/api/plugins/expenses/calendar.ics` → `/api/costs/calendar.ics` 与外部代理用户链接 `#/plugins/sing-box/{account,member,join}` → `#/{account,member,join}`。旧任务书与执行计划退出当前工作树，不增加别名文件。

## 生产采用与既发地址

源码原生 `sing-box` 领域架构、先预览后原子提交的成员原生采用（[成员原生采用](native-adoption.md)）与生产切换分开记录：Nexus 成员采用原生订阅保留原 UUID、已发地址和当前周期用量，预览展示阻塞与入口条件，提交在事务中固定计量锚点并排除桥同步；但生产上的成员原生采用、公开原订阅入口（Caddy/DNS）切换与兼容桥退役**均尚未在生产执行**（待上线执行与读回核对，不凭源码合入视为生产切换完成）。仍在服务的桥在逐户采用与旧端停用确认前保留。

## 发布与运行时

面板当前目标是 `1.14.4-apsp.2`。运行时附件门禁只按目标设备选中的已验签制品判断，不因同一 Release 中其他平台的附件使主机停止接收配置；安装视图区分现有已应用配置健康（`current_healthy`）、同步受阻（`sync_blocked`）与需升级 Agent（`upgrade_required`）。新制品发布优先扩展现有固定候选 rollout，保持每台服务器的制品身份与配置修订一致；仅导入旧发布不会构成回退，固定候选与路径依赖需要新的受控修订。

## 权益查询性能与领域边界基线

`singbox_eligible_accesses` 的简单合并改写曾使某些真实调用变慢，已撤回；后续以调用处、数据规模和查询计划决定。本轮已收口代理用量统计（`access/src/usage/statistics.rs`）、生命周期策略（`server_lifecycle::policy`）、迁移节点查询（`traffic::nexus_nodes`）、新服务器监测初始化（`ServerObservation::initialize_tx`）、门户三网延迟摘要（`HostNetwork::carrier_latencies`）与退订提醒故障通知（`Evidence`）；其余既有跨界项（如 `costs/suppliers.rs` 读取诊断表、`naming/acme.rs` 读取 `network_documents`、`credential_entries` 归属等）仍如实记在 `tools/feature-boundaries.baseline`，只许减少，不通过扩大豁免或虚假宣称零基线掩盖遗留项。

## 名称与诊断

lego 尚无固定构建配方，名称功能转正时需解决并迁回所属文档存储。NodeQuality 完整执行继续拒绝新任务，许可、rootfs、资源保护和实机矩阵按[整改条件](acceptance/ordered-remediation.md)完成后才能签收。

## CI

0.3.9 的临时发版例外已结束，项目工作流仍暂停。全面恢复按 AGENTS 的现行规则处理；本轮只记录实际执行的集中本地验证，不把未执行 CI 或生产迁移记为通过。
