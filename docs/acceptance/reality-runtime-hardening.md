# 自维护运行时 REALITY 行为验收

对应 [ADR 0102](../adr/0102-self-maintained-singbox-runtime.md)。2026-11-01 在隔离的 Docker 网络中执行，使用一次性密钥，只连接真实目标站和测试入站，不涉及生产主机。

## 对比对象

- 旧内核：旧 Agent 的同一依赖图（cedar2025 sing-box 分支 + `metacubex/utls` 1.8.4）构建的运行器。
- apsp 内核：已发布的 sing-box 1.14.2（`metacubex/utls` 1.8.7）。
- 补丁版：`1.14.2-apsp.1`，使用 `with_quic,with_utls,with_clash_api,with_v2ray_api` 标签构建。完整发布标签的构建未在本机执行。
- Xray-core v26.10.31，作为服务端行为参考。
- 直连真实目标站，作为基准。

## 未认证探测（目标 itunes.apple.com）

| 场景 | 目标站 | 旧内核 | apsp 内核 | 补丁版 | Xray |
| --- | --- | --- | --- | --- | --- |
| 完整 TLS 握手与 HTTP | 正常 | 相同 | 相同 | 相同 | 相同 |
| 明文 HTTP | RST 0 秒 | FIN 0 秒 | FIN 0 秒 | RST 0 秒 | FIN 0 秒 |
| 空闲 | FIN 34 秒 | 相同 | 相同 | 相同 | 相同 |
| 残缺 ClientHello | FIN 20 秒 | 相同 | 相同 | 相同 | 相同 |
| 完整 ClientHello 后等待 | FIN 20 秒 | 超过 150 秒不关 | 超过 150 秒不关 | FIN 20 秒 | FIN 20 秒 |
| 错误 SNI 后等待 | FIN 20 秒 | 45 秒内不关 | 45 秒内不关 | FIN 20 秒 | FIN 20 秒 |
| TLS 1.2 ClientHello 后等待 | FIN 20 秒 | 20 秒内不关 | 45 秒内不关 | FIN 20 秒 | FIN 20 秒 |
| ClientHello 后半关闭 | FIN 0 秒 | FIN 19 秒 | FIN 19 秒 | FIN 0 秒 | FIN 0 秒 |

响应记录长度在各组之间一致。目标 www.jd.com 上，apsp 内核与补丁版的对比结论相同：明文 HTTP 和半关闭只有补丁版与目标站一致。

## 认证连接

服务端发往客户端的记录，握手结束后：

| 对象 | 记录长度 |
| --- | --- |
| 目标站直连（ALPN h2） | `69, 298, 298` |
| 旧内核、apsp 内核 | `69`，之后直接是用户数据 |
| 补丁版 | `69, 298, 298`，之后是用户数据 |

Mihomo 和 sing-box 1.14.2 客户端分别以普通与 Vision 模式连接补丁版，HTTPS、HTTP/1.1、明文 HTTP 和 5 MB 下载全部成功，并发 12 个请求全部 200，服务端无错误日志。测试用的 Mihomo 连不上 Xray v26.10.31 服务端，这是没有换用 Xray 的原因之一。

## MPTCP

客户端提供 MP_CAPABLE 时，旧内核的 SYN-ACK 带 MPTCP 选项，apsp 内核、补丁版和 Xray 不带。原因是旧二进制自行编译时没有上游 `release/LDFLAGS` 里的 `multipathtcp=0`。

## 其他验证

- `go test -race -run Reality`：解析、FIN、RST、半关闭 4 项全部通过。
- `apply.py` 在干净容器中两次得到相同提交，`-mod=readonly` 可构建，`vcs.modified=false`。

## 结论的边界

以上证明补丁版在这些测试项上与目标站一致，不证明防火墙不再封锁。旧内核与 apsp 内核在这些项上基本没有差异，因此它们也不能解释 2026-10-26 的集中封锁。
