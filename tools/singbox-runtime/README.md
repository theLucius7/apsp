# 自维护 sing-box 运行时

运行时就是旧服务（xboard-node）一直在用的内核，由 apsp 自己重建、签名和发布。决策与对比见 [ADR 0119](../../docs/adr/0119-legacy-service-kernel.md)，此前的补丁方案见 [ADR 0104](../../docs/adr/0104-self-maintained-singbox-runtime.md)。

版本 `1.14.0-apsp.1` = 上游 `v1.14.0-alpha.2`（`7df171ff`）+ 本目录的补丁队列：

| 文件 | 作用 |
| --- | --- |
| `cedar2025-2e665cb7.patch` | `github.com/cedar2025/sing-box` 在 alpha.2 之上的两个提交：入站用户与路由规则的热更新接口、mieru 协议。应用后的目录树必须等于该分支 `2e665cb7` 的树 `ce5b9a70` |
| `legacy-dependencies.patch` | 把 `go.mod`/`go.sum` 固定到旧服务二进制链接的依赖版本，并用 `godebug default=go1.26` 保持旧服务的 Go 运行时默认值 |
| `modules.txt` | 运行时链接的全部模块（版本与校验和），构建和发布校验都要求逐行一致 |
| `apply.py` | 应用补丁，并按固定身份和日期提交，得到固定的提交哈希 |
| `wire-probe.py` | 对比 REALITY 入站和真实目标站的未认证行为 |

构建与旧服务相同：纯 Go（`CGO_ENABLED=0`），标签 `with_quic,with_utls,with_wireguard,with_clash_api`，另加 Agent 读取流量计数需要的 `with_v2ray_api`；不带上游发布脚本的 `multipathtcp=0` 和 `-checklinkname=0`。所以 glibc 与 musl 主机用的是同一个静态二进制，`tools/build-singbox.sh` 的 `--libc` 只决定制品名称。`modules.txt` 里除标 `cli` 的四个命令行模块外，每个模块的版本和校验和都与旧服务二进制（amd64 `fbe08463`、arm64 `bca8f5e3`）相同：旧服务把 sing-box 当库嵌入，没有链接命令行代码。

## 与 apsp 原生节点的兼容

内核早于上游 1.14.2，apsp 原生节点的以下配置它不认识，部署时会被运行时拒绝：Snell v6、ACME 证书提供者（`certificate_providers`）、Hysteria2 的 `bbr_profile`、TLS `handshake_timeout`，以及需要 `with_naive_outbound` 的 Naive 出站跳。以 `www.microsoft.com` 为目标站的 REALITY 入站也无法认证客户端（`itunes.apple.com`、`updates.cdn-apple.com`、`www.jd.com` 正常）。接管的旧线路都在旧服务上跑过这份内核，不受影响。

## 修改补丁或升级基线

1. 新的修改作为补丁加入队列，并写进 `apply.py`。改依赖时同时更新 `legacy-dependencies.patch` 和 `modules.txt`。
2. 运行 `apply.py <上游 checkout>`，把输出的提交哈希写入 `build-singbox.sh`、`build-runtime-native.py`、`verify-release-runtime.py`、`scripts/verify-e2e-runtime.py`。
3. 递增版本号 `1.14.N-apsp.M`，同时修改上述脚本、`RUNTIME_VERSION`（`plugins/singbox/panel/mod.rs`）、面板 SQL 中的同名字面量，并新增迁移放宽版本约束。用 `rg` 搜索旧版本号，确认没有遗漏。
4. 用 `sing-box check` 检查所有在用主机的候选配置，再跑 `apsp-adapter-singbox` 的 `runtime` 集成测试（`APSP_TEST_SINGBOX`），最后用真实客户端验证。
