# 代理协议与证书

在“sing-box 插件 → 代理节点”创建节点，选择协议并授权给代理用户。使用新增协议前需升级面板和 Agent。每台服务器仍运行一份完整配置。无授权节点不监听端口，也不申请证书；变更按原有 5 秒窗口合并发布。

| 协议 | 节点监听 | 证书与凭据 |
|---|---|---|
| VLESS + Reality | TCP | 保留原有 X25519、UUID 与 short ID，不需要申请证书 |
| Hysteria2 | UDP | TLS 证书，每个授权独立密码 |
| Shadowsocks 2022 | TCP + UDP | AES-128-GCM 或 AES-256-GCM，节点密钥与授权密钥自动生成 |
| TUIC v5 | UDP | TLS 证书，每个授权独立 UUID 和密码，ALPN 为 h3 |
| AnyTLS | TCP | TLS 证书，每个授权独立密码 |
| Naive | TCP（HTTP/2） | TLS 证书，统计名称作为用户名，独立密码；客户端开启 UDP over TCP |
| Snell v6 | TCP | v6 多用户模式，节点 PSK 与每个授权的 userkey 自动生成 |

所有协议使用同一用户账本，统计名称仍为 `u{用户ID}_n{节点ID}`。撤销后重新授权会更换凭据；旧的客户端需更新订阅。协议与 Shadowsocks 加密方法创建后不可更改，需要变更时创建新节点。

## 节点连接与高级设置

参考 3X-UI 的入站设置分组和 S-UI 的监听/客户端地址分离，具体原生字段按固定 sing-box 1.14.2 实现，见 [ADR 0042](adr/0042-node-settings-and-panel-operations.md) 和 [ADR 0075](adr/0075-node-options-and-background-refresh.md)。

| 字段 | 行为 |
|---|---|
| 启用节点 | 默认开启。关闭后立即移出订阅资格，设备应用整包后停止监听；授权、凭据和历史保留。链路任一端停用会使入口失效，不会退化为直连 |
| 监听地址 / 监听端口 | 本机 IP，默认 `::`；IPv4 可填 `0.0.0.0`。端口仍在每台服务器内唯一，停用节点继续占用其分配记录 |
| 公开地址 / 公开端口 | 客户端与内部链路连接端点；公开端口留空跟随监听端口，NAT 场景可分别填写。修改不会自动设置端口映射或网络策略 |
| TCP Fast Open / 保活 | 仅 TCP 入站可选。Fast Open 默认关闭，需系统支持；保活空闲与间隔为 1–3600 秒，留空使用默认值，关闭保活时不发送时长 |
| TLS 版本 / 握手超时 | 证书协议可指定 TLS 1.2/1.3 的范围，QUIC 必须可使用 1.3；Naive 版本范围仅服务端生效。Reality、AnyTLS、Naive 服务端握手超时 1–3600 秒，留空默认 |
| VLESS 传输 / 流控 | 默认 TCP + Vision，也可使用无 flow 的 TCP、WebSocket、HTTPUpgrade、gRPC；非 TCP 传输自动关闭 Vision。WS 可设路径、Host、提前数据大小及请求头，HTTPUpgrade 可设路径/Host，gRPC 可设服务名；同步编译服务端、客户端、分享链接与受管链路 |
| TLS ALPN | 证书协议可设置最多 8 个不重复 ASCII 值；留空沿用默认值。Naive 仅允许 h2，客户端自行协商；不适用于 Reality |
| Reality | 握手目标默认跟随 SNI，握手端口默认 443；客户端指纹同时写入 JSON、分享链接与内部链路；时间容差 1–3600 秒仅服务端，留空不额外限制 |
| Hysteria2 | 上下行带宽同时留空或填写 1–1000000 Mbps，以服务器为视角，客户端自动交换方向。强制 BBR 与手填带宽互斥；Salamander 混淆密码可自动生成，编辑留空保留，关闭清除。BBR 档位为标准/保守/激进，仅协商 BBR 时生效；服务端可设置字符串伪装响应（状态码 200–599、内容类型、最多 16 KiB 内容；固定版本只在 200 响应时支持自定义内容类型，非 200 使用运行时决定的类型） |
| TUIC | CUBIC / BBR / New Reno，认证超时与心跳 1–3600 秒；留空用原生默认值。0-RTT 默认关闭，启用需接受重放风险。客户端 UDP 转发模式为原生 UDP、QUIC 流或 sing-box UDP over stream，互斥生成原生字段 |
| AnyTLS | 客户端闲置会话检查/超时 1–3600 秒，保留数量 0–128；留空使用原生默认值。服务端填充策略每行一项、最多 8 KiB，包含 `stop` 及合法编号/填充区间；空列表恢复默认 |
| Snell v6 | 默认、关闭整形或不安全原始传输模式；两端模式一致，连接复用仅客户端 |
| Shadowsocks 2022 | 客户端 UDP over TCP；两端启用多路复用与填充，客户端可选 h2mux/smux/yamux。连接数/最少流数与每连接最大流数互斥，数量 1–1024；关闭复用恢复默认参数 |

