# 自维护 sing-box 运行时

运行时 = 固定上游版本 + 本目录的补丁。决策与证据见 [ADR 0104](../../docs/adr/0104-self-maintained-singbox-runtime.md)。

| 文件 | 作用 |
| --- | --- |
| `utls-reality-hardening.patch` | 修改 `metacubex/utls`：转发关闭行为、模仿握手后记录，含 Go 测试 |
| `sing-box-reality-prepare.patch` | 入站启动时开始测量目标站 |
| `apply.py` | 把上述补丁应用到上游 checkout，并按固定身份和日期提交 |
| `wire-probe.py` | 对比 REALITY 入站和真实目标站的未认证行为 |

`tools/build-singbox.sh` 和 `tools/build-runtime-native.py` 都通过 `apply.py` 打补丁，并校验提交哈希。

## 修改补丁或升级上游

1. 更新补丁，或修改 `build-singbox.sh`、`build-runtime-native.py` 里的上游版本与提交。补丁基线变化时同步 `apply.py` 的 `UTLS_VERSION`。
2. 运行补丁测试：在应用了 utls 补丁的 checkout 中执行 `go test -race -run Reality .`。
3. 运行 `apply.py <上游 checkout>`，把输出的提交哈希写入 `build-singbox.sh`、`build-runtime-native.py`、`verify-release-runtime.py`、`scripts/verify-e2e-runtime.py`。
4. 递增版本号 `1.14.N-apsp.M`，同时修改上述脚本、`RUNTIME_VERSION`（`plugins/singbox/panel/mod.rs`）、面板 SQL 中的同名字面量，并新增迁移放宽版本约束。用 `rg` 搜索旧版本号，确认没有遗漏。
5. 用 `wire-probe.py` 对比新运行时和真实目标站，再用真实客户端验证普通与 Vision 模式。
