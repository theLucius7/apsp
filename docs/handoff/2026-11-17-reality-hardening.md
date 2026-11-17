# 交接：REALITY 封锁修复、阿里云 hk 换 IP 与全量上线（2026-11-17）

用户要求：“研究被墙的原因，提供修复然后 pr，修复到 apsp agent 里面；修复完了之后换 ip 更新”；之后“尽快保证能用，尽量热更新，不要中断”；“做规划，写进交付，写好 handoff.md 和 progress.md，写好需要做什么清单”。

决策与原因见 [ADR 0139](../adr/0139-reality-hardening-on-stable-runtime.md)，执行记录见 [PROGRESS](../../PROGRESS.md) 2026-11-17 条目。本文只写现状和接下来要做的事，不含任何密钥。

## 现状（2026-11-17 22:40 CST）

### 已完成

- **原因**：11-17 09:37–09:39 五个云的端口同一小时被集中封锁，11:38–11:40 四台整 IP 被封，14:40 aliyun-hk2 的 11456 被封。与流量无关。运行时 `1.14.4-apsp.1` 的 REALITY 回落与真实目标站在 6 项未认证探测上可区分，认证连接没有握手后记录；公开资料把 Apple 目标站、非 443 端口、云 IP 列为 2026 年封锁潮的画像。
- **修复**：PR #277（`84564240`…`49687316`）与 `bf71ea39` 已在 main，生产已部署。运行时 `1.14.4-apsp.2`（补丁提交 `b8c49498`）随 **agent-v0.3.9** 于 2026-11-17 11:58:15 UTC 签名发布（VPS 根 `4D645C2885298F58`）。`CI` 与 `Signed release draft` 已重新停用。
- **面板目标运行时**：`1.14.4-apsp.2`。导入：仅 `linux-musl-amd64`。
- **aliyun-hk1（server 9）/ aliyun-hk2（server 11）**：Agent 0.3.9、运行时 `1.14.4-apsp.2` musl（加固内核），健康。
  - 新 EIP：hk1 `47.76.79.98`（`eip-j6cf7534f086kknwhcctr`），hk2 `8.218.155.222`（`eip-j6chwz9ri2vnjvopkumer`），均已加入原共享带宽包。旧 EIP（hk1 `47.76.245.224`，hk2 `8.218.59.179`）**未绑定、未释放**，留作回退。
  - 公网只开 443 与 SSH：OpenRC 服务 `apsp-public-ingress`（`/etc/apsp-public-ingress.nft`）把 443 转到台湾线路（hk1 `11463`，hk2 `11456`），其他代理端口对外丢弃。hk2 原有的 `apsp-legacy-ingress`、`apsp-node19-port`、`apsp-node19-ipv6` 已停用并移出默认运行级（文件保留）。
  - 台湾线路（原节点 45、46，apsp 节点 27、19）目标站改为 `www.bandisoft.com`（`extend --revise`，hk1 修订 5、hk2 修订 13）；`nodes.sni`、旧面板 `v2_server`、apsp 公网端点均已同步。其余 6 条线路配置未动、对外关闭。
  - conntrack：443 转发启用连接跟踪，内存 435 MB 的默认上限 3584 在 22:22 写满导致 hk1 随机丢包；已热修改为 `nf_conntrack_max=65536`、`hashsize=16384`、established 超时 7200 秒，持久化在 `/etc/sysctl.d/90-apsp-conntrack.conf`、`/etc/modprobe.d/apsp-conntrack.conf`，`apsp-public-ingress` 启动时再应用一次。
  - 磁盘（hk2 根分区 427 MB）：旧运行时 `1.14.0-apsp.1` 与 Agent `0.3.5` 移到 `/boot/apsp-stash/`。
- **用户自己的订阅**（`~/published-configs`，Caddy 按 User-Agent 区分 Clash 与 Shadowrocket）已改为新地址与 bandisoft，mihomo 实测可用。

### 未完成（影响）

- **其余 15 台的 Agent 仍是 0.3.7**（servers 1、2、3、5、6、7、8、13、14、15、16、17、18、19、21）。面板目标已是 `1.14.4-apsp.2`，GNU 与 arm64 制品尚未导入，这些服务器的**清单下发一直失败：运行中的内核和配置不变、线路可用，但任何配置变更（新增用户、改线路）都不会下发**，直到第 1 项完成。
- aliyun-hk3（server 20）被 CDT 守护关机到 12-01，仍是 Agent 0.3.7、目标站 `updates.cdn-apple.com`、端口 2053。
- 今天被封的 oracle-sg1/2/3、azure-in1、azure-kr1、huawei-sg1 的 IP/端口仍不可达（国内）。

## 需要做什么（按顺序）

### 1. 其余服务器换新内核（建议凌晨 04:00–06:00，逐台）

每台换内核会重启代理进程，连接中断约 20–60 秒，无法做到零中断热更新，所以放在低峰期逐台做。

