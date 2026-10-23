# 后续路线图

- 状态：**提议**，2026-10-24 起草。方向依据市场和社区，而不是现有代码；优先级、范围变更与授权由用户确认后，转为 ADR 或 AGENTS.md 规则。
- 研究范围：NodeSeek、LINUX DO、V2EX 的近期讨论，以及同类开源项目在 GitHub 上的功能、活跃度和安全公告（数据截至 2026-10-24），出处见文末。

## 一、市场在发生什么

### 1. 谁在长期玩服务器

论坛里的典型用户不是运维团队，而是“收集癖”玩家：

- **机器多、厂商杂。** 有人十几台服务器分布在腾讯云、阿里云、netcup、甲骨文等多家厂商和零售商家，“不可能每次都去商家后台一个一个看”，还会忘记续费。
- **用途集中。** 小机器主要用来做代理、中转、探针，或者干脆吃灰；大一点的跑博客、密码管理器、订阅管理和各种 Docker 小项目。很多是几十到几百 MB 内存的 NAT / LXC 小鸡。
- **分线路机和性能机。** 线路机看回国延迟、丢包和用量，业务机看网站或端口是否正常。
- **AI 成了新负载。** 2026 年下半年的帖子里，VPS 上大量出现 Hermes 等 AI Agent、AI 接口中转与聚合（CLIProxyAPI、sub2api），以及“让 AI 通过 SSH 管服务器、按情况发飞书或微信提醒”。
- **爱展示。** 公开探针页、“点亮各个国家的地图”本身就是乐趣；面板好不好看直接影响选择。

### 2. 他们现在用什么

| 需求 | 主流选择 | 趋势 |
| --- | --- | --- |
| 探针与监控 | Komari、哪吒、Beszel；新出现 CF-Server-Monitor（跑在 Cloudflare 免费额度上）、NodeGet（NodeSeek 官方） | 论坛推荐从哪吒转向 Komari，理由是轻量、好上手；哪吒的安全事件让一部分人离开。无服务器、零成本部署开始流行。 |
| 续费与到期 | 探针自带到期字段、Wallos、DueGuard 等 Telegram 机器人 | 到期、价格、流量重置日被当作独立需求，单独装工具来管。 |
| 节点面板 | 3x-ui（单机 Xray）、s-ui（单机 sing-box）、Marzban / Remnawave（多节点） | 自用选单机面板，运营选多节点面板；面板之间没有打通监控与续费。 |
| 订阅管理 | Sub-Store、妙妙屋、sublink 系列 | 自建节点与机场订阅混合使用；输出给 mihomo / Clash、Shadowrocket、Surge 等多种客户端；妙妙屋直接读取哪吒、Komari 探针的流量数据。 |
| 中转转发 | flvx、哆啦A梦（flux-panel）、nyanpass、直接写 realm 配置 | 线路机的端口转发和按规则计流量是常见需求，现有面板各自为政。 |
| 测试与质量 | NodeQuality、融合怪、IPQuality、回程路由脚本 | 关注 IP 是否“原生”、流媒体与 AI 服务能否解锁、三网回程线路。 |
| 运维操作 | SSH 客户端 + AI：Netcatty（自带 AI 运维 Agent）、Hermes、Muse | 面板不再是唯一的操作入口；用户希望 AI 能读面板数据、在受控范围内执行。 |

### 3. 竞品的走向

| 项目 | GitHub 星标 / 最近更新 | 值得注意的方向 |
| --- | --- | --- |
| Beszel | 约 2.6 万 / 2026-10 | 轻量、历史数据、Docker 统计和告警 |
| 哪吒 | 约 1.0 万 / 2026-10 | v2 持续发版；已加入 **MCP 接口及审计**，让 AI Agent 通过面板操作；2026-09 至 10 月连续公布多项安全公告 |
| Komari | 约 6500 / 2026-10 | 1 秒刷新、主题市场、插件、网页终端；2025-08 公布过 2FA 绕过和 WebSocket 劫持，2026-06 公布 CSRF |
| CF-Server-Monitor | 约 2700 / 2026-10 | Cloudflare Workers + D1 免费托管；Agent 只单向上报，不提供 WebSSH 和远程命令，可非 root 运行；三网延迟丢包、到期提醒、地图、主题商店、iOS 小组件；支持 OpenWrt、群晖、飞牛 |
| NodeGet | 约 300 / 2026-10 | Rust + PostgreSQL / SQLite，细粒度 Token 权限，JS Worker 扩展，一个 Agent 可同时连多个面板 |
| 3x-ui / s-ui | 约 4.8 万 / 约 1 万 | 单机节点面板的事实标准 |
| Sub-Store / 妙妙屋 | 约 1.1 万 / 约 1100 | 多客户端订阅、机场与自建节点聚合、读取探针流量 |
| 1Panel | 约 3.7 万 | 自我定位已改为“Linux 服务器运维管理面板和轻量 AI 管理平台” |
| CLIProxyAPI / sub2api | 约 5.6 万 / 约 4.5 万 | AI 接口中转成为 VPS 上的热门负载，已有专门的管理面板 |

