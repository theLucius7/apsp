# apsp

**自托管的代理网络控制平面。**

apsp 把你分散在各家的 VPS 编成一张可靠的代理网络：入口、中转、落地一处配置，节点和订阅自动下发，流量按用户精确记账；面板离线时，已应用的节点照常工作。你分享给的每个人都可以自己登录，查看订阅、用量、套餐与到期。

[功能地图](docs/product-map.md) · [路线图](docs/roadmap.md) · [使用文档](docs/README.md) · [安装部署](docs/deploy.md) · [开发指南](docs/dev.md) · [验证进度](PROGRESS.md)

## 适合谁

- **线路玩家**：手上 5–50 台跨多家厂商的服务器，按入口机、中转机、落地机组织，关心回国线路、延迟丢包和 IP 能否解锁。
- **为自己和亲友自建节点的人**：需要节点、链路、订阅、套餐和准确的流量统计，让每个人能自己登录查看，但不想运营一个售卖平台。原来用 Xboard 一类面板管理熟人订阅的，可以迁移过来（规划中）。
- **不想再忘记续费的人**：服务器、域名、机场订阅等周期费用一处记录、按时提醒（规划中）。

apsp 不是通用探针、通用运维面板或机场售卖系统：不做公开注册、支付和工单，通知只提醒、不自动处置。完整边界见[功能地图](docs/product-map.md#不做什么)。

## 工作方式

- **面板保存期望状态。** 节点、链路、用户和授权都在面板编辑，面板把它们确定性地编译成每台服务器的 sing-box 配置包。
- **Agent 主动连接面板。** 每台服务器安装一个 Agent，由它上报指标、比对期望状态、应用配置并回报结果；服务器不需要开放入站管理端口。
- **运行时独立、账本可靠。** sing-box 作为独立系统服务运行，面板或 Agent 暂时离线不影响节点；流量只在用户接入的一端计一次，重启后不重复、不丢失。

## 功能

### 代理网络（核心）

| 能力 | 说明 | 文档 |
| --- | --- | --- |
| 节点与协议 | VLESS + Reality、Hysteria2、Shadowsocks 2022、TUIC v5、AnyTLS、Naive、Snell v6；TLS 证书手动上传或自动申请、续期 | [协议与证书](docs/proxy-protocols.md) |
| 链路 | 入口、中转、落地组成的多段线路；中间段或落地可以来自机场订阅中的节点 | [链路](docs/agent-runtime-and-chains.md)、[节点库](docs/node-catalog.md) |
| 用户、策略组与套餐 | 策略组决定能用哪些节点，套餐决定流量额度、有效期和重置周期 | [策略与套餐](docs/singbox-groups.md) |
| 订阅与流量账本 | 订阅预览、复制、下载与重置；逐用户、逐节点的精确流量 | [安装与控制流程](docs/singbox-installation.md) |
| 用户自助门户 | 代理用户用 Passkey 登录自己的页面，查看订阅与用量 | [Passkey](docs/passkeys.md) |
| 部署与运行时运维 | 配置预检、发布、运行时状态与日志、重启与部署重试 | [运维与代理用户流程](docs/singbox-operations-workflows.md) |

### 运营这张网络（支撑）

| 能力 | 说明 | 文档 |
| --- | --- | --- |
| 服务器看板与统计 | 实时状态、历史曲线、延迟与丢包，可选只读公开；网卡与代理流量趋势 | [看板](docs/server-display.md)、[统计](docs/statistics.md) |
| 资产与流量 | 地区、成本、到期、续费记录、网卡流量额度与矫正 | [服务器资产](docs/server-assets.md) |
| 告警与延迟检测 | 离线、资源、到期和流量告警；三网 TCP / ICMP 延迟检测；Telegram 与 Webhook | [延迟检测与通知](docs/monitoring.md) |
| 线路与 IP 质量 | IP 质量与解锁、TCP 连接诊断、基础网络探测与路由追踪 | [网络检测](docs/network-workbench.md) |
| 管理与安全 | 管理员权限、API 令牌、审计、自检，面板备份与恢复 | [管理与安全](docs/control-center.md) |

可选插件：[DDNS](docs/ddns.md)（Cloudflare、腾讯云、阿里云、华为云）与[阿里云 CDT](docs/alicloud.md)。

**实验功能**：远程命令、交互式终端、日志与文件、批量任务与自动处置、故障事件，以及网络配置中的端口转发、隧道、组网与防火墙、云资源对照、吞吐与硬件测试等，保留可用但不再扩展，高风险操作将默认关闭。分级清单见[功能地图](docs/product-map.md#功能分级)。

### 接下来

按[路线图](docs/roadmap.md)，下一步依次是：核心旅程实机签收与 v0.4 发布；多客户端订阅（mihomo / Clash、Shadowrocket 等）、完整的用户门户与 Xboard 类系统迁移、Wallos 式的费用与续费账本、中转规则和线路质量中心；最后开放供 AI Agent 调用的只读与受控接口。

## 快速开始

1. **安装面板。** 准备 Docker Engine、Compose 插件与 Python 3，按[部署文档](docs/deploy.md)核对仓库内的发布公钥，然后在仓库根目录执行：

   ```sh
   python3 scripts/panel.py install --public-url https://panel.example.com
   ```

   面板默认监听 `http://127.0.0.1:8080`，远端 Agent 接入和 Passkey 需要可达的 HTTPS 地址。`scripts/panel.py` 还提供状态、日志、自检和私有备份；升级前会先备份，保留凭据与数据卷。
2. **接入服务器。** 登录后在「服务器列表」添加服务器，复制一次性安装命令到服务器上执行：Linux、macOS、FreeBSD 使用 Shell 入口，Windows 使用 PowerShell 入口。安装前，维护者需按[制品导入流程](docs/deploy.md#导入签名-release)准备对应架构的已签 Release。
3. **搭建代理网络。** 在「代理服务 → 服务概览」按步骤为服务器启用 sing-box，创建节点和链路，再创建代理用户、分配策略组与套餐，把门户开通链接发给对方。

## 平台与安全边界

- Agent 支持 Linux systemd/OpenRC、macOS launchd、FreeBSD rc.d 和 Windows 计划任务，各平台制品与升级范围见[设备平台与能力](docs/platforms.md)。
- Agent 二进制只从 GitHub Release 或配置的独立 HTTPS 镜像下载，面板不提供 Agent 二进制。安装和自动更新独立验签，设备只应用内嵌公钥认可的制品；运行时和配置仍经面板分发。
- 运行时固定上游 sing-box 1.14.2，保留官方默认构建标签并启用统计 API，不修改上游源码。
- 管理员登录支持限速、TOTP 二步验证和 Passkey；代理用户与管理员使用独立会话。
- 删除在线服务器时先停止服务、清除设备凭据；离线服务器只删除面板记录，本地服务需手动停用。
- NodeQuality 完整验机暂停新任务，限制见[安全门禁](docs/acceptance/nodequality-full-start-gate.md)。生产迁移、正式发布与实机验收按各功能文档的边界单独执行。

## 开发与维护

- [功能地图](docs/product-map.md)：定位、功能分级、归类规则和界面用语；新功能先确认服务代理网络主线。
- [代码目录与维护约定](docs/repository.md)、[开发指南](docs/dev.md)、[发布流程](docs/release.md)。
- [HTTP API](docs/api.md)、[设备协议](docs/protocol.md)、[架构决策索引](docs/adr/README.md)。
- [PROGRESS](PROGRESS.md) 记录历史进度，[验收索引](docs/acceptance/README.md)记录实机门禁与证据。仓库与部署命名统一为 `apsp`，既有安装的迁移见[改名说明](docs/apsp-renaming.md)。

GitHub Actions 当前按协作安排暂停，只做本地验证。未运行或取消的 CI 不代表通过，恢复条件见 [AGENTS.md](AGENTS.md#临时-ci-暂停2026-10-01-用户要求)。

许可证：[AGPL-3.0-only](LICENSE)。

成员运营、Wallos 订阅费用、私密导出工具及兼容切换见 [Nexus 与 Wallos 迁移](docs/nexus-migration.md)。
