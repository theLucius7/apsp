# apsp

**未发布开发基线：** 不支持旧 APSP 开发数据库自动升级。空库初始化当前结构，旧库拒绝启动，请先备份再手动重建；见[数据库基线与重建说明](docs/database-baseline.md)。

**自托管的服务器管理与配置面板。**

apsp 用一个面板管理分散在各家的服务器：每台服务器安装一个 Agent，上报监控数据、接收面板下发的服务配置并回报结果；面板离线时，已应用的配置照常运行。面板同时记录服务器的资产、费用与续费。

[功能地图](docs/product-map.md) · [路线图](docs/roadmap.md) · [使用文档](docs/README.md) · [安装部署](docs/deploy.md) · [开发指南](docs/dev.md) · [验证进度](PROGRESS.md)

## 工作方式

- **面板保存期望状态。** 服务配置、用户和授权都在面板编辑，面板把它们确定性地编译成每台服务器的 sing-box 配置包。
- **Agent 主动连接面板。** 由 Agent 上报指标、比对期望状态、应用配置并回报结果；服务器不需要开放入站管理端口。
- **运行时独立、账本可靠。** sing-box 作为独立系统服务运行，面板或 Agent 暂时离线不影响已应用的配置；流量按用户只计一次，重启后不重复、不丢失。

## 功能

### 服务配置（核心）

| 能力 | 说明 | 文档 |
| --- | --- | --- |
| 节点与协议 | 每台服务器上的服务入口；TLS 证书手动上传或自动申请、续期 | [协议与证书](docs/proxy-protocols.md) |
| 链路 | 多台服务器组成的多段路径 | [sing-box 管理](docs/singbox-management.md)、[节点库](docs/node-catalog.md) |
| 用户、策略组与套餐 | 策略组决定能用哪些节点，套餐决定流量额度、有效期和重置周期 | [策略与套餐](docs/singbox-groups.md) |
| 订阅与流量账本 | 订阅预览、复制、下载与重置；逐用户、逐节点的精确流量 | [安装与控制流程](docs/singbox-installation.md) |
| 用户自助门户 | 用户用 Passkey 登录自己的页面，查看订阅与用量 | [Passkey](docs/passkeys.md) |
| 部署与运行时运维 | 配置预检、发布、运行时状态与日志、重启与部署重试 | [运维流程](docs/singbox-operations-workflows.md) |

### 服务器运营（支撑）

| 能力 | 说明 | 文档 |
| --- | --- | --- |
| 服务器看板与统计 | 实时状态、历史曲线、延迟与丢包，可选只读公开；网卡与服务流量趋势 | [看板](docs/server-display.md)、[统计](docs/statistics.md) |
| 资产与流量 | 地区、成本、到期、续费记录、网卡流量额度与矫正 | [服务器资产](docs/server-assets.md) |
| 告警与延迟检测 | 离线、资源、到期和流量告警；三网 TCP / ICMP 延迟检测；Telegram 与 Webhook | [延迟检测与通知](docs/monitoring.md) |
| 网络检测 | IP 信息、TCP 连接诊断、基础网络探测与路由追踪 | [网络检测](docs/network-workbench.md) |
| 管理与安全 | 管理员权限、API 令牌、审计、自检，面板备份与恢复 | [管理与安全](docs/control-center.md) |

可选功能：[DDNS](docs/ddns.md)（Cloudflare、腾讯云、阿里云、华为云）与[阿里云 CDT](docs/alicloud.md)。

**实验功能**：远程命令、交互式终端、日志与文件、批量任务与自动处置、故障事件，以及网络配置中的端口转发、隧道、组网与防火墙、云资源对照、吞吐与硬件测试等，保留可用但不再扩展，高风险操作将默认关闭。分级清单见[功能地图](docs/product-map.md#功能分级)。

后续计划见[路线图](docs/roadmap.md)。

## 快速开始

1. **安装面板。** 准备 Docker Engine、Compose 插件与 Python 3，按[部署文档](docs/deploy.md)核对仓库内的发布公钥，然后在仓库根目录执行：

   ```sh
   python3 scripts/panel.py install --public-url https://panel.example.com
   ```

   面板默认监听 `http://127.0.0.1:8080`，远端 Agent 接入和 Passkey 需要可达的 HTTPS 地址。`scripts/panel.py` 还提供状态、日志、自检和私有备份；升级前会先备份，保留凭据与数据卷。
2. **接入服务器。** 登录后在「服务器列表」添加服务器，复制一次性安装命令到服务器上执行：Linux、macOS、FreeBSD 使用 Shell 入口，Windows 使用 PowerShell 入口。安装前，维护者需按[制品导入流程](docs/deploy.md#导入签名-release)准备对应架构的已签 Release。
3. **配置服务。** 登录后的「概览」会列出需要处理的事项和开始使用步骤：按步骤为服务器安装 sing-box，创建节点和链路，再创建用户、分配策略组与套餐，把门户开通链接发给对方。

## 平台与安全边界

- Agent 支持 Linux systemd/OpenRC、macOS launchd、FreeBSD rc.d 和 Windows 计划任务，各平台制品与升级范围见[设备平台与能力](docs/platforms.md)。
- Agent 二进制只从 GitHub Release 或配置的独立 HTTPS 镜像下载，面板不提供 Agent 二进制。安装和自动更新独立验签，设备只应用内嵌公钥认可的制品；运行时和配置仍经面板分发。
- 运行时为上游 sing-box 1.14.2 加 `tools/singbox-runtime/` 的补丁（ADR 0104），保留官方默认构建标签并启用统计 API。
- 管理员登录支持限速、TOTP 二步验证和 Passkey；用户与管理员使用独立会话。
- 删除在线服务器时先停止服务、清除设备凭据；离线服务器只删除面板记录，本地服务需手动停用。
- NodeQuality 完整验机暂停新任务，限制见[安全门禁](docs/acceptance/nodequality-full-start-gate.md)。生产迁移、正式发布与实机验收按各功能文档的边界单独执行。

## 开发与维护

- [功能地图](docs/product-map.md)：定位、功能分级、归类规则和界面用语；新功能先确认所属功能区。
- [代码目录与维护约定](docs/repository.md)、[开发指南](docs/dev.md)、[发布流程](docs/release.md)。
- [HTTP API](docs/api.md)、[设备协议](docs/protocol.md)、[架构决策索引](docs/adr/README.md)。
- [PROGRESS](PROGRESS.md) 记录历史进度，[验收索引](docs/acceptance/README.md)记录实机门禁与证据。仓库与部署命名统一为 `apsp`，既有安装的迁移见[改名说明](docs/apsp-renaming.md)。

GitHub Actions 当前按协作安排暂停，只做本地验证。未运行或取消的 CI 不代表通过，恢复条件见 [AGENTS.md](AGENTS.md#临时-ci-暂停2026-10-01-用户要求)。

许可证：[AGPL-3.0-only](LICENSE)。

Nexus 迁移用户、Wallos 订阅费用及兼容通道见 [Nexus 与 Wallos 迁移](docs/nexus-migration.md)。

阿里云 CDT 配置见 [RAM 最小权限与实例 ARN](docs/alicloud-ram.md) 和 [CDT 脚本守护迁移](docs/cdt-script-migration.md)。
