# 构建与验证工具导航

本目录主要服务于维护者，命令在仓库根目录运行。面板日常安装和升级入口在 [scripts/](../scripts/README.md)，完整步骤见[开发指南](../docs/dev.md)和[签名发布](../docs/release.md)。按 [ADR 0141](../docs/adr/0141-self-contained-agent-and-desired-service-config.md)，正式发布仅分发各平台单文件自包含 `raw` `apsp-agent` 与配对对应源码包，不再单独发布或选装运行时与诊断制品。

## 一体化 Agent 构建与签名发布

| 工具 | 用途 |
| --- | --- |
| `build-agent.sh`、`build-agent.py` | 一体化自包含 `raw` Agent 平台构建入口（`APSP_RELEASE_EMBED_REQUIRED=1` 的 Linux 正式构建强制内嵌同 ABI `sing-box`、`-r22` `nodequality`、`ipquality`、`network-workbench` 四组件闭环并编译内置 `apsp-agent tcp-probe` 子命令，缺任一必需输入即失败，`<= 128 MiB`） |
| `prepare-agent-linux-inputs.py` | 准备 Linux 正式 Agent 编译期内嵌输入及配对对应源码包（`package-singbox-sources`、`prepare-bundled-tools` 打包 `-r22` `nodequality` 与 `network-workbench` `1.0.0`、`prepare-ipquality` 构建离线 rootfs 包） |
| `build-singbox.sh`、`build-runtime-native.py`、`singbox-builder.Dockerfile`、`verify-release-runtime.py` | 构建并静态核验供 `apsp-agent` 编译期内嵌的目标 ABI `sing-box`（`1.14.4-apsp.2`，GNU 含 `libcronet.so`、musl 纯静态、Windows 含 `libcronet.dll`），不作为独立安装制品分发 |
| `singbox-runtime/` | 自维护 `1.14.4-apsp.2` 运行时的补丁队列、依赖清单、应用脚本和线上行为对比工具 |
| `network_workbench_artifact.py`、`ipquality_artifact.py`、`build-nodequality.sh`、`nodequality*` | 受控诊断组件的离线归档构建、许可/来源固化与内嵌载荷校验（原生 TCP 探测由 `crates/tcp-probe` 直接编入 `apsp-agent`；目录内 `tcp_probe_*` 仅保留供历史归档回归兼容） |
| `release.py`、`publish.py`、`artifact_manifest.py` | 渲染安装器（`render-installer`）、组装自包含 Release 清单与配对源码包（`assemble`，省略旧历史兼容参数，仅收录 `raw` `apsp-agent`、`install.sh`、`release.json`、`SHA256SUMS` 与 `*-sources.tar.gz`）及离线签名后的发布核验（`verify` / `verify-publication`） |
| `render-bootstrap.py`、`render-bootstrap-powershell.py`、`bootstrap.py`、`legacy_agent_checkpoint.py` | 从模板和受控源码生成 `deploy/bootstrap.sh`、`deploy/bootstrap.ps1` 单行接入入口及历史版本持久化检查点预检 |

`tools/licenses/` 存放构建中使用的许可材料。内嵌载荷清单、配对源码摘要与固定来源须随对应构建更新，不手动改摘要绕过验证。包含 Linux 四组件内嵌闭环的新版一体化 Agent 正式签名 Release 尚未构建发布；NodeQuality 离线材料或网络检测工具归档准备不等于完整执行能力已经通过实机签收。

## 本地规则与回归