编辑时完整回显已保存的高级设置，清空可选值恢复默认；协议组一旦提交，未提供的组内字段恢复该组默认值。存活混合链路引用的节点提前显示关联链路，只允许修改名称和启用状态；更改连接参数应先替换链路中的节点。

管理接口不会回显混淆密码或服务端私钥。升级本次 Agent 后再使用 HY2 混淆和自定义 QUIC ALPN：旧 Agent 的普通 QUIC 健康探测无法正确验证这些监听器。新适配器执行实际混淆 QUIC/TLS 握手，错误密码或证书域名均不能通过。

节点行“部署”显示所属服务器的合并等待、目标/应用版本、失败回报和有效授权节点数量；“检查部署条件”检查 Agent 接入、在线、插件声明和签名运行时。缺少制品时由维护者按[部署文档](deploy.md#导入签名-release)准备。检查条件通过不代表已应用，最终以 Agent 健康回报为准。

## TLS 证书

Hysteria2、TUIC、AnyTLS、Naive 的“证书域名”必须是证书覆盖的 DNS 名。

- 自动：选择 Let's Encrypt，填写联系邮箱和验证方式。HTTP-01 要求域名解析到服务器、TCP 80 可从公网访问且未被占用；TLS-ALPN-01 对 TCP 443 有相同要求。两种方式均不支持通配符申请。同一服务器共用邮箱与验证方式；编辑现有自动证书时这两项原子同步到该服务器的其他自动证书节点，冲突时整个修改撤回。
- 手动：同时粘贴 PEM 证书链和私钥。面板检查格式和密钥匹配；部署时实际握手检查域名、信任及有效期。编辑时两项均省略会保留原证书。手动证书需自行续期并重新保存。

自动证书由运行时持续维护，保存在其 `data/certificates` 目录，配置版本切换和回滚都保留。该目录包含账户密钥和证书私钥，应按现有服务账户权限备份，不得公开。面板列表不回显任何私钥；客户端订阅只包含必要的公共证书和自身授权凭据。

初次签发最多等待 240 秒；健康检查要完成真实 TLS 或 QUIC 握手，同时检查服务状态和统计接口。证书未就绪、域名不匹配或过期都不会使新版本进入订阅，失败沿用既有回滚。TLS-ALPN-01 的 TCP 443 不能同时作为托管 TCP 节点端口；使用 TCP 443 节点时请选择 HTTP-01。UDP 443 不与 TCP 验证端口冲突。

管理员自行准备 DNS、网络放行和服务绑定低端口所需的系统权限；本功能不修改 DNS、防火墙或系统安全设置。已有 Linux systemd/OpenRC 服务带绑定低端口能力；其他系统仍需核对实际服务账户权限。本轮未接入 DNS-01 或 DNS 服务商令牌。

## 客户端订阅

新增协议统一使用 `?format=singbox`，界面默认复制此格式；需要支持对应协议的 sing-box 客户端，建议与固定运行时 1.14.2 对齐。Naive 客户端还需包含 `with_naive_outbound` 及对应平台 Cronet 支持，Snell 客户端必须支持 v6。服务端支持不代表所有客户端或操作系统发行包都支持全部出站。

原有链接订阅和订阅令牌保持不变；`?format=links` 仅支持 Reality。混合订阅含新增协议时返回 409，提示切换 JSON，不静默省略节点、不生成非标准 Snell 链接。手动证书的公共链随 JSON 下发，客户端不启用 `insecure`。订阅仍仅含已经成功应用且健康的版本及当前有效授权。

在“代理用户 → 订阅链接”可查看已授权、当前资格与可用节点、套餐额度和到期状态，并复制订阅地址、显式预览、复制完整配置或下载文件。未部署、节点停用、套餐未开始/到期/额度用尽时会说明原因。内容获取复用公开订阅的同一生成逻辑；读取失败不继续提供旧配置，复制/下载时重新检查。

支持 `?format=singbox&download=true` 下载 JSON。后台预览只向面板当前同源的管理员接口请求，不调用第三方转换服务；订阅地址不存入浏览器持久存储。重置后旧地址立即失效，但已经下载的节点凭据保持不变，需要撤销授权才能使旧凭据失效。

## 本地验证

Rust、Bun 常规检查外，可使用固定运行时执行实测：

```sh
APSP_TEST_SINGBOX=/tmp/sing-box APSP_GROUPS_RUNTIME=/tmp/sing-box APSP_TEST_UPSTREAM=/tmp/sing-box cargo test -p apsp-compiler -- --include-ignored
APSP_TEST_SINGBOX=/tmp/sing-box cargo test -p apsp-panel --test protocol_runtime -- --ignored --nocapture
```

新增参数还可用官方 1.14.2 归档执行配置解析和混淆健康探测（纯 Go 归档需同时解出 `libcronet.so`，完整计量验收仍需上面的定制标签构建）：

```sh
APSP_TEST_UPSTREAM=/tmp/upstream/sing-box cargo test -p apsp-compiler --test modern_protocols official_runtime_accepts_protocol_settings -- --ignored
APSP_TEST_UPSTREAM=/tmp/upstream/sing-box cargo test -p apsp-compiler --test node_transports official_runtime_accepts_all_transports_and_both_relay_formats -- --ignored
APSP_TEST_UPSTREAM=/tmp/upstream/sing-box cargo test -p apsp-adapter-singbox obfuscated_quic_health_checks -- --ignored
```

运行时需包含 `with_quic,with_acme,with_v2ray_api,with_utls,with_naive_outbound`；本地环境需 OpenSSL 生成一次性证书。第二项通过实际客户端验证新协议的 TCP/UDP 流量、用户计量、证书域名拒绝及重载撤销。所有测试只使用临时凭据和回环端口。

实际 ACME 测试使用 [Let's Encrypt Pebble](https://github.com/letsencrypt/pebble)：在独立目录编译其 `cmd/pebble` 与 `cmd/pebble-challtestsrv` 到 `bin/`，然后运行：

```sh
python3 tools/acme-smoke.py --pebble-source /tmp/pebble --runtime /tmp/sing-box
```

脚本隔离测试 CA/DNS、执行真实 HTTP-01 与 TLS-ALPN-01、检查证书持久复用，以及短期证书过期后重载自动续期；不请求公网证书、不安装系统信任根。测试证书只存临时目录。此测试不替代真实 DNS、公网 Let's Encrypt、长期驻留续期与多平台实机验收；运行结果及未验证范围见 PROGRESS。
