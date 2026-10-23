# ADR 0050：使用节点私有凭证的正式 IP 查询

状态：实现与离线合同测试已编写，尚未运行本轮最终测试。日期：2026-10-02。

## 决定与依据

保留 check-place 聚合入口和七种响应数据库，也保留面板 AbuseIPDB 正式接口。新增独立的 Ipregistry、DB-IP 正式节点接口：provider 分别为 `ipregistry-node`、`dbip-node`，数据库分别为 `ipregistry-v1`、`dbip-v2`。每个来源都明确记录实际执行位置 `node`，不能将七个 `db` 参数视作七个独立来源。

[Ipregistry 身份认证](https://ipregistry.co/docs/authentication)提供私有 API key 的 `Authorization: ApiKey` 请求头；[正式端点](https://www.ipregistry.co/docs/endpoints)包含 `/` 出口查询和 `/{ip}` 单地址查询。[字段文档](https://www.ipregistry.co/docs/responses/ip-address-fields)声明 IP、连接、国家和安全布尔字段。[错误文档](https://ipregistry.co/docs/errors)说明授权与限流错误。适配只使用这些正式合同，不从首页提取 key。

[DB-IP 官方 v2 API 文档](https://db-ip.com/api/doc.php)声明 `https://api.db-ip.com/v2/{apiKey}/{ip|self}`，响应使用 `ipAddress`、国家、ASN、ISP、用途和安全/位置字段。私有 key 按官方合同位于 URL 路径，但含 key 的 URL 只经 curl 标准输入传入，错误和报告仅记录不含 key 的来源根地址。网页的 `data-api-key`、公共备用 key、试用 key 和公共 cookies 均不作为集成授权。

## 配置和凭证边界

仅读取节点固定文件 `/etc/apsp/nodequality-providers.json`。父目录必须为 root 管理且其他用户不可写，文件必须是 root 的 0600 或 0400 普通文件，不跟随符号链接，最多 8 KiB。顶层字段恰为 `schema`、`providers`，schema 为 `apsp.node-ip-quality-config.v1`。providers 仅允许上述两个 provider id；每项恰为 `api_key`、`authorized`，授权必须明确为 true。未提供某来源、授权未确认、格式错误或文件不安全时，该来源为未知，任务不读取网页材料或自动补凭证。

管理员通过自己的凭证管理流程写入配置；产品 API 不接收 key、自由 endpoint、命令或任意 JSON 适配器。私有 key 不出现在诊断任务、argv、环境变量、临时请求文件、日志、章节或最终报告。请求配置只通过 `/usr/bin/curl --disable --config -` 的 stdin 传入；`--disable` 必须是首选项，阻止继承 curlrc。清除代理和 APSP 环境变量，使用空 proxy、noproxy 全匹配，不覆盖 UA，不跟随重定向，不重试，不忽略 TLS 验证。每次最多 6 秒、响应最多 64 KiB，helper 总时限 75 秒。

## 节点出口、任务和翻译

专用管理员端点 `POST /api/servers/{id}/ip-quality/node-query` 只接收 `ip_version`，默认 both。面板从 Agent 已上报地址冻结 1–8 个不同公网 IP；Agent 对每个所选 IP family 先请求正式来源的 origin/self 端点，再校验返回 IP 的标准表示、版本和公网身份。只有与观察出口相同的冻结地址才继续查询；其他地址逐来源为未知。不会拿面板出口、传入目标或单纯 operator 声明当作已证明的节点出口。无所选版本地址则拒绝创建。

专用 NodeQuality r21 和 `diagnostic:nodequality-node-query` capability 才能启动此模式；主线默认 r19 包装器保留完整执行准入与日常正式 IP 自查，r18 历史制品契约不改；r20 是另一离线准备制品。r20/r21 从完整的主线 r19 严格派生，保留其准入记录、来源 helper 与日常接口，使用独立签名身份，不改写 r19 字节。任务仍由公共诊断服务负责排队、90 秒截止、64 MiB / 32 tasks 预算、设备隔离、重复提交、报告历史和确认式取消。Agent adapter 只调用 SDK，不增加插件持久化。完整验机门禁继续保持。

插件解释 `apsp.node-ip-quality.v1` 的独立 `ip_quality` 章节：固定 job UUID、来源、数据库、节点执行标识、冻结 IP 列表、观察出口、时间及 schema。只能接收登记字段；零数值和 false 保留。已提供字段类型错误或没有可信字段时，该来源为 schema_mismatch，不用其他有效字段掩盖错误。未配置、401/403/429、超时、响应不符均保持逐来源未知；错误展示由登记错误类别生成，不能回显上游错误的含凭证 URL。

独立章节先持久化，然后释放 job 锁再写 IP 缓存，避免 job/server 锁倒序。相同章节版本与相同内容的 Agent outbox 重放再次写缓存，能修复章节已经提交、缓存写入失败的情况；旧版本、同版本冲突和 complete 回退不覆盖新结果。缓存沿用 `(server, ip, provider, database)` 的上次成功、时间和有效期，失败、凭证删除及面板重建不清掉历史。

缓存事务取得 server 锁后，再以普通读取核对当前已提交章节的任务 UUID、revision 和原文 SHA256；较早请求的延迟缓存 continuation 与当前章节不一致时跳过写入，不取得 job 锁。公共服务在创建任务时已有 server 锁，插件在该事务内分配并保存 `node_query_generation`，明确连续任务的创建顺序；服务器只允许一个活动任务，但任务结束后的缓存 continuation 仍可能晚于下一任务，因此不能只依赖活动任务限制。专用缓存 payload 保存内部 generation/revision 顺序，较早任务即使与新任务在同一秒结束，也不能覆盖新缓存。无需数据库迁移；这些内部键不会进入公开 IP 查询响应，普通面板 IP 刷新继续沿用原有时间比较规则。同版本同原文重放仍可补偿 postcommit 缓存失败，逐数据库上次成功的 0 和 false 不变。

## 流媒体和验收边界

没有为 Disney+、YouTube Premium 或 ChatGPT 找到并核实可用的正式授权检测合同。本轮不提供借用公共授权材料或任意 operator JSON 断言的替代品；三个服务仍明确为未知，已有检查源码、来源与报告能力保留，原 r18 public-access policy 继续逐源禁止公共凭证自动使用。正式 IP 成功不代表流媒体解锁或完整验机可用。

#122 的独立验收允许无配置或拒绝为未知。本轮正式凭证适配和已有逐源拒绝策略可完成其源码整改，最终关闭以本轮测试收据为依据。#24 的 provider/database 区分和正规节点 API 已实现；其流媒体节点出口与适用故障实证不能由本轮 IP 接口合同冒充。专用 Debian 12 完整链和真实流媒体认证仍待环境与授权，不能声称完整恢复。

测试入口：`tools/test-nodequality-node-query.py`、插件 `node_queries/tests.rs`、`crates/panel/tests/node_ip_queries.rs`、`web/tests/node-ip-provider.mjs`，以及已有公共授权策略、诊断生命周期与字段显示套件。这里只记录测试代码已编写；没有使用真实凭证、访问外部 provider 或运行本轮测试。
