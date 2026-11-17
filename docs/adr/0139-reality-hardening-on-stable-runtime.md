# ADR 0139：在 1.14.4 运行时上恢复 REALITY 仿真补丁

状态：已接受（2026-11-17）。恢复 [ADR 0104](0104-self-maintained-singbox-runtime.md) 的补丁内容（[ADR 0119](0119-legacy-service-kernel.md) 撤下、[ADR 0124](0124-stable-runtime-and-user-limits.md) 未带回），并加以修正；运行时版本、发布方式与 ADR 0124、0129 相同。

日期：2026-11-17。用户要求：“研究被墙的原因，提供修复然后 pr，修复到 apsp agent 里面；修复完了之后换 ip 更新”。

## 2026-11-17 的封锁

国内可达性检测（ADR 0105，每小时一轮，连续两轮失败才记事件）记下的时间：

| 首次失败 | 端点 | 目标站 |
| --- | --- | --- |
| 09:37–09:39 | oracle-sg3、oracle-sg2、azure-in1、azure-kr1 的 11452；aliyun-hk1 的 11463 | itunes.apple.com、www.jd.com、updates.cdn-apple.com |
| 11:38–11:40 | oracle-sg3、oracle-sg2、oracle-sg1、huawei-sg1 整个 IP | itunes.apple.com |
| 14:40 | aliyun-hk2 的 11456 | updates.cdn-apple.com |

- 五个云、五台主机的端口在同一小时内被封，之后部分升级为整 IP 封锁，是集中封锁，不是逐台触发。时间早于当天 `569de5d7` 的提交与部署，与它无关。
- 与流量大小无关：被封的 Oracle 线路前一晚每小时只有 20–36 MiB；流量最大的 aws-sg1（443 端口）、aws-jp1 和 wap-tw1 没有被封。
- azure-in1、azure-kr1、huawei-sg1 此前被封过、后来自行解封，这次再次被封。
- aliyun-hk1 的 11463 与 aliyun-hk2 的 11456 只有一位用户使用；该用户从前者换到后者后约 4 小时，后者也被封。两台主机的其他端口、22 端口和 ICMP 从国内一直可达。

公开资料（2026 年的几次 REALITY 封锁潮、Xray-core v26.3.23 起的警告）把同一类配置列为高风险：目标站是 Apple 域名、监听非 443 端口、使用代理常用的云。我们大部分线路正是这样。www.jd.com 是国内站点，不满足 REALITY 对目标站“国外网站”的最低要求。

## 运行时可被区分的行为

用未认证探测对比真实目标站和生产中的 `1.14.4-apsp.1`（`tools/singbox-runtime/wire-probe.py` 同类方法，目标 itunes.apple.com）：

| 探测 | 真实目标站 | `1.14.4-apsp.1` |
| --- | --- | --- |
| 发 ClientHello 后等待 | 20 秒 FIN | 不关闭 |
| 发 ClientHello 后半关闭 | 立即收到告警并关闭 | 20 秒后关闭，收不到告警 |
| 明文 HTTP、随机字节 | RST | FIN |
| ClientHello 后发垃圾数据 | 0.3 秒 RST | 不关闭 |
| 错误 SNI 的 ClientHello | 20 秒 FIN | 不关闭 |
| 目标站 FIN 之后再发数据 | RST | 一直接收 |
| 认证连接握手完成后的记录 | 两条 303 字节（会话票据） | 没有 |

前六项一次连接就能主动探测出来，最后一项在每条真实连接上都可旁路观察到（公开的 Aparecium 方法）。上游 sing-box 1.14.4 与它使用的 `metacubex/utls` v1.8.7 都没有这些修复（sing-box #4610 仍未关闭）；XTLS/REALITY 已陆续修复。

ADR 0104 的补丁修过前四项与最后一项，曾在约 10 台主机上运行（2026-11-02 至 11-06），期间没有新增封锁；ADR 0119 为了与旧服务行为一致撤下它，破绽回到线上。这个时间关系不能证明因果，但这些破绽是确定存在、可以修复的。

## 决策

### 补丁

`tools/singbox-runtime/apply.py` 从已校验的 Go 模块缓存复制 `metacubex/utls` v1.8.7 到 `third_party/utls`，应用 `utls-reality-hardening.patch`，再用 `go.mod` 的 `replace` 接入；`sing-box-reality-prepare.patch` 让 REALITY 入站启动时开始测量目标站。补丁按固定身份和日期提交，提交 `b8c49498e4ec16c883bc5bc5002c5221a08c3dc5` 写入构建与发布校验，包括复制进来的 utls 源码。

补丁内容：