### 4. 结论

1. **监控已经商品化。** 实时指标、告警、历史曲线人人都有，单拼探针赢不了 Komari 和免费的 Cloudflare 方案。竞争点转向：多轻、多安全、多好看、能不能零成本跑起来。
2. **用户在用四五个工具拼一套。** 探针 + 节点面板 + 订阅管理 + 续费提醒 + 转发面板，各自登录、各自备份、数据不通。妙妙屋读探针流量、DueGuard 抓商家后台，都是在补这种割裂。
3. **安全成了选型理由。** 哪吒和 Komari 都公布过高危或密集的安全公告，用户开始看重“Agent 只上报、默认不能远程执行、非 root 运行”。
4. **AI 是新入口。** 运维正在从“打开面板点按钮”变成“让 Agent 去查、去做”。哪吒加 MCP、1Panel 改口号、Netcatty 内置 AI 运维，都指向同一件事：面板要成为 AI 可调用、可审计的数据与操作源。
5. **IP 质量与 AI 可用性是线路玩家的新焦点。** 买机器先测能否解锁流媒体和 AI 服务，再决定用途。

## 二、apsp 应该占的位置

**多机玩家的一站式控制台：** 看得清每一台（状态、费用、续费、剩余价值），管得住每条线路（延迟、回程、IP 质量、中转），发得出自己的节点（自建节点 + 订阅聚合 + 多客户端订阅），并且默认安全、能被 AI 安全地调用。

和竞品的差异不在某一项功能，而在**把玩家现在拼起来的四五个工具合成一套数据互通的系统**，同时守住安全底线：

| 用户现在的做法 | apsp 的对应 |
| --- | --- |
| Komari / 哪吒看状态 | 服务器看板与告警（已有） |
| Wallos / DueGuard 管续费 | 资产、续费、剩余价值（已有基础） |
| 3x-ui / s-ui 建节点 | sing-box 多服务器节点、用户与套餐（已有） |
| Sub-Store / 妙妙屋 做订阅 | 订阅来源与链路（已有），**缺多客户端输出** |
| flvx / realm 做中转 | 网络配置中的端口转发（已有底层），**缺面向玩家的中转产品** |
| NodeQuality / IPQuality 测机器 | 网络检测与诊断插件（已有，完整验机暂停） |
| Hermes / Netcatty 让 AI 管机器 | **缺 AI 接口** |

## 三、路线图

### 阶段 1：先让人愿意装（v0.4，约 1–2 个月）

论坛选型的第一道门槛是“装起来简单、占用小、安全”。这一阶段解决“为什么换到 apsp”。

| 事项 | 市场依据 | 完成标准 |
| --- | --- | --- |
| 轻量 Agent 与监控模式 | 大量 NAT / LXC 小鸡；CF-Server-Monitor 支持非 root、单向上报 | Agent 常驻内存给出公开数字；提供“仅监控”模式：只上报，不接受远程命令、终端和文件操作，可非 root 运行；支持 Alpine 与 OpenWrt |
| 低门槛安装 | Komari 与 CF 方案一行命令或零服务器即可 | 新用户从安装面板到看见第一台服务器不超过 5 分钟；评估单二进制 + SQLite 的轻量部署形态（需决定） |
| 从哪吒、Komari 迁移 | 两者用户量最大，且有人因安全事件想离开 | 一键导入服务器列表、分组、到期与价格；提供替换 Agent 的批量命令 |
| 公开状态页与主题 | “点亮地图”、主题市场是用户主动传播的动力 | 公开页可选主题、地图视图、隐藏敏感字段；提供移动端小组件或 PWA |
| 安全作为卖点 | 竞品密集的安全公告 | 公开安全说明（签名升级、最小权限、二次验证、审计）和漏洞披露渠道；默认关闭网页终端与远程命令，按服务器显式开启 |
| 恢复交付管线 | 没有版本就没有用户 | 恢复 CI；面板与 Agent 发布 v0.4.0 已签版本 |

### 阶段 2：做竞品拼不出来的事（v0.5，约 2–3 个月）

| 事项 | 市场依据 | 完成标准 |
| --- | --- | --- |
| VPS 账本 | 续费被当作独立需求；NodeSeek 交易文化看重剩余价值 | 续费日历、多币种月度支出、到期与流量重置提醒；一键生成剩余价值卡片，用于交易或展示 |
| 线路质量中心 | 线路机看延迟、丢包和回程；买机先测解锁 | 三网延迟与丢包长期曲线、回程路由（如 NextTrace）、IP 质量与流媒体及主流 AI 服务的解锁状态集中展示，并有历史对比 |
| 多客户端订阅 | Sub-Store、妙妙屋的核心价值 | 同一份节点与授权输出给 mihomo / Clash、Shadowrocket、Surge、sing-box 等客户端（**需解除现有“不提供 Clash 订阅”的限制**） |
| 中转产品化 | flvx、nyanpass 的长期需求 | 在网络配置的转发能力上，提供面向玩家的“中转规则”：入口机 → 落地机、按规则计流量、到期与限额（**需确认转发范围**） |

