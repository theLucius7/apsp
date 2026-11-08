# ADR 0119：运行时改用旧服务同一份内核

状态：被取代：内核选择由 [ADR 0124](0124-stable-runtime-and-user-limits.md) 取代。

日期：2026-11-06。用户要求：“检查两者配置有什么不同，然后更新同步上，变成没什么不一样的，就一次性全切过来，singbox 不再原生接入了，变成自己也维护一个”，随后选定“自建与旧服务同一份内核”。本决策替代 [ADR 0104](0104-self-maintained-singbox-runtime.md) 的补丁内容（转发关闭行为、模仿握手后记录），保留其迁移节奏、健康检查和目录接口部分。

## 背景

配置逐台对比后（`extend --revise` 补齐 Oracle 的 `reuse_addr`），旧服务和 apsp 的差别只剩内核：

| | 旧服务（xboard-node） | apsp `1.14.2-apsp.1` |
| --- | --- | --- |
| 源码 | `github.com/cedar2025/sing-box` `2e665cb7`：上游 `v1.14.0-alpha.2` 加两个提交（入站用户与路由规则的热更新接口、mieru 协议），REALITY 部分与上游相同 | 上游 1.14.2 加 REALITY 仿真补丁 |
| 依赖 | `metacubex/utls` 1.8.4、`sing` 0.8.2 等 | `metacubex/utls` 1.8.7 等 |
| 构建 | Go 1.26.5，`CGO_ENABLED=0`，标签 `with_quic,with_utls,with_wireguard,with_clash_api`，Go 默认的 MPTCP 监听 | Go 1.26.8，CGO，上游完整标签，`multipathtcp=0` |

## 决策

运行时版本改为 `1.14.0-apsp.1`，由 `tools/singbox-runtime/` 重建旧服务的内核：

- 基线是上游 tag `v1.14.0-alpha.2`（`7df171ff`），应用 `cedar2025-2e665cb7.patch` 后目录树必须等于 cedar2025 分支 `2e665cb7` 的树，再应用 `legacy-dependencies.patch` 固定依赖版本。补丁按固定身份提交，提交哈希 `7cf03705` 写入构建与发布校验。基线来自上游官方仓库，补丁在本仓库，不依赖 cedar2025 的仓库继续存在。
- 构建与旧服务相同：纯 Go、旧服务的四个标签，另加 Agent 读取流量计数必须的 `with_v2ray_api`；不带上游发布脚本的 `multipathtcp=0` 和 `-checklinkname=0`；`godebug default=go1.26` 保持旧服务（主模块 go 1.26）的运行时默认值，`go version -m` 不出现 `DefaultGODEBUG`。工具链仍用 Go 1.26.8（只差补丁版本，含安全修复）。
- 运行时链接的 60 个模块写在 `modules.txt`，构建和发布校验要求逐行一致。其中 56 个与旧服务二进制（amd64 `fbe08463`、arm64 `bca8f5e3`）的版本和校验和相同；另外 4 个（cobra、pflag、maxminddb、wgctrl）只被 sing-box 命令行使用，旧服务把 sing-box 当库嵌入，没有链接它们。
- glibc 与 musl 主机用同一个静态二进制，制品名照旧（`amd64`、`linux-musl-amd64` 等）。构建可复现：同一输入两次构建得到相同哈希（amd64 `20f90dce`、arm64 `c97c126a`）。
- 版本号沿用 `1.14.N-apsp.M` 格式，Agent 0.3.4 起都接受，不需要先升级 Agent。`1.14.0` 表示基线属于 1.14.0 的预发布，具体源码以本文和补丁为准。
- Windows 运行时不再附带 `libcronet.dll`（纯 Go，不含 Naive 出站），Agent 的描述同步去掉这个附属文件。

## 不选的方案

- **保留 `1.14.2-apsp.1`：** 已在 10 台主机上运行、未见封锁，但线上行为与旧服务不同；用户要求切换前后一致。
- **上游 1.14.2 去掉补丁：** 依赖和 535 个上游提交都与旧服务不同，不是同一份内核。
- **旧内核只给接管主机、原生节点仍用 1.14.2：** 面板要同时维护两个运行时版本、两套制品和按主机选择版本的逻辑；目前没有在用的原生节点，不值得。

## 影响

- apsp 原生节点的以下配置这份内核不认识，部署时运行时会拒绝：Snell v6、ACME 证书提供者、Hysteria2 `bbr_profile`、TLS `handshake_timeout`、Naive 出站跳。以 `www.microsoft.com` 为目标站的 REALITY 无法认证客户端（utls 1.8.4 的问题，在用的 `itunes.apple.com`、`updates.cdn-apple.com`、`www.jd.com` 正常）。需要时把功能移植进补丁队列，或在界面隐藏，见[待定事项](../open-questions.md)。
- REALITY 的未认证行为回到旧服务的样子（ADR 0104 记录的破绽重新存在）。这是用户的选择：行为与此前多年运行的旧服务一致。
- 监听重新接受 MPTCP（与旧服务相同）。CVE-2026-98432 的已知利用路径经过 MPTCP，所以靠主机层面防护：各主机已升级到修复的内核，或用 `/etc/sysctl.d/90-disable-mptcp.conf` 关闭 MPTCP（2026-11-01 用户同意的处理；aliyun-hk1 的 6.18.44 当时未处理，切换前补上）。内核关闭 MPTCP 时 Go 自动退回普通 TCP。
- 切换方式：面板部署后，Agent 下次取清单时拿到新版本；同一配置版本只是内核变化，Agent 安装新内核后重启运行时，健康检查失败自动回滚到旧内核。导入签名制品之前，取清单会因缺少制品失败，运行时继续用旧内核，不中断。
- OpenRC 主机（阿里云三台）由 Agent 重启运行时后，运行时与 Agent 同在一个进程组；升级到 0.3.6 之前要按 [平台文档](../platforms.md#openrc) 从 shell 重启一次运行时。

## 验证

- 依赖对比：`go version -m` 对比旧服务二进制，结果如上。
- 17 台主机（10 台已接管、7 台待切）的候选配置都通过新内核的 `sing-box check`。
- `apsp-adapter-singbox` 的 `runtime` 集成测试通过：累计计数、重载后计数归零、按会话日志识别用户，并经 `clash_api` 列出和断开会话。
- 本机 REALITY 真实握手：以在用的三个目标站，新内核服务端分别对新内核和 `1.14.2-apsp.1` 客户端都能连通。
- `tools/verify-release-runtime.py`、`scripts/verify-e2e-runtime.py` 对四个 Linux 制品通过；迁移 0090 保留历史版本记录，`runtime_migration` 测试覆盖。
