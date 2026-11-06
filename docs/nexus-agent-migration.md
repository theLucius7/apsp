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

### 追加导入后新增的线路

来源导入后，旧面板又新增了主机或线路（例如新开的机器）时，不重新导出整份快照：同一来源只接受一份完整快照，换一份会被拒绝覆盖。改用追加，只导出指定节点：

```sh
python3 tools/nexus_node_inventory.py append /private/nexus.sqlite /private/append-<日期>.json /private/append-<日期>-receipt.json \
  --source-id <原来源编号> --node <节点编号> [--node …] \
  [--create-server] [--link-server <原主机编号>=<apsp 服务器编号>] \
  --env-file /private/apsp.env --panel-api http://127.0.0.1:8080
# 核对输出的预览后，加 --confirmed-digest '<预览摘要>' 重跑同一命令
```

- 快照第一次运行时写入指定文件，之后的预览、确认和重试都读同一份文件，摘要不会变化。
- 只能追加有自己 machine 的节点。别名节点（`parent_id` 非零、没有 machine）由父节点的监听承载，导入父节点即可。
- 节点所在主机必须在旧面板有基础设施记录，并且 machine 已关联到主机。apsp 侧按来源标签找服务器，不信任请求里的编号：`--create-server` 为还没有服务器的主机按原资料新建；`--link-server` 给已经接入的服务器补上该主机的标签。
- 服务端逐个检查：节点都必须是新的；同一请求重试时，节点都已存在且主机映射不变视为已导入；新旧混合或归属不同一律拒绝。追加不改原导入的摘要，只增加节点计数，并记录管理审计。

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

导出时机器令牌不读取，`token_env` 引用一律填占位值；`custom_config` 指向的文件缺失时直接报错，不像旧节点那样静默跳过。汇总只写原因代码：`protocol_requires_conversion`（Hysteria2 等需要证书文件与跳端口规则）、`transport_requires_conversion`、`external_files`、`global_settings_require_reconciliation`、`reality_short_id_count`、`brutal_requires_review`、`tunnel_host_invalid`（隧道源站的旧地址不是域名）、`effective_configuration_missing`（未取得主机文件）等。被阻止的主机继续由旧服务运行。

无 TLS 的 VLESS + WebSocket/HTTPUpgrade 监听（Cloudflare 优选线路的源站）转换为 `vless-tunnel` 节点（[ADR 0109](adr/0109-vless-tunnel-origins.md)）：`sni` 取旧节点地址（隧道主机名），监听收窄为 `127.0.0.1`。切换前确认该主机的隧道连接器转发到 `127.0.0.1:<端口>`；如果连的是其他地址，先改隧道配置。主机已经接管时，用下文“给已接管主机追加线路”加入。切换后用 `PATCH /api/plugins/sing-box/nexus-nodes/{库存编号}/public-endpoint`（`{"public_host":"edge.example.com","public_port":443}`）把公开入口改成成员实际连接的边缘地址；该接口同时更新目录、节点的公开地址和公开端口，监听端口不变。

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

### 给已接管主机追加线路

主机已经由 apsp 运行（或已准备）后，旧面板在这台主机上新增的线路（例如 Cloudflare 优选线路的隧道源站，由单独的 `xboard-node-cf` 服务提供）不能用 `prepare` 加入：一台主机只有一份运行快照，不能覆盖。用 `extend`：

1. 用 `tools/nexus_node_inventory.py append` 把新线路导入到这台主机（见上文“追加导入后新增的线路”）。
2. 只为新线路生成快照（导出与 `build` 同上，清单只含新线路），得到只含新线路的 `host-<ID>-snapshot.json`。
3. `nexus_runtime_takeover.py extend-preview --server <ID> --digest <当前快照摘要> --input <新线路快照>`：面板把新线路并入已存的快照，只对新线路做与准备时相同的存档核对，要求合并后覆盖这台主机已导入的全部线路（已下线的线路作为历史留在快照里），并在随后丢弃的事务中编译出候选配置（含新线路的用户），写入 0600 输出文件。用同版本签名运行时检查候选；监听、DNS 或路由选项与原有线路冲突时在这一步报错，不发布。
4. 在主机上停掉占用新监听的旧服务（如 `xboard-node-cf`），在十分钟内执行 `extend --extended-digest <预览中的合并摘要> --stopped-service <服务名>`。面板保存合并后的快照（加密，旧快照停用）、新增原生节点与策略组成员；主机正在运行时重新发布，Agent 照常应用新修订（先读取当前运行时的终止计数）。
5. 用真实客户端验证新线路和原有线路。新线路有问题时，用下文的 `retire` 下线它，再启动旧服务；原有线路不受影响。

`extend` 只追加，不改变快照中已有的条目；要修改已有线路，仍需按原流程处理。

### 下线已接管主机上的单条线路