1. **回落时转发关闭**：目标站的 FIN、RST 与客户端的半关闭、RST 都转发到另一侧，关闭时机和类型取自目标站。一侧的 RST 常被另一方向的写入先取走、读取只见 EOF，补丁按套接字已关闭的状态仍按 RST 处理。目标站关闭后客户端再发数据，目标站回 RST，客户端同样收到 RST：目标站结束后由一个观察协程轮询目标套接字（Linux 读 `tcp_info` 状态，关闭后才读取错误码区分 RST；其他 Unix 读错误码；Windows 识别 WSAECONNRESET 但不镜像），上传方向不等待。目标站关闭后最多再等 30 秒；回落时写给客户端的每次写入须在 2 分钟内有进展，不读数据的客户端不能一直占用连接。
2. **穿透连接包装**：sing-box 交给 REALITY 的连接（连接跟踪、计数、TCP Fast Open）不直接提供半关闭和 linger，补丁沿 `Upstream()`/`NetConn()` 找到底层套接字。ADR 0104 的补丁没有这一步，在 1.14.4 上半关闭会退化成立即断开、收不到目标站回复。
3. **模仿握手后记录**：后台按目标、SNI、ALPN 测量目标站在客户端 Finished 之后发送的记录长度，认证连接握手完成后发送等长的空记录，TLS 1.3 客户端忽略它们。测量只取长度、不校验证书（与 XTLS 相同，不依赖本机证书库），每次测量最多 15 秒；每 6 小时刷新，失败 5 分钟后重试，尚无结果时不发送。

### 不做的

- **按目标站探测可容忍的无效记录数**（XTLS/REALITY #29，针对重放 ClientHello 后发送多条 ChangeCipherSpec 的探测）：在用目标站（itunes.apple.com、updates.cdn-apple.com、www.jd.com）实测都容忍 32 条、第 33 条告警，与 utls v1.8.7 的固定值 32 相同。
- **拒绝不带 X25519MLKEM768 的 ClientHello**：现有 sing-box 与 Mihomo 客户端默认不发 MLKEM，会全部断线。
- **改客户端、目标站和端口**：属于线路配置，见下文“配置层面”。

### 版本与发布

版本号仍是 `1.14.4-apsp.2`：它尚未签名发布，没有任何制品，数据库的运行时版本检查已允许它，不需要改数据库基线。新补丁与 #255 的会话补丁、Cronet 一起随 Agent 0.3.9 发布。

发布顺序（取代 docs/release.md 中“在 main 之外打标签”的写法：`tools/publish.py` 要求标签提交有 main 上的 push CI 记录）：

1. 本变更合并到 main，面板目标仍是 `1.14.4-apsp.1`，部署不换内核。
2. 经用户临时批准开启 `CI` 与 `Signed release draft`，main 的 CI 通过后在该提交打 `agent-v0.3.9`，构建草稿、离线签名、发布，再停用两个工作流。
3. 全部 Agent 升级到 0.3.9（GNU 主机必须先升级，才能接收 Cronet 附件），导入发布中的运行时制品；目标版本未变，服务器不换内核。
4. 撤销 `7345021a`（面板目标改为 `1.14.4-apsp.2`），先用运行时滚动升级在一台主机上切换并观察，再推广；回退把目标版本改回 `1.14.4-apsp.1`。

## 配置层面（不由本补丁解决）

- 目标站：避开 Apple、iCloud、Microsoft 与国内站点，选同地区、支持 TLS 1.3 与 H2、不跳转的国外站点。
- 端口：公网用 443。
- 客户端：Mihomo 可用 `reality-opts.support-x25519mlkem768: true` 发出与真实 Chrome 一致的 MLKEM 密钥交换；本补丁的服务端已验证兼容。
- 换 IP 时同时调整上述配置，否则新 IP 仍是同一种画像。

## 验证

- `go test -race -count=10`（utls 包内 `Reality` 测试，含包装连接、目标站 RST、客户端 RST 被另一方向取走、客户端半关闭、FIN 后再发数据、目标站半关闭后上传不停顿）；撤掉客户端 RST 修复后对应测试失败。`go vet`（Linux、Windows、macOS），FreeBSD 与 arm64 编译通过。独立审阅（并发、泄漏、记录封装、`apply.py` 与替换检查）的意见已处理。
- 同一 `apply.py` 在两个全新 checkout 上得到相同提交；`build-singbox.sh` 构建 `linux-gnu-amd64`（制品 SHA-256 `1637c1a3…`），`verify-release-runtime.py` 通过。
- 本机对比真实 itunes.apple.com：上表 8 项探测的关闭类型、时机和收到的字节全部一致；认证连接握手后发送两条 303 字节记录，与目标站相同。
- sing-box 客户端（Vision）、Mihomo v1.19.33（Vision，带与不带 MLKEM）均能正常使用，20 MB 下载正常。

这些只能证明行为与目标站一致，不能证明不再被封。真正的验证是生产主机切换后持续观察国内可达性。
