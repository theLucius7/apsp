# ADR 0048：使用 Agent 已上报地址的 Cloudflare DDNS

日期：2026-10-02。用户明确授权增加 DDNS，首个提供方为 Cloudflare，覆盖 MVP 中 DDNS 的排除项。

## 决策

- 用户在实现过程中进一步明确 DDNS 做成插件。实现物理位于 `plugins/ddns/panel/`，核心仅通过现有 plugins 注册桥装配路由与后台任务；前端位于 `web/src/plugins/ddns/`，API 使用 `/api/plugins/ddns/`。复用 `server_plugins` 的 `ddns` 标识按服务器显式启用，默认为未启用；禁用同时阻止周期和手动同步，保留规则和 DNS。（2026-11-05 起取消按服务器启用，规则自身的启用状态是唯一开关，见 [ADR 0110](0110-ddns-without-server-switch.md)。）插件目录标明执行位置为面板，无设备制品。DDNS 与代理插件、代理用户、配置编译及流量账本无关。Agent 继续使用现有 `telemetry.static.ip_addresses`，不新增 DNS 凭据、外部命令或探测服务。
- 规则绑定一台服务器、一个 Cloudflare Zone ID、一个完整域名及一个 A 或 AAAA 类型。双栈使用两条独立规则；同一 Zone、规范化域名和类型只能绑定一次。支持泛域名与 IDN 规范化、TTL、代理开关、检查间隔、暂停、立即同步及执行状态。域名/Zone/类型创建后固定，改变目标应另建规则。
- 每条规则独立保存 API Token；仅接受 API Token，不支持 Global API Key。管理员读接口仅返回是否已配置，留空编辑保留原值。沿用项目数据库凭据存储方式，不宣称静态加密；备份和数据库访问权限必须按包含秘密处理。不向 Agent、公开看板、日志或 Cloudflare 原始错误转发 Token。
- 只调用固定 Cloudflare HTTPS API，不接受自定义 URL、代理或重定向。请求、响应大小、总时长、规则数与并发均有上限。响应必须同时满足 HTTP 成功、Cloudflare `success=true` 和预期资源身份，错误使用本地固定说明。
- 接收 Agent 静态报告时记录接收时间，不将旧数据库内容迁移为新鲜 IP。设备需在线、未退役且报告不超过 10 分钟；仅选择对应家族的公网地址。多个地址时优先沿用仍在本轮报告中的上次成功地址，否则按地址排序选择首个；界面展示候选地址。公网探测仍遵守 Agent 原有开关及其最多 30 分钟的发现缓存，接收时间不等同于第三方探测时间。
- 同步前读取 Zone 与精确域名/类型的现有记录。无记录时创建；已有唯一记录仅在明确勾选接管或属于本规则时更新。创建记录带规则 UUID 标记，以恢复“远端创建成功、本地提交失败”的情形；已有记录使用 PATCH，只改 IP、TTL 和代理状态，保留备注与标签。同名同类型多记录、CNAME/NS 冲突及身份不符停止，不自动清理其他记录。
- 成功校验到一致时不写 DNS；公网地址缺失、报告过期、设备离线/退役时不请求提供方、不删除解析。暂停或删除规则只影响面板，不删除远端记录。数据库租约协调周期与手动请求，运行中编辑/删除拒绝，租约过期恢复；失败退避，429 尊重有界 Retry-After。外部 API 不具备跨数据库事务，超时可能已经在远端生效，下轮必须重新读取后对账，不能承诺恰好一次。
- 普通 TTL 为 60–86400 秒或 1（自动），代理记录强制自动 TTL；先不实现 Enterprise 的 30 秒 TTL。默认关闭 Cloudflare 代理，开启时提示 Cloudflare 代理只适用其支持的流量与端口。

## 参考与取舍

只参考本地 IPFlare `19bcf463a3dfdc3d13a9e61dd22bbf1a6fc68c80` 的行为思路（独立地址家族、变化对账、探测失败保留记录），实现按本项目已有 Rust/SQL/API 模式重新编写，不复制其 GPL 源码。首轮不包含 WAF 列表、自动删除 DNS、多提供方、DNS 传播保证或代理节点域名自动改写。

官方依据：[动态 IP](https://developers.cloudflare.com/dns/manage-dns-records/how-to/managing-dynamic-ip-addresses/)、[记录列表](https://developers.cloudflare.com/api/resources/dns/subresources/records/methods/list/)、[创建记录与 TTL](https://developers.cloudflare.com/api/resources/dns/subresources/records/methods/create/)、[局部更新](https://developers.cloudflare.com/api/resources/dns/subresources/records/methods/edit/)、[Zone 详情](https://developers.cloudflare.com/api/resources/zones/methods/get/)。

## 验证与边界

使用专用 PostgreSQL、私有回环 Cloudflare 替身和浏览器夹具验证权限、凭据脱敏、并发、恢复、重复记录、部分更新及无有效 IP 的零外呼。真实 Cloudflare Token、DNS 写入和传播属于配置后的实机验证，本地测试不冒称通过。CI 仍暂停；不因本任务完成而恢复，不影响原诊断门禁。