### 阶段 3：成为 AI 可调用的控制台（v0.6，约 2 个月）

| 事项 | 市场依据 | 完成标准 |
| --- | --- | --- |
| 只读 MCP 接口 | 哪吒已提供 MCP；用户用 Hermes、Netcatty 做运维 | AI Agent 可读取服务器状态、告警、流量、到期、线路质量；沿用 API 令牌的服务器范围与审计 |
| 受控的 AI 操作 | 用户希望 AI 能做事，但怕失控 | 写操作只能调用已有的固定步骤任务（重启服务、部署重试、续费记账等），高风险操作要求管理员二次确认，全部写入审计；不开放任意命令 |
| AI 负载的服务检查 | VPS 上大量运行 AI 网关与 Agent | 内置 CLIProxyAPI、sub2api、Hermes 等常见服务的存活与端口检查模板，异常时按现有渠道告警 |

不做：不内置聊天机器人，不托管模型，不代管 AI 账号。AI 能力以接口形式开放给用户自己的 Agent。

### 持续进行

- **轻与快：** 每个版本公布面板与 Agent 的内存、安装时间和首屏时间，变差即视为回归。
- **社区：** 在 NodeSeek、LINUX DO 发布版本说明和迁移指南，收集反馈进入候选池。
- **安全：** 每个版本附安全变更说明；依赖与 Agent 制品保持签名和可复现记录。

## 四、明确不做

市场上有需求但不适合 apsp 的方向：

| 方向 | 原因 |
| --- | --- |
| 机场售卖、支付、工单（V2Board / Xboard 方向） | 定位是自用和小圈子；引入支付会带来合规与运营负担 |
| 建站与应用商店（1Panel / 宝塔方向） | 会稀释“多机控制台”的定位；需要时在服务器上另装 |
| 自研 AI 聊天或托管模型 | 用户已有自己的 Agent；apsp 提供数据与受控操作即可 |
| 多内核代理（Xray 等） | 维护成本高；sing-box 已覆盖主流协议，优先做好多客户端订阅 |

## 五、需要用户决定

| 问题 | 建议 | 影响 |
| --- | --- | --- |
| 是否解除“不提供 Clash 订阅”的限制 | 解除，统一做多客户端订阅输出 | 修改 AGENTS.md 与 ADR 0076 的范围约束 |
| 中转规则做到什么程度 | 做“入口 → 落地”的规则、计量与限额，不做售卖 | 修改转发相关范围约束，复用网络配置的执行与回退 |
| 是否提供单二进制 + SQLite 的轻量形态 | 先评估：面板当前依赖 PostgreSQL，改动大 | 可能需要新的存储层 ADR |
| 是否开放 MCP | 先只读，写操作在阶段 3 后半再开 | 新增接口与审计范围 |
| “仅监控”Agent 是否作为默认 | 是；终端与远程命令按服务器显式开启 | 安装流程与文档调整 |

## 出处

- [LINUX DO：手上十几台不同厂商的 VPS，想统一监控](https://linux.do/t/topic/3020162)（2026-10-13）
- [LINUX DO：收集一下佬们使用 VPS 的用途](https://linux.do/t/topic/3012326)（2026-10-12）
- [NodeSeek：komari VS 哪吒](https://www.nodeseek.com/post-905934-1)（2026-09-01）
- [NodeSeek：转发面板讨论](https://www.nodeseek.com/post-600301-1)、[NodeSeek：节点多了怎么管理](https://www.nodeseek.com/post-293173-4)
- [V2EX：有没有轻量级的多服务器监控工具](https://www.v2ex.com/t/1110504)（2025-02）
- [VPS DueGuard](https://blog.cxing.top/p/vps-dueguard/)、[NodeQuality 发布帖](https://www.nodeseek.com/post-297125-1)
- GitHub：[Beszel](https://github.com/henrygd/beszel)、[哪吒](https://github.com/nezhahq/nezha)（含安全公告与 MCP 代码）、[Komari](https://github.com/komari-monitor/komari)（含安全公告）、[CF-Server-Monitor](https://github.com/huilang-me/CF-Server-Monitor)、[NodeGet](https://github.com/NodeSeekDev/NodeGet)、[3x-ui](https://github.com/MHSanaei/3x-ui)、[s-ui](https://github.com/alireza0/s-ui)、[Sub-Store](https://github.com/sub-store-org/Sub-Store)、[妙妙屋](https://github.com/iluobei/miaomiaowu)、[Netcatty](https://github.com/binaricat/Netcatty)、[1Panel](https://github.com/1Panel-dev/1Panel)、[CLIProxyAPI](https://github.com/router-for-me/CLIProxyAPI)、[sub2api](https://github.com/Wei-Shaw/sub2api)、[Wallos](https://github.com/ellite/Wallos)