1. 记录基线：`~/apsp-migration-20261102/ops-20261106/fleetcheck.py`（已加入 apsp.2 的二进制摘要），真实客户端 `~/apsp-kr-direct-20261109/realcheck.py <原节点>`。
2. 导入 GNU 与 arm64：`import_release.py agent-v0.3.9 linux-gnu-amd64`、`… amd64`、`… linux-gnu-arm64`（与 0.3.8 相同的目标名，以面板实际选择为准）。导入后未升级的 Agent 仍会因不支持运行时附件被拒绝，不会自动换内核。
3. 逐台升级 Agent：`upgrade_agent_039.py go <id>`（**不要用旧的 `upgrade_agent.py`，它把版本写死成 0.3.7，会误报成功**）。升级后该台在同一修订上换用新内核。每台确认：`fleetcheck.py <id>` 显示 `1.14.4-apsp.2`、健康；该机线路真实客户端正常；再做下一台。
   - 建议顺序：先无国内用户或已被封的（16、17、18、3、6、7），再 1、2、5、13、14、15、19，最后 8（本机，容器内运行时）与 21（仅监控）。
   - 小盘主机先看空间：13（约 400 MB）、14（约 330 MB）；GNU 包约 29 MB、解压后加 Cronet 约 70 MB。
4. 全部完成后 `fleetcheck.py` 全量核对，确认清单下发不再报错。
5. 回退：把面板目标改回 `1.14.4-apsp.1` 并部署（数据库仍允许该版本，制品库保留），各服务器在同一修订换回旧内核。

### 2. aliyun-hk3（12-01 CDT 守护自动开机后）

1. 确认开机（12-01 10:30 已有提醒）。
2. `upgrade_agent_039.py go 20`，确认换用 `1.14.4-apsp.2`。
3. 按 hk1/hk2 的做法：安装 `apsp-public-ingress`（443 → `2053` 对应的监听端口）与 conntrack 设置；台湾线路（原节点 53，apsp 节点 29）`extend --revise` 改 `www.bandisoft.com`；更新旧面板 `v2_server`、apsp 公网端点；IP 未被封，可不换。
4. 更新用户订阅中的 `tw0 | Wap[hk[5]]`（端口 443、`www.bandisoft.com`）并重新生成 Shadowrocket 文件。

### 3. 释放旧 EIP（用户确认新地址稳定后）

`~/apsp-block-20261117/eip/rotate.py hk1 --release-old eip-j6csuehkl3rg65a8qe7fc`，`rotate.py hk2 --release-old eip-j6c7jrcrtvh00aqf9ir3p`。回退窗口内可 `--rollback`。

### 4. 今天被封的其他云（需要用户在各云控制台换 IP 或决定）

oracle-sg1/2/3、azure-in1、azure-kr1、huawei-sg1：
- 换内核（第 1 项）之后再换 IP，否则新 IP 仍是旧画像。
- 同时改画像：公网 443、目标站换成同地区、TLS 1.3 + H2、不跳转的国外站点（避开 Apple、Microsoft 与 www.jd.com 这类国内站点）；先用 `~/apsp-block-20261117/targets/scan.py` 扫同网段、`check.py` 筛选，再在实验室比对。
- 新 IP 由用户提供后，按 hk 的流程更新 apsp 公网端点、旧面板 `v2_server` 与订阅。

### 5. 收尾

- 监控：新 IP 的国内可达性（apsp 检测每小时一次）；hk1/hk2 已关闭的 6 条线路在检测里会显示不可达，属预期，可考虑在 apsp 里下线或隐藏。
- 客户端：用户订阅已用 `client-fingerprint: chrome` 与 `support-x25519mlkem768: true`；成员的旧面板订阅仍是 `utls random` 与 smux，可另行评估。
- 本地清理：`~/apsp-block-20261117/secret/`（解密的快照、修订输入，含私钥）用完删除；`target1`、`target2` 构建目录、测试 Postgres 容器、`repo2` 工作树与本地分支。
- 运维脚本：`~/apsp-migration-20261102/ops-20261106/README.md` 补记 `upgrade_agent_039.py`。

## 文件与工具位置

- 工作目录 `~/apsp-block-20261117/`：`HANDOFF.md`（执行日志）、`notes/`（关联分析、网上资料、代码地图）、`lab/`（`activeprobe.py` 8 项探测、`shdelay.py`、`ccsprobe.py`、`reclog.py`、`gp_ping.py`）、`cutover/`（nft 规则、OpenRC 服务、`legacy_update.py`、`apsp_endpoint.py`、旧面板备份）、`eip/`（`rotate.py`、`ali.py`、轮换回执）、`release/bundle`（已签发布）。
- 主机访问：`~/apsp-nexus-takeover-20261025/host.py <旧面板主机 id>`（hk1=9、hk2=11、hk3=19），地址已随换 IP 更新。
