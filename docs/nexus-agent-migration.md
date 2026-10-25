# Nexus 服务器向 apsp Agent 迁移

本轮已授权实际服务器迁移。主机资产迁入、节点信息迁入、Agent 接入、代理运行时接管分别记录状态。`nexus_agent_migration.py` 完成只读快照、主机合并、兼容差异预览和可续跑的主机资产导入；`nexus_node_inventory.py` 将原节点信息保存到 apsp 并展示在节点页。两者均不安装 Agent、不停止旧服务、不签收节点切换。

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

## 导入节点信息

部署包含迁移 0071 的面板后，使用同一份基础设施快照和已完成的主机收据。导出时重读源库，若基础设施摘要已变化则停止，先核对来源修改。输出同时包含原权限组和路由记录；不转换原节点 JSON，不生成新密钥。

```sh
python3 tools/nexus_node_inventory.py export /private/nexus.sqlite /private/infrastructure.json /private/host-receipt.json /private/nodes.json
python3 tools/nexus_node_inventory.py preview /private/nodes.json /private/node-preview.json \
  --env-file /private/apsp.env --panel-api http://127.0.0.1:8080
python3 tools/nexus_node_inventory.py import /private/nodes.json /private/node-receipt.json \
  --confirmed-digest '<节点预览返回的摘要>' --env-file /private/apsp.env --panel-api http://127.0.0.1:8080
```

在“代理服务 → 代理节点 → 节点库”的 Nexus 节点区域查看、搜索和按协议/服务器筛选。原编号、主机、实例、端口范围、流量倍率、启用和隐藏状态保持；详情展示权限组及白名单协议参数。原始密钥、证书和自定义路由一起加密保存在现有凭据库，随面板数据库备份；备份仍依赖独立保存的原凭据密钥环。不能仅备份数据库而丢弃密钥环。

导入需要所有者登录及近期验证。API 检查稳定来源标签，不信任单独传入的服务器编号。同快照重复提交不重复创建，不同快照拒绝覆盖。导入和加密凭据在一个事务中提交，响应丢失时使用原文件和摘要重试。节点明确显示“待 Agent 接管”，不会自动写入运行时或使新订阅出现尚未接管的节点。设计见 [ADR 0094](adr/0094-nexus-node-inventory.md)。

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
## 运行接管（ADR 0095）

库存导入后，通过 `tools/nexus_runtime_takeover.py` 的 `preview`、`prepare`、`candidate`、`activate`、`pause`、`discard` 操作完成运行接管。所有操作使用面板管理员 API，除 `preview` 外都在调用前自动完成管理员再次验证；正文配置和候选只写入指定的 0600 文件，不打印密钥。示例地址使用 `https://panel.example.invalid`。

```sh
python3 tools/nexus_runtime_takeover.py preview --panel http://127.0.0.1:8080 --environment /private/apsp.env \
  --input /private/runtime/host-<ID>.json --output /private/runtime/host-<ID>-preview.json
python3 tools/nexus_runtime_takeover.py prepare ...同上... --output /private/runtime/host-<ID>-prepared.json
python3 tools/nexus_runtime_takeover.py candidate --server <apsp 服务器 ID> ... --output /private/runtime/host-<ID>-candidate.json
python3 tools/nexus_runtime_takeover.py activate --server <apsp 服务器 ID> --input /private/runtime/host-<ID>-cutover.json ...
```

运行快照为 `{source_id, server_id, nodes:[{legacy_node_id, preserved:{node, configuration}}]}`。`node` 为编译器节点模型，初始 ID 为原节点编号，`users` 为空。`configuration` 为完整单节点原生配置，包括实际使用的本地覆盖、TLS 证书及出站。SS 服务端密钥来自旧面板 `buildNodeConfig`，不能调用新建节点随机生成；用户密钥由发布器按原 UUID 派生。

### 由旧节点自身生成快照

不要手写或模拟旧节点的配置。旧节点（xboard-node）在内存中生成 sing-box 配置，不落盘；`tools/nexus-legacy-export/` 的导出测试直接调用它自己的 `buildConfig`、弱类型解码和本地 `custom_config` 合并，得到与线上完全一致的生效配置。手工推导容易出错：例如旧面板把 Reality 握手端口存成字符串 `"443"`，旧节点会弱类型转换，手写候选则会被面板拒绝或让新运行时启动失败。