主机已接管后，某条线路不再使用（例如出口失效，或入口端口在国内被封），用 `POST /api/plugins/sing-box/nexus-nodes/{id}/retire` 下线，请求体为 `{"legacy_node_id": <原节点编号>}`。`{id}` 是 apsp 的库存编号，原节点编号必须一致，防止输错编号删掉另一条线路。

- 删除这条线路的接管记录和库存记录，停用它单独保存的原配置凭据，原生节点只标记删除，保留已记录的流量；主机正在运行时会重新发布，Agent 应用后该端口不再监听。
- 加密的运行快照保持不变，作为历史记录；发布时跳过已经没有库存记录的条目。所以用旧快照重新预览会提示节点不全，这是预期结果。
- 主机只剩最后一条线路时拒绝下线；整台主机停用请暂停接管。
- 顺序：先在 apsp 下线，等 Agent 应用新配置后，再删除旧面板中的节点。兼容桥要求 apsp 接管的每个节点在旧面板都存在，先删旧面板会使所有已接管主机的心跳中断，线路从订阅中消失。只想先让成员看不到，可以先在旧面板隐藏该节点。
- 下线和 Agent 应用之间这段时间内的流量不再计入旧面板。

迁移节点的用户授权来自策略组（[ADR 0115](adr/0115-original-permission-groups-as-policy-groups.md)）：每个原权限组对应一个「原权限组 · 组名」策略组，接管时线路自动加入；策略组不为迁移线路生成新 UUID 与密钥，下发的仍是用户原来的身份。需要调整谁能用哪个节点时，编辑策略组或用户所属的策略组；修改套餐不再改变线路。

## 2026-10-26 回滚后的重新迁移

上次切换后节点集中被封，已全部回滚到旧运行时。现有证据更支持一次集中封锁，而不是 apsp 内核被单独识别，详见 [ADR 0104](adr/0104-self-maintained-singbox-runtime.md)。重新迁移前：

1. 发布并导入包含运行时 `1.14.2-apsp.1` 的签名版本，把 Agent 升级到 0.3.4。
2. 用公网端点接口更新换过 IP 或端口的节点，否则目录和可达性检测看到的都是旧地址。
   导入后在旧面板新增的权限组用 `PATCH /api/plugins/sing-box/nexus-nodes/{id}/groups` 补上，并用 `compare-users` 确认没有缺用户。
3. 开启国内可达性检测（ADR 0105），记录每台主机切换前的状态。已被封的主机先换 IP。
4. 每次只切一台，默认间隔 24 小时；观察国内可达性和真实流量后再继续。先切没有历史包袱的主机。
5. 优先把 REALITY 入口放到 443 端口，并换掉命中风险提示的目标。这会改变客户端配置，需要单独安排。
6. 旧运行时开启了 MPTCP（自编译时没有带上游的 `multipathtcp=0`），会暴露 CVE-2026-98432 的已知利用路径；新运行时已关闭。迁移完成前，旧主机应尽快升级内核。

### 2026-11-02 就绪检查

逐台只读登录 16 台在用服务器，并从国内探测节点端口。结论：还不能切换，下面是当时的状态。

- 所有节点仍由旧发布器承载，没有任何主机运行 Agent 或 apsp 运行时。磁盘上保留的 Agent：10 台 0.3.2，4 台 0.3.3，2 台从未安装，需要全新接入。
- 4 台主机从国内不通，公开的 20 个节点条目中有 8 个受影响，近 7 天几乎没有流量：2 台整 IP 不通，1 台节点端口和 443 不通，1 台全部节点端口不通而 22 端口正常。这几台要先换 IP，否则切换后无法验证效果。
- 面板把 3 台主机标记为已接管，实际却运行旧发布器时，不能先启动 Agent，否则新运行时会和旧服务抢端口。先核对面板状态。
- 1 台 OpenRC 小盘主机可用空间约 109 MB，放不下 92 MB 运行时、15 MB Agent 和 32 MB 压缩包，还要留 64 MB 余量，迁移前要先腾空间。这台主机排在最后。
- 多台主机的 80、443 等端口由其他服务占用，还有独立运行的 sing-box 和中转服务。Agent 只接管面板记录的入站，切换时不能动这些服务。

`pause`、`activate` 的 `stopped_at` 用面板所在机器的时钟取值。个别主机时钟不准，主机时间比面板快时会被判定为未来时间而拒绝。

**回滚后的主机重新切换时，要先拉起上一个运行时。** Agent 替换运行时之前，必须从上一个已应用的运行时读取最终流量计数，读不到就取消这次应用，并每隔约 20 秒重试。回滚时旧运行时已停止，所以直接激活会一直报 `cannot capture terminal counters`，节点在此期间不可用。正确顺序是：停旧发布器并确认端口释放 → 启动 `apsp-singbox@main` → 激活。Agent 读到计数后会自行换成新版本。2026-11-02 首台主机按这个顺序切换，中断 47 秒。

切换顺序：先切有真实国内流量、没有其他服务、出事损失可控的主机，不先切流量最大的主机。切换时不改目标站和端口，出问题才分得清原因；目标站和端口之后单独调整。
