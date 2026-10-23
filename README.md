# apsp

**自托管的服务器与代理节点管理面板。**

apsp 把分散在不同云厂商、不同地区的服务器接入同一个面板：集中查看状态、流量、到期与费用，完成日常运维和网络诊断；需要时再通过 sing-box 插件在这些服务器上提供代理节点，管理代理用户、套餐和订阅。

[功能地图](docs/product-map.md) · [使用文档](docs/README.md) · [安装部署](docs/deploy.md) · [开发指南](docs/dev.md) · [代码目录](docs/repository.md) · [验证进度](PROGRESS.md)

## 适合谁

- 手上有几台到几十台 VPS 的个人或小团队，服务器分布在多家云厂商和多个地区。
- 想在一个地方看清每台服务器是否在线、资源够不够、流量用了多少、什么时候到期、每月花多少钱。
- 自己维护代理服务，给自己或少量熟人使用，需要节点、订阅、套餐和流量统计，但不想手工改配置文件。

apsp 不是云厂商控制台，也不是公开售卖的代理平台：没有公开注册和在线支付，通知只提醒、不自动处置。完整边界见[功能地图](docs/product-map.md#不做什么)。

## 工作方式

- **面板保存期望状态。** 服务器、节点、用户和授权都在面板编辑，面板把它们确定性地编译成每台服务器的配置包。
- **Agent 主动连接面板。** 每台服务器安装一个 Agent，由它上报指标、比对期望状态、应用配置并回报结果；服务器不需要开放入站管理端口。
- **运行时独立运行。** sing-box 作为独立系统服务运行；面板或 Agent 暂时离线时，已应用的节点继续提供服务。

## 功能

侧栏按任务分为七组，每组两到四个入口：

| 功能区 | 能做什么 | 文档 |
| --- | --- | --- |
| **总览** | 服务器看板实时显示状态、历史曲线、延迟与丢包，可选只读公开；统计报表回顾网卡与代理流量的趋势和排行 | [看板](docs/server-display.md)、[统计](docs/statistics.md) |
| **服务器** | 添加与接入服务器；详情按概况、资产与流量、Agent 与任务、插件、IP 信息、节点验机、TCP 诊断分页；运维工具提供终端、服务日志与文件、资产与生命周期；自动化与恢复提供批量任务、故障事件、备份恢复和云费用账本 | [部署接入](docs/deploy.md)、[资产](docs/server-assets.md)、[日常运维](docs/fleet-operations.md)、[自动化与恢复](docs/operations-recovery.md) |
| **监控告警** | 离线、资源、到期和流量告警；TCP / ICMP 周期延迟检测；Telegram 与 Webhook 分渠道投递和重试 | [延迟检测与通知](docs/monitoring.md) |
| **网络** | 网络检测测量连通性、路由、吞吐、硬件与 IP 质量；网络配置维护域名、证书、端点、转发、隧道、组网与防火墙；DDNS 同步 Cloudflare、腾讯云、阿里云和华为云 | [网络检测](docs/network-workbench.md)、[网络配置](docs/network-configuration.md)、[DDNS](docs/ddns.md) |
| **代理服务** | sing-box 插件：节点库、多段链路与订阅来源，代理用户、策略组与套餐，订阅预览、复制、下载与重置，代理流量统计 | [节点库](docs/node-catalog.md)、[协议与证书](docs/proxy-protocols.md)、[策略与套餐](docs/singbox-groups.md)、[链路](docs/agent-runtime-and-chains.md) |
| **扩展** | 插件中心介绍每个插件的用途和已验证版本，并按服务器汇总启用状态；阿里云 CDT 管理用量、带宽、ECS 启停与费用缓存 | [插件中心](docs/plugin-catalog.md)、[阿里云](docs/alicloud.md) |
| **系统** | 面板设置（公开看板、显示币种与汇率、历史保存）；管理员与权限、凭据、API 令牌、会话和审计；当前管理员的 Passkey 与二步验证 | [管理与安全](docs/control-center.md)、[Passkey](docs/passkeys.md) |

代理协议支持 VLESS + Reality、Hysteria2、Shadowsocks 2022、TUIC v5、AnyTLS、Naive 和 Snell v6，TLS 证书可手动上传或自动申请、续期。界面支持简体中文和 English。

## 快速开始

1. **安装面板。** 准备 Docker Engine、Compose 插件与 Python 3，按[部署文档](docs/deploy.md)核对仓库内的发布公钥，然后在仓库根目录执行：

   ```sh
   python3 scripts/panel.py install --public-url https://panel.example.com
   ```

   面板默认监听 `http://127.0.0.1:8080`，远端 Agent 接入需要可达的 HTTPS 地址。`scripts/panel.py` 还提供状态、日志、自检和私有备份；升级前会先备份，保留凭据与数据卷。
2. **接入服务器。** 登录后在「服务器列表」添加服务器，复制一次性安装命令到服务器上执行：Linux、macOS、FreeBSD 使用 Shell 入口，Windows 使用 PowerShell 入口。安装前，维护者需按[制品导入流程](docs/deploy.md#导入签名-release)准备对应架构的已签 Release；入口自动选择最新兼容的已签稳定版本，也可指定版本。
3. **按需启用插件。** 服务器上线后即可使用看板、告警和运维工具。需要提供代理时，在「代理服务 → 服务概览」按四个步骤启用 sing-box、创建节点和代理用户。

## 平台与安全边界

- Agent 支持 Linux systemd/OpenRC、macOS launchd、FreeBSD rc.d 和 Windows 计划任务，各平台制品与升级范围见[设备平台与能力](docs/platforms.md)。
- Agent 二进制只从 GitHub Release 或配置的独立 HTTPS 镜像下载，面板不提供 Agent 二进制。安装和自动更新独立验签，设备只应用内嵌公钥认可的制品；运行时和配置仍经面板分发。单行安装入口校验固定摘要并核验签名，无需预装 `apsp-bootstrap`。
- 运行时固定上游 sing-box 1.14.2，保留官方默认构建标签并启用统计 API，不修改上游源码。
- 管理员登录支持限速、TOTP 二步验证和 Passkey；Passkey 需要实际使用的 HTTPS 域名，本地开发可用 localhost。代理用户与管理员使用独立会话。
- 删除在线服务器时先停止服务、清除设备凭据；离线服务器只删除面板记录，本地服务需手动停用。
- NodeQuality 完整验机暂停新任务，限制见[安全门禁](docs/acceptance/nodequality-full-start-gate.md)。生产迁移、正式发布与实机验收按各功能文档的边界单独执行。

## 开发与维护

- [功能地图](docs/product-map.md)：定位、功能分区、归类规则和界面用语；新增功能先确定归属。
- [代码目录与维护约定](docs/repository.md)：面板、Agent、插件、前端、脚本和测试的位置。
- [开发指南](docs/dev.md)：本地构建与检查；[发布流程](docs/release.md)：签名和公钥轮换。
- [HTTP API](docs/api.md)、[设备协议](docs/protocol.md)、[架构决策索引](docs/adr/README.md)。
- [PROGRESS](PROGRESS.md) 记录历史进度，[验收索引](docs/acceptance/README.md)记录实机门禁与证据。仓库与部署命名统一为 `apsp`，既有安装的迁移见[改名说明](docs/apsp-renaming.md)。

GitHub Actions 当前按协作安排暂停，只做本地验证。未运行或取消的 CI 不代表通过，恢复条件见 [AGENTS.md](AGENTS.md#临时-ci-暂停2026-10-01-用户要求)。

许可证：[AGPL-3.0-only](LICENSE)。