- `check-core-boundary.py`：检查 `agent-core` 与平台 crate（`panel-host`、`cloud-api`）的禁用词，以及 Agent 核心、适配器、平台与各领域 crate 的 `Cargo.toml` 依赖方向。
- `check-feature-boundaries.py`：按 `feature-boundaries.toml` 的表与路径归属，检查跨功能的 SQL、跨 crate 的 `#[path]` 和前端跨功能引用；已知的记在 `feature-boundaries.baseline`，只许减少（`--update` 删去已修复的，`--summary` 按所有者统计）。见 ADR 0128 与[功能划分与逻辑结构 §5.3](../docs/architecture.md#53-自动守护)。
- `route-policy.py`：核对路由权限表 `crates/panel/tests/fixtures/route-policy.tsv` 与源码登记的路由逐条一致（`--write` 补上新增路由、去掉已删除的；权限结论由 Rust 测试 `route_policy` 写入并比对）。见 ADR 0132。
- `test-*.py`：同目录工具、策略、构建脚本和夹具的回归；根 `tests/test_*.py` 也覆盖部分工具。
- `test-*.cjs`：既有前端浏览器夹具；较新的前端回归位于 `web/tests/`。
- `ci-run.py`、`ci-release-fixture.py`：已有 CI 调用及公开 TEST_ONLY 夹具支持，不能用于生产签名。

## 隔离环境与实机驱动

`agent-smoke.py`、`native-service-smoke.py`、`openrc-smoke.py`、`openrc-job-smoke.py`、`acme-smoke.py` 涉及实际进程、服务或容器，按[设备平台说明](../docs/platforms.md)和对应 CI/验收文档准备环境。

`p0-joint-load.py`、`probe-lease-acceptance.py`、`carrier-monitoring-acceptance.py`、`prepare-native-tcp-acceptance.py`、`prepare-plugin-install-acceptance.py` 属于指定场景的验收与准备驱动；不要把它们当成无需条件的通用测试命令。执行范围、资源预算和证据见[验收索引](../docs/acceptance/README.md)。

## 文件放置原则

新工具按用途放在现有目录内，与对应回归及说明一并维护。诊断工具的打包脚本与策略位于 `crates/observability/upstream/`，运行时服务定义位于 `deploy/`，面板/Agent 业务逻辑位于对应 Rust 模块，不因工具语言相同就移入本目录。已有脚本路径、生成入口和发布元数据保留兼容；当前整理不重写语言或改变安装依赖。

成员运营、Wallos 订阅费用、私密导出工具及兼容切换见 [Nexus 与 Wallos 迁移](../docs/nexus-migration.md)。

## 按固定清单逐台升级一体化 Agent

`fleet-upgrade.py` 经现有面板 API 按显式顺序逐台推进整包签名 Agent 升级（不读取本机部署目录、不直连服务器或数据库）。运行时随一体化 Agent 整包升级并由现有有效服务配置（`GET /api/traffic/servers/{id}` 的 `configured`）驱动，不存在独立运行时发布或单独组件选版：

- 必填参数：`--servers <ID ...>`（不重复的正整数服务器 ID 顺序清单）、`--agent-version <X.Y.Z>`（目标稳定版号）以及 `--api-url`（或环境变量 `APSP_API_URL`）。公开 Origin 用 `--public-origin`（或 `APSP_PUBLIC_ORIGIN`，省略时同 `--api-url`）；凭据用 `--credentials /private/path/panel.json`（当前用户独有、`0600` 普通文件，含 `password` 及可选 `login_name`、`totp_code`）或 `APSP_PASSWORD`。支持 HTTPS 与明确的回环 IP HTTP，禁止重定向，不输出 Cookie、凭据和面板错误正文。
- 默认只执行只读计划核验（输出 `fleet_plan` JSON，会话退出时自动注销）；传入 `--apply` 才实际修改设置，且必须同时提供 `--receipt /private/path/fleet-upgrade.json`（私有锁与原开关恢复回执）。可选 `--timeout 900`（单台超时秒数）、`--interval 10`（1–60 秒轮询间隔）与 `--import-targets <目标ABI ...>`（升级前调用 `POST /api/system/agent-releases/import` 收录对应签名目标；也可先由所有者收录后省略）。
- 已配置有效代理服务（`configured=true`）的目标机不接收单独的运行时版本参数，期望内置 `sing-box` 版本与摘要由已验签的一体化 Agent 发布清单及升级后设备上报的 `self_contained_agent`、`static_info.components["sing-box"]`、`static_info.component_digests` 自动派生并交叉核验；执行 `--apply` 时必须传入 `--client-check /private/path/check-server-client`（不经 shell 直接以 `<服务器ID> <派生的内置运行时版本>` 调用的真实客户端连通检查程序，返回 `0` 才放行下一台）。纯监控机（`configured=false`）不启动 `sing-box`，只核验 Agent 精确版本与在线状态，无需 `--client-check`。

```sh
# 1. 只读计划核验（清单含已配置服务的服务器时，一并校验升级前置条件并派生目标内置运行时身份）
APSP_API_URL=https://panel.example.com \
python3 tools/fleet-upgrade.py --servers 1 2 3 \
  --agent-version <目标稳定版本> \
  --credentials /private/path/panel.json

# 2. 按清单顺序逐台整包升级并核验配置回执与真实客户端（先准备私有回执目录与检查程序）
APSP_API_URL=https://panel.example.com \
python3 tools/fleet-upgrade.py --servers 1 2 3 \
  --agent-version <目标稳定版本> \
  --credentials /private/path/panel.json --apply \
  --receipt /private/path/fleet-upgrade.json \
  --client-check /private/path/check-server-client
```

每台先核对面板 `/api/system/agent-releases` 中匹配本机 ABI 的最高已签名稳定候选必须等于 `--agent-version`，持久化原 `auto_update` 后开启自动升级；观测到精确新 Agent 版本上线后立即恢复原 `auto_update` 开关。对 `configured=true` 的服务器继续等待内置运行时版本匹配、`status.state="ready"`、部署 `target_rev == applied_rev` 且健康无错，再主动发起 `POST /api/servers/{id}/runtime-control/checkpoint`（`{"module":"singbox"}`）等待受管进程与文件核对 `verified`，最后执行 `--client-check`。超时、异常、信号中断或任一台失败均非零退出并停止后续机器；恢复失败保留 `restore_required`，使用同一命令与回执重跑会先恢复原开关再重算计划。

自动升级接口按最高稳定版选择候选，执行期间不得并发导入其他 Agent 发布（候选漂移即中止并恢复设置）。工具不执行版本降级、服务配置修改或云实例/EIP 操作；工具与离线故障测试已实现，新版一体化 Agent 的正式签名发布、生产升级与真实客户端验收尚未执行。
