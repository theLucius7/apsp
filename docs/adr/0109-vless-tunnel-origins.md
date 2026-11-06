# ADR 0109：隧道后的 VLESS 源站

## 背景

Cloudflare 优选线路（HKT、TMNET、华为云、Azure HK/KR、Alice）的落地上，旧节点（xboard-node）以无 TLS 的 VLESS + WebSocket 监听一个本机端口，`cloudflared` 把隧道主机名（如 `ah.solve.ac`）转发到 `http://127.0.0.1:<端口>`。TLS 由 Cloudflare 边缘终止，客户端连 `edge.solve.ac:443`，SNI 和 Host 都是隧道主机名。

apsp 只支持 VLESS + Reality、SS2022 和 Hysteria2 的运行接管，快照工具对这些线路报 `transport_requires_conversion`、`vless_without_reality`，还把 WS 的请求路径 `path` 误判成外部文件（`external_files`）。用户希望 Alice 和 CF 线路也由 apsp 运行。

## 决策

- 编译器新增协议 `vless-tunnel`（`ProtocolConfig::VlessTunnel`）：隧道或 CDN 后的普通 VLESS 源站。
  - 服务端：`vless` 入站，无 TLS，使用节点已有的 WebSocket 或 HTTPUpgrade 传输设置（路径、提前数据），用户只有 UUID、不带流控。
  - **监听地址必须是回环地址**（`127.0.0.1` 或 `::1`）。没有 TLS 的监听只允许本机隧道连接器访问，由模型保证，不依赖各主机的防火墙。TCP 和 gRPC 传输拒绝。
  - 客户端：普通 TLS 连公开地址，SNI 为节点的 `sni`（隧道主机名），uTLS 指纹 chrome；WS 的 Host 头（HTTPUpgrade 的 host）未显式设置时取 `sni`，因为边缘按 Host 选择隧道。sing-box JSON、Clash 和 `vless://`（`security=tls`）分享链接同步生成。
  - Reality 参数必须保持默认值，Vision 只属于 Reality。
- 运行接管：`nexus_runtime_snapshot.py` 把“VLESS + HTTP 传输 + 无 TLS”的旧监听转换成 `vless-tunnel` 节点，`sni` 取旧节点地址（隧道主机名，必须是域名），**监听收窄为 `127.0.0.1`**，原配置的入站同步改写，其余字段保持旧节点生成的原样。传输里的 `path` 不再当作文件引用。面板预览时要求存档里的旧节点同样是 `tls=0`、相同传输与路径、地址等于 `sni`，否则报“监听或客户端身份与已存档资料不一致”。
- 迁移 0084 只给 `nodes.protocol` 的取值约束加上 `vless-tunnel`。
- 国内可达性监测不再探测隧道源站：原生 `vless-tunnel` 节点，以及导入目录中“VLESS、`tls=0`、WS/HTTPUpgrade”的旧节点。它们的入口在边缘，探测源站端口只会得到误报（Alice 的 `ah.solve.ac:21443` 此前一直被标成“服务不可用”）。
- 界面只显示该协议名称（“VLESS + 隧道（WS）”），新建节点暂不提供：源站需要同时配置隧道，单独创建没有意义。

## 取舍

- **放开 VLESS 无 TLS 的任意监听：** 和旧节点一致，但安全只靠主机防火墙（Alice 上确有 nft 规则只放行 lo）。模型直接要求回环更可靠，且 `cloudflared` 本来就连 `127.0.0.1`。
- **在源站也启用 TLS：** 需要为隧道主机名签发和续期证书，`cloudflared` 也要改用 HTTPS 源站，收益只是本机明文变密文。未采用。
- **把隧道配置（`cloudflared`）也纳入 apsp 管理：** 范围更大，留待之后。

## 影响与后续

- Alice（单节点主机）可以按普通流程准备并切换。
- 其余 CF 线路运行在已经接管的主机上（例如 TMNET、华为云的主线路已由 apsp 运行）。已接管主机的运行快照不能覆盖；2026-11-06 起用 `extend`（预览后提交，只追加新线路，已有条目不变）把它们加入，步骤见[迁移文档](../nexus-agent-migration.md#给已接管主机追加线路)。

## 验证

- `crates/compiler/tests/vless_tunnel.rs`：服务端、客户端、Clash、分享链接、Host 默认值、HTTPUpgrade；非回环监听、TCP/gRPC 传输、IP 作为 SNI、重复 UUID、非默认 Reality 参数都被拒绝；保留配置的用户按 UUID 重建，监听、传输或 TLS 与模型不符时拒绝；官方 sing-box 1.14.2 与 apsp 签名运行时都通过 `check`。
- `tools/test_nexus_runtime_snapshot.py`：转换、回环收窄、IP 地址、gRPC、相对路径、额外请求头、Vision 流控、路径不再误判为文件。
- `crates/panel/tests/nexus_runtime_takeover.rs`：存档一致时可预览和准备；存档为 TLS、换传输或换路径时报冲突。
- `reachability::targets` 单元测试：隧道源站不进入探测目标。
