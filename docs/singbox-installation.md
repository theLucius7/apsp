# sing-box 服务配置与运行生命周期

Apsp 面板管理代理业务；服务器只需要接入一个一体化自带能力的 Apsp Agent（[ADR 0141](adr/0141-self-contained-agent-and-desired-service-config.md)）。Agent 二进制内置对应平台真实构建的 `sing-box` 运行时（`1.14.4-apsp.2`）与受控诊断能力（Linux 正式发布构建强制内嵌 `sing-box`、`-r22` `nodequality`、`ipquality`、`network-workbench` 四组件闭环及原生 `tcp-probe`），不再需要面板维护独立制品目录或管理员手工点击“安装 sing-box”。部署维护者收录签名 Agent 版本（`/api/system/agent-releases*`）与日常管理员配置服务严格分开：代理用户、节点、链路、订阅和套餐都在「网络服务」工作区管理；系统管理员和服务器监控仍在原有系统/服务器页面。

```mermaid
flowchart LR
  A[管理员] --> P["面板网络服务<br/>节点、代理用户、链路、授权与订阅"]
  P --> C[当前有效目标配置与修订]
  C --> G["服务器一体化 Agent<br/>（内嵌 sing-box）"]
  G --> S[设备运行时对账与服务定义]
  S --> B[独立 sing-box 运行时]
  B --> G
  G -->|应用确认、健康与业务流量| P
```

## 操作顺序

1. 在“服务器”创建并接入设备。Agent 的安装命令只用于接入设备；未配置任何启用节点或链路的服务器保持 `unconfigured`（纯监控模式），不提取、不启动 `sing-box`。
2. 直接在「网络服务 › 节点」为目标服务器创建受管节点，或在「链路」视图用“创建链路”将该服务器选为入口/中转/落地跳（允许在设备离线或旧 Agent 时先保存排队）。公开地址必须是客户端实际可访问的入口。
3. 创建代理用户。把普通节点、链路和外部节点放进策略组，再把策略组分配给代理用户。需要流量、重置周期和到期限制时另分配套餐。
4. 在线的一体化 Agent 收到有效配置后，自动幂等补齐受管系统服务定义、提取校验内置 `sing-box`、应用配置、执行健康检查并回报。在服务器详情的「网络服务」分页（`/#/servers/{id}/services`）或「节点 › 运行时」视图（`/#/nodes?view=runtimes`）确认目标配置已应用且健康（`ready`），再使用代理用户的订阅。保存资源与实际代理可用分别显示。

`configured`（运行需求）仅由当前有效服务配置（未删除且启用的节点、启用的链路跳，或已激活的兼容切流目标）决定；历史 `deployments` 记录或默认 `ServerProfile` 调优项不单独制造运行需求。删除或显式禁用全部节点与链路、服务器退役或 `--monitor-only` 优先停止受管运行时，宿主机或 Agent 重启后不会因历史安装目录反开。本地组件上报（`static_info.components`）不代表可立即占端口启动：实际应用仍核验权限、端口与服务归属，遇未知进程或旧控制面拒绝夺取并置 `failed`。

## 异常与恢复处理

当目标配置修订未变但运行时异常退出或宿主机重启导致脱离受控实例时，Agent 在满足恢复代数门禁的前提下自动执行受控重启恢复并重新认证实例；当 Agent 整包升级携带新版内置运行时时，对账一并比较本地已激活组件的摘要身份并在试用（trial）期内受控切换、失败整包回退。当前连接的设备若仍为旧版非一体化 Agent，状态明确提示 `upgrade_required`（需升级 Agent），旧已运行配置是否健康继续由 `current_healthy` 如实反映。

修改节点或授权后，面板合并发布配置，设备离线期间保留待办，重连后继续。新配置失败时自动回滚至上一已验证修订并重验健康；页面不宣称所有长连接更新无中断。

已有用户 ID、节点密钥、授权、旧 `/sub/{token}` 和历史流量继续可用。配额周期与计数器重置分开；详见[策略与套餐](singbox-groups.md)。维护者收录签名 Agent 发布及服务器接入见[部署说明](deploy.md)。

包含 Linux 四组件内嵌闭环的新版一体化 Agent 已在工作树完成实现，正式 GitHub Release 签署发布与实机升级验收待按发布流程单独执行。早期里程碑的独立验收记录见[双端插件安装与恢复](acceptance/singbox-plugin-installation.md)，区分源码测试、专用节点实际安装和仍未验证的公网/生产范围。

部署与模块状态表属于数据库基线 `crates/panel/migrations/0001_current.sql`，见[数据库基线](database-baseline.md)。业务与架构决策见 [ADR 0061](adr/0061-singbox-plugin-lifecycle.md) 与 [ADR 0141](adr/0141-self-contained-agent-and-desired-service-config.md)。
