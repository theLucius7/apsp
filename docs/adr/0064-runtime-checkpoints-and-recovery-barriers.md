# ADR 0064：精确运行确认与持久恢复屏障

> 主线整合编号 0064；作者原独立分支编号 0047。旧主线同号决策及链接保留，历史验收仅认证原冻结输入。

状态：源码与本地验收完成，实际 Linux 服务与联合负载验收待执行。日期：2026-10-02。

## 问题与决定

当前设备的 `ApplyResult` 和心跳只给出模块与 revision，结果中的 operation UUID 可在重建时变化。面板据 revision 补记 healthy，无法把确认绑定到同一次部署、实际配置和受控运行实例。未完成 intent 的恢复也没有持久下限，不能作为 [ADR 0040](0040-mixed-chains-and-subscriptions.md) 撤旧身份之前的恢复条件。

新增业务中性的运行控制协议、面板请求/收据和设备持久 outbox。现有 `ModuleManifest`、`ApplyResult` 必需字段保持兼容；旧设备仍属于明确的 legacy 路径。新能力只在可以核对实际服务进程的后端声明。sing-box 的用户、授权、订阅和链路代数继续属于插件，core 仅核对模块、配置身份及恢复 revision。

本步骤同时修复 [#142](https://github.com/theLucius7/apsp/issues/142)：链路创建/删除的已开窗口和提交处理器也检查相关列表的失败与 pending，保留旧数据与草稿，恢复后重新核对实体。该界面修复不改变已有两跳模型或后端权限规则。

## 精确部署与受控实例

面板为每个不可变部署保存稳定的 `deployment_id`，绑定模块、revision 和原完整 bundle SHA256。`binding_digest` 仅标识这一绑定，不是签名，也不是全路径探测证明。配置仍由设备认证传输与 bundle 摘要保护；已有独立制品签名认证运行时，不能把两者混称为独立配置签名。

设备检查 actual current 引用、revision 目录中的普通配置文件及其完整字节、重新计算的 bundle 身份和已签运行时。通过服务管理器核对真正受控的主进程、可执行路径及配置参数；不能只返回 SQLite 中的 expected 字段。

Linux systemd 后端要求 loaded/active/running、非零 MainPID、无进行中的 ControlPID，并核对 ControlGroup 和 InvocationID。通过 Privileged 接口有界读取 `/proc` 的 stat、cmdline、cgroup、exe 与 boot 身份；读取前后 PID/starttime、命令、cgroup、可执行文件和 systemd invocation 必须稳定。只接受单个绝对配置路径，拒绝多个配置来源、未知实例、退出进程和替换的可执行文件。systemd 的运行周期身份及 proc starttime 语义依据 [systemd v252 原始文档](https://raw.githubusercontent.com/systemd/systemd/v252/man/systemd.exec.xml)和 [Linux proc stat 手册](https://man7.org/linux/man-pages/man5/proc_pid_stat.5.html)。其他后端不声明精确检查能力，不由一般 is_active 填造实例身份。

运行检查不是读取进程内部全部配置的接口。实际文件与命令参数还必须关联到 core 成功执行的 apply、适配器的目标健康检查和稳定 activation 记录；后续观察再次核对同一受控实例。Noop 和结果重建不生成新的 activation。旧数据库缺少 activation 时明确要求管理员重新部署，不由检查请求或普通轮询隐式重启业务，也不把旧历史标量补造为精确确认；新的明确更高版本部署可完成受控重应用后建立记录；已有 activation 因实际进程重启失效时也只允许明确更高版本受控 Restart，同版本检查/轮询仍拒绝且不重启。

## 持久请求、结果与恢复下限

`runtime.checkpoint.*` 核对部署绑定并返回 activation/instance；`runtime.barrier.*` 核对完整 expected checkpoint 后承诺最低可恢复 revision。请求包含稳定 UUID、完整内容摘要、有限期限和经过校验的身份。相同 UUID 的不同 kind/内容拒绝，相同请求只返回原结果；ACK 同时匹配 UUID 与摘要。

连接循环只交付有界后台请求，不承担实例检查或服务动作。结果先落入 SQLite FULL 事务，再补传；未 ACK 的结果不按 TTL 删除。面板在自己的事务中保存严格匹配的原结果后才确认，断连、重启和丢 ACK 不改变结果身份。过期或较旧部署的结果可以保存其历史事实并 ACK，但不推进当前健康/后代阶段；接收时间不能伪称观察时间。

检查、应用、intent 恢复与屏障建立共享同一 gate。检查或屏障遇到未完成 intent 只拒绝确认，交由独立的受管恢复流程处理，不在检查请求内重启业务。屏障必须先确认实际 checkpoint、健康及无未完成 intent，最低 revision 不大于当前 revision 且不小于已承诺下限；错误 expected 或未来下限不能改变设备边界。单事务持久化单调下限和成功收据，崩溃后不因丢失网络 ACK 而降回。

此后 apply、rollback、unfinished intent recovery 都禁止选择下限以下的配置。无法安全恢复时保留 intent、报告故障和可管理状态；不能撤销屏障或恢复已被排除的旧版本。该约束由声明能力的新 Agent 实现，旧二进制无法解释新屏障，不认证未经能力核对的降级恢复。

## 兼容与完整交付条件

设备一旦进入精确确认路径，后续缺失 capability 不能静默降回 revision 心跳补 healthy。新增控制表不改代理用户 ID、节点/链路凭据、订阅令牌/路径、历史部署 source 或计量账本。legacy 两跳配置和原资格流程保持独立，新混合路径尚需版本向量、来源解析、实际候选/切后探测与分阶段 publisher。

本步先完成全部协议、SDK、core、面板、界面、测试代码和文档，再冻结并统一验收。至少覆盖：错误实际文件/引用/进程、稳定 activation、丢 ACK 与重连补传、同 UUID 不同内容、错 ACK、过期/迟到结果、屏障事务与崩溃、禁止低于下限恢复、旧库/旧设备边界、链路窗口 error/pending 零写及恢复失效实体。实际 Linux 服务观察与受控替身的协议/事务证据分开；未运行的完整代理负载与混合通路继续待验。

四源码 CI 保持暂停，正式签署、发布、生产迁移及部署不由本步骤代办。完整 NodeQuality 许可、builder 与负载门禁保持，不能用该运行控制基础能力签收整体整改。

本步实际范围、失败修复与去重567/18结果见 [验收记录](../acceptance/runtime-checkpoints-and-chain-guards.md)。原两跳链路仍不冒称混合路径，完整验机及发布门禁保留。
