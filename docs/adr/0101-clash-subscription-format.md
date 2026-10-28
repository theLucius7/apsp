# ADR 0101：订阅支持 Clash Meta（mihomo）格式

## 背景

apsp 的订阅此前只有 sing-box JSON 和 Reality 分享链接两种格式。成员大多使用 Clash 系客户端：Clash Verge、FlClash、Clash Meta for Android 和 Stash。这些客户端读不了上述两种格式，导入订阅地址后得不到节点。用户要求订阅链接与订阅格式都支持 Clash。仓库约定原先把「Clash 订阅」列为排除项，这次由用户明确授权覆盖。

## 决定

**新增 `format=clash`，输出 Clash Meta（mihomo）YAML。** 传统 Clash 核心不支持 VLESS、Reality、Hysteria2、TUIC 和 AnyTLS，所以目标格式是 Clash Meta。

**由同一组客户端出站转换，不另建节点模型。** 转换的输入与 sing-box 订阅相同，都是受管节点和外部节点的客户端出站，因此两种格式始终描述同一组已授权、已应用且健康的节点。

- **支持转换的协议：** VLESS（含 Reality、Vision）、VMess、Trojan、Shadowsocks（含 2022）、Hysteria2、TUIC v5、AnyTLS、SOCKS5 和 HTTP。
- **传输层与多路复用：** WebSocket、HTTPUpgrade、gRPC、HTTP/2 和多路复用都按对应字段转换。
- **传输兼容边界：** WebSocket、HTTPUpgrade 与 HTTP 代理的请求头仅转换字符串或单元素字符串数组，多值或空数组跳过；Trojan 的 HTTP/2、VMess/VLESS 未启用 TLS 的 HTTP 传输均跳过，不能把后者的 HTTP/1.1 语义改成 HTTP/2。

**不能等价表达的节点不写入，也不做近似。** 这类节点包括 Snell v6、Naive、前置跳板、仅 UDP、未知参数，以及非 QUIC 协议内联自签证书。它们会在文件开头的注释中列出名称与原因，提示改用 sing-box 格式。所有节点都无法表达时返回 409。

不改变连接身份的客户端调优参数会被省略，包括 TLS 版本范围、gRPC 保活时长和 Hysteria2 的 BBR 配置档。

**配置文件内容：**
- 「节点选择」手动组；
- 「自动选择」测速组，地址为 `https://www.gstatic.com/generate_204`，间隔 300 秒；
- 最终规则 `MATCH,节点选择`。

这与 sing-box 订阅只有一个选择器、全部流量走代理的语义一致。节点名称取显示名称，重名时自动编号，并避开组名和内置策略名。sing-box 完整客户端模板只适用于 sing-box，因此 Clash 格式不套用模板。

**同一订阅地址自动识别 Clash 客户端。** 请求不带 `format` 时，User-Agent 含 `clash`、`mihomo` 或 `stash` 即返回 Clash 格式；其他客户端仍得到原有的分享链接。显式的 `format` 参数始终优先。

**界面同步：**
- 管理端订阅对话框新增「Clash Meta（mihomo）配置」，可预览、复制和下载 `.yaml`。
- 成员门户在「我的订阅」下显示「Clash 订阅」地址。Nexus 兼容成员显示原订阅地址加 `?flag=meta`，这是保留的旧渲染器选择 Clash Meta 的参数。

**YAML 输出不新增依赖。** 用一个小型输出器写块状 YAML：

- 所有字符串都用双引号转义，控制字符及 U+2028、U+2029、U+FEFF 以 `\u` 形式写出；
- 注释行中的控制字符及 U+2028、U+2029、U+FEFF 替换为空格，节点名称无法插入额外的 YAML 内容；节点名称避让全部内置策略名与两个生成组名。

## 取舍

- 以 sing-box 出站为转换源，意味着 Clash 订阅能力受 sing-box 客户端编译结果约束。好处是只有一处授权与快照逻辑，不会出现两种格式节点集合不一致的情况。
- 自动识别依赖 User-Agent。以前 Clash 客户端拿到的是读不了的分享链接，所以这不改变任何原本可用的行为。
- 规则只保留最终规则，不内置分流规则，与 sing-box 订阅保持一致；需要分流的用户可以在客户端里覆写。
