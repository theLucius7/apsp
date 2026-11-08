# ADR 0075：节点高级参数与后台刷新呈现

状态：已接受。

## 背景

用户要求参考成熟项目整体补齐节点协议、TLS、传输、监听及编辑回显，并反馈多个后台页面周期性闪动。本轮参考 S-UI 的服务端/客户端参数分组和 3X-UI 的按协议展示；字段以项目固定的 sing-box 1.14.2 为准，不复制 Xray 配置。

## 决策

- 沿用 `NodeSettings` 的 JSON 存储，新增字段有兼容默认值，不改变节点身份、凭据、授权与账本。省略参数组保留原组；提供参数组则完整替换，可空标量明确支持清空。
- 补齐 TCP 保活、TLS 版本与握手超时、Reality 时间容差及 flow、可用的 VLESS 传输；默认保持原来的 TCP + Vision。传输、flow、分享链接、订阅与受管链路使用同一确定性编译规则，禁止静默丢弃不兼容选项。
- 补齐 HY2 BBR 与字符串伪装响应、TUIC UDP 转发模式、AnyTLS 填充、Snell v6 模式与复用、SS2022 多路复用及 UDP over TCP。限制数量、大小、时长和互斥组合，不开放任意原生 JSON 或任意服务器文件路径。
- 服务端选项不进入客户端配置。Naive 客户端不支持通用 TLS 版本等字段，必须按固定运行时能力省略；该界面明确区分仅服务端参数。秘密留空保留，替换证书必须成对提供，回显不含私钥与混淆密码。
- 被存活混合链路引用的连接配置继续锁定，面板返回锁定信息；界面提前显示关联链路，仍允许修改名称与启用状态。
- 后台轮询保留现有内容，区分首次加载、主动刷新和短暂后台读取。提交有效性在请求开始时立即失效；只有展示可延后，不能用延迟放宽陈旧快照写入。慢请求、真实失败和资源消失必须显示，错误状态不假装成功。

## 验证与边界

实施后验证编译确定性、方向性、非法组合、旧数据默认值、API 清空/保留语义、浏览器回显和跨页刷新。官方运行时检查与真实端到端握手分别记录；配置解析成功不等于所有客户端实机兼容。只使用隔离数据库和回环测试服务，不发布或部署；CI 继续暂停。

## 参考

- [S-UI 前端](https://github.com/alireza0/s-ui-frontend)，参考提交 `4669846caf9678df6cd1b349b64c9d8e632a1c35`。
- [3X-UI 节点表单](https://github.com/MHSanaei/3x-ui/blob/05a083eaef0dbbae97256f6dfddab056822b36b6/frontend/src/pages/inbounds/form/InboundFormModal.tsx)。
- [sing-box 1.14.2 配置类型](https://github.com/SagerNet/sing-box/tree/v1.14.2/option)。
- [监听参数](https://sing-box.sagernet.org/configuration/shared/listen/)、[TLS](https://sing-box.sagernet.org/configuration/shared/tls/)、[多路复用](https://sing-box.sagernet.org/configuration/shared/multiplex/)。在线文档可能包含更高版本，实施以固定 tag 源码及原生检查为准。