```sh
# 1. 在旧面板容器内只读导出每个节点的发布配置和当前可用用户（含成员 UUID，保存为 0600）
python3 tools/nexus_runtime_snapshot.py export-legacy --container <旧面板容器> /private/runtime/legacy.json
# 2. 按物理主机生成导出请求
python3 tools/nexus_runtime_snapshot.py request /private/nodes.json /private/runtime/legacy.json /private/runtime
# 3. 每台主机：只读取回 /etc/xboard-node（或 /etc/nexus-agent）后，在断网容器中按原路径只读挂载，
#    用该主机实际运行版本的 xboard-node 源码执行导出测试（用法见 apsp_export_test.go 文件头）
# 4. 生成快照与汇总；只有整台主机全部节点可转换才写出 host-<ID>-snapshot.json
python3 tools/nexus_runtime_snapshot.py build /private/nodes.json /private/runtime
```

导出时机器令牌不读取，`token_env` 引用一律填占位值；`custom_config` 指向的文件缺失时直接报错，不像旧节点那样静默跳过。汇总只写原因代码：`protocol_requires_conversion`（Hysteria2 等需要证书文件与跳端口规则）、`transport_requires_conversion`、`external_files`、`global_settings_require_reconciliation`、`reality_short_id_count`、`brutal_requires_review`、`effective_configuration_missing`（未取得主机文件）等。被阻止的主机继续由旧服务运行。

### 切换步骤

1. 获取完整当前配置和回退备份，验证同机所有实例和监听；按上节由旧节点生成快照，逐一处理不能合并的配置，不能删掉冲突字段后继续。
2. 使用正式签名 Agent 接入主机，先保持监控模式。原服务继续运行，准备操作不发布。
3. `candidate --server <ID>` 导出私有候选，使用已独立验签的同版本 sing-box 执行配置检查；使用原客户端凭据验证候选的隔离实例。再用 `nexus_runtime_snapshot.py compare-users <快照> <候选> <legacy.json> <报告>` 按监听端口逐节点比对候选用户与旧节点当前可用用户：缺少或多出任何用户都以退出码 2 停止，报告同时统计带限速/设备数限制的用户（apsp Agent 不执行这两项限制）。保留摘要和结果。
4. 确认兼容桥已更新到支持原子流量投递的版本并健康。原订阅读取、SS/Reality/Hysteria 握手和相关出口必须有切换前基线。
4a. 设备数限制（[ADR 0099](adr/0099-nexus-device-limits.md)）：兼容桥须在 120 秒内同步过旧面板的设备数限制；主机有设定限制的成员时，Agent 须声明会话控制能力（0.3.2 起、systemd 245 起，运行时服务单元写入 `apsp-runtime` 命名空间）。具备该能力的主机，候选配置包含 info 级运行时日志和回环 `127.0.0.1:18086` 控制接口；先确认该端口未被占用。`compare-users` 报告中的设备数限制由此执行，速度限制仍不执行。
5. 停旧发布器并排空终末计量，确认旧监听释放。`activate` 输入 `{expected_digest, config_sha256, backup_sha256, stopped_at, stopped_services}`；三个摘要分别对应准备收据、当前候选和主机备份，停用时间为 Unix 秒。配置或授权变化导致摘要不一致时先恢复旧服务，再重做候选。
6. 等待 Agent 应用健康回执，再使用同一旧客户端验证实际入口、出口及订阅。任何失败先停新运行时，提交 `pause` 并恢复旧配置；不能仅依据 API 接受就标记成功。

旧订阅兼容后端及重置调度继续运行。未经正式签名发布、网络不可达、证书续期未承接的主机保持原运行状态。

准备之后发现快照需要修正（例如补齐旧服务的实际生效字段），且该主机从未启动、没有任何部署时，用 `discard --server <ID> --input <{"expected_digest":…}>` 撤销准备，再重新预览和准备。启动过的主机不能撤销，只能暂停；暂停后旧服务的恢复见上文第 6 步。

迁移节点的用户授权只来自套餐与原权限组映射。策略组和直接授权会生成新 UUID 与新密钥，对迁移节点直接拒绝；需要调整谁能用哪个节点时，修改套餐映射或原权限组。
