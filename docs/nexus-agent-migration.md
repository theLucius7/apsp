# Nexus 服务器向 apsp Agent 迁移

本轮已授权实际服务器迁移。主机资产迁入、Agent 接入、代理运行时接管是三个独立状态。`nexus_agent_migration.py` 完成只读快照、主机合并、兼容差异预览和可续跑的主机资产导入；它不安装 Agent、不停止旧服务、不签收节点切换。

## 保存现状与迁入资产

Nexus 的 `v2_server_machine` 是协议运行实例，不等于物理服务器。同一 `v2_infra_host` 的多个实例只创建一个 apsp 服务器，保留机器、节点和主机的对应关系。不能按节点逐个安装 Agent，否则同机端口和计量会冲突。

在可信维护机上运行：

```sh
python3 tools/nexus_agent_migration.py export /private/nexus.sqlite /private/infrastructure.json --source-id nexus-production
python3 tools/nexus_agent_migration.py preview /private/infrastructure.json --release-metadata /private/release/release.json --output /private/readiness.json
python3 tools/nexus_agent_migration.py stage-hosts /private/infrastructure.json /private/host-receipt.json \
  --confirmed-digest '<预览的完整摘要>' --env-file /private/apsp.env --panel-api http://127.0.0.1:8080
```

快照含节点原密钥、端点、路由和证书配置，应保存在权限 700 的目录，文件写入权限为 600。SSH 密码、私钥、机器令牌、成员身份和实时遥测不导出。预览只输出 ID、数量和固定原因，不输出端点或密钥。`release.json` 仅用于定位身份差异，预览不声称完成验签、下载、安装或功能验收。

导入复用正常管理员 API；密码从既有私有环境文件读取，不能通过外部 HTTP 明文发送，不跟随携带认证的重定向。按来源与原主机 ID 保存稳定标签，保存逐台收据。请求成功但响应丢失时，重跑从面板标签识别已创建服务器，不重复创建。重复标签、失踪的已导入主机和快照变化均停止，不覆盖管理员后续修改。只运行一个维护任务，并在恢复时使用同一份收据。

费用保留原金额、币种及周期，不四舍五入；日期按原日期映射为 UTC 当日零时，不自动续费。来源没有说明 GB 换算口径和流量重置日时，将原值保留在私有快照，不推测有效限额。停用备注、供应商、NAT、端口映射等原始资产字段也保留在快照；目标支持的字段才写入资产设置。新资产暂时关闭离线通知和自动升级；真实静态信息、在线状态和遥测由设备接入后上报。

## 正式切换前的缺口

2026-10-24 查询 GitHub 公开 Release，当前只有 `agent-v0.3.0`。其已签 `release.json` 的来源为旧仓库身份，Agent 二进制名称为旧名称；当前校验器要求现项目身份。不能修改已签 JSON 或跳过校验，把旧资产伪装为新发布。旧适配器在 `native::listen_addresses` 明确只接受 VLESS；面板支持 Hysteria2/SS2022 不代表已签 Agent 支持。

实际接管还必须完成以下工作：

- **节点与身份**：原节点私钥、公钥、short ID、端口、公开地址、UUID/协议密码原样采用。当前普通创建接口会生成新 Reality 密钥；不能拿它直接代替保留凭据的迁移接口。当前受管编译器固定八位 short ID，与既有不同长度配置不兼容。
- **路由与协议**：保留 VLESS flow、多路复用和实际生效的加密设置；保留 Hysteria2 原认证、证书续期及跳端口规则；保留 SS2022 原服务端及用户密钥。包含外部落地、条件分流、detour 或本机额外路由文件的机器，必须将完整生效配置转换并校验后一起切换。
- **计量**：原节点存在非 1 倍倍率。兼容计量当前直接使用旧总量，新 Agent 账本不会自动计入；需实现单机交接检查点、倍率和周期合并，确保不丢量、不重复计费、不通过 epoch 重置套餐。
- **授权和订阅**：原权限组、套餐、隐藏状态、暂停与到期继续生效；旧域名的原订阅路径、令牌及返回格式保持。旧订阅后端与新 Agent 可以分阶段保留，但同一节点只允许一个配置发布方和计量来源。
- **设备接入**：逐台确认实际主机、SSH 指纹、权限、服务管理器、端口冲突、证书及回退配置。旧资产的关机备注不能替代当前观测；无法访问的主机保留未接管状态。

完整切换顺序：冻结该机器配置 → 保存可恢复配置及计量 → 验签接入兼容 Agent → 准备完整候选运行时 → 停止该机旧发布/计量 → 最终计量交接 → 应用并确认新运行时 → 验证旧客户端协议和订阅 → 记录接管结果。任一步失败保留原身份与服务恢复能力，不以页面“保存成功”代替节点可用。

生产发布继续遵守 [签名发布规则](release.md) 和 [临时 CI 暂停](../AGENTS.md)。新签名 Agent、前置能力验收与发布流程未准备完成前，资产导入结果不能标记为 Agent/节点迁移完成。测试与现场状态见[本轮验收](acceptance/nexus-agent-migration.md)。
