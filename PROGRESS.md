# 当前交付与验证

## 2026-11-18 一体化自带能力 Agent 重构与待集中验收

延续本次领域整合交付，按用户“去掉独立制品与手工安装层、由单一 APSP Agent 自带能力并连接面板接配置”的要求完成重构，决定见 [ADR 0141](docs/adr/0141-self-contained-agent-and-desired-service-config.md)，操作说明对齐见 [成员原生采用与设备升级](docs/native-adoption.md)。

- `apsp-agent`（`0.4.0`，单文件已签 `raw` 二进制 `<= 128 MiB`）编译期内嵌同 ABI `sing-box`（`1.14.4-apsp.2`）与受控诊断载荷：Linux 正式发布构建（`APSP_RELEASE_EMBED_REQUIRED=1`）强制内嵌 `sing-box`、`-r22` `nodequality`、`ipquality`、`network-workbench` 四组件闭环及内置 `apsp-agent tcp-probe`；macOS、FreeBSD、Windows 内嵌原生 `sing-box` 并如实省略 Linux 专用诊断适配器；NodeQuality 完整验机（`mode="full"`）与第三方商业/分发工具许可门禁保持关闭，OpenRC 无 `systemd`/cgroup v2 硬保护时继续拒绝启动诊断负载。
- 移除面板独立制品目录与设备制品下载（`/api/artifacts`、`/#/settings/artifacts`、`/api/agent/v1/artifacts/...`）、运行时库存/灰度及手工启用（`runtime_enablements`、预检手工确认流），收敛为 `/api/system/agent-releases*` 与 `StaticInfo` 组件身份校验；有效服务配置（启用节点、受管链路跳或 `active` 切流主机）作为唯一运行期望（`configured`），补齐 Supervisor 整包试用回退与同修订不健康自动恢复；面板端 Cloudflare DNS-01 内置只读校验的官方 `lego` `5.6.0`。
- 配套数据库基线接入脚本 `scripts/baseline-adoption-native-control-plane-20261117.sql`（含隔离演练驱动 `scripts/rehearse-native-control-plane-baseline.py`）；`tools/fleet-upgrade.py` 与操作文档对齐为只传 `--agent-version`，从已签一体化 Agent 发布清单与设备上报摘要自动派生核验内置运行时并发起实时 `checkpoint` 及外部客户端检查。
- 待集中验收：当前处于整步修改阶段，尚未运行冻结后的集中本地验证（`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`、核心与领域边界及路由权限守卫、前端构建与测试夹具、隔离 PG16 基线接入演练均待全部修改冻结后统一执行补记）；四个 GitHub Actions 工作流保持暂停，记为未验证。
- 未发布与未上线边界：包含四组件闭环的 `agent-v0.4.0` 正式签名 Release 尚未构建发布，macOS/FreeBSD/Windows 正式签名制品与实机服务验收未执行；生产库基线接入、存量服务器整包升级、Nexus 成员原生采用、公开原订阅入口反代切换与兼容桥退役均未执行，不凭源码实现记为已通过或已上线。

## 2026-11-17 领域整合与历史资料清理

用户要求一次性完成整合、不再按插件维护、删除历史冗余。本次集中实现、统一验收，以同一个整合变更交付，决定见 [ADR 0140](docs/adr/0140-domain-consolidation.md)。

- 内部制品描述使用 `artifact_name`，诊断任务使用 `tool`；序列化边界保留已发布 Agent、检查点和签名报告的精确字段，设备路径与 SIP003 协议字段不变。
- 控制台诊断清单、运行时启用、供应商条件和迁移影响统一使用现行对象名，后端、界面、调用和夹具同步。
- 运行时附件门禁只检查所选签名制品，状态区分当前配置健康、配置同步受阻与 Agent 待升级。
- 代理用量统计的账本读取与聚合迁入服务权益，观测保留鉴权、时间窗口和呈现组合；查询快照、统计口径与接口保持。
- 迁移成员的原生订阅与用量采用在同一变更实现，按预览与事务核对保留既有身份、地址和周期用量；生产采用状态单独记录。
- 移除 14 份已被替代的旧任务书、执行草案与逐小时日志，历史引用转到删除前的 Git 版本。架构、路线图、当前问题与文档入口只描述现行状态。
- 验证：集中实现期间尚未运行；完成冻结后统一补记最终结果。
- 生产：本次整合尚未部署；未执行成员采用、Agent 升级、云资源或 DNS 变更，不把源码准备记为生产完成。

## 当前生产基线

2026-11-17 只读核对与 2026-11-18 `origin/main`（[`e78bfcad`](https://github.com/theLucius7/apsp/commit/e78bfcad1f1777e138388f020a4fd5e5be862b6c)）运维更新：Agent 0.3.9 从 `bf71ea39` 签名发布，运行时为 `1.14.4-apsp.2`；四个项目工作流仍停用，0.3.9 发版例外已关闭。

| 范围 | 已知状态 |
| --- | --- |
| hk1/hk2 | Agent 0.3.9，加固运行时摘要正确，conntrack 参数已生效；2026-11-18 因原第三方目标站香港节点拒连，已改为每台自建 `nginx` + Let's Encrypt 站点（Cloudflare 仅 DNS `hk1.solve.ac` / `hk2.solve.ac`，本机 `127.0.0.1:8443` 提供 TLS 1.3 + h2 + `X25519MLKEM768`），台湾线路 `extend --revise`（修订 7 / 15）改 SNI 为各自域名、握手目标为本机 `127.0.0.1:8443`，`nodes.sni`、旧面板与用户订阅已同步，真实客户端（`sing-box`、Mihomo 带 MLKEM）台湾出口正常，握手额外延迟 +0.6 ms / +2 ms，国内 TCP 443 均 6/6；**换 IP 必须同步改对应 DNS 记录** |
| hk4 | 新接入 `aliyun-hk4`（server 22），Agent 0.3.9 在线，尚无节点；待用户确定出口后再建节点、`hk4.solve.ac` 与本机自建站点 |
| 其余在线运行时 | 14 台清单准备失败，13 台缺附件能力，1 台缺平台制品；现有配置仍运行 |
| 仅监控设备 | server 21 在线，Agent 0.3.7，单独按监控职责验收 |
| hk3 与其他被封云 | `aliyun-hk3`（server 20）CDT 关机至 12-01，Agent 0.3.7，恢复开机后单独处理；与被封的 `oracle-sg1/2/3`、`azure-in1`、`azure-kr1`、`huawei-sg1` 后续一律按每台自建站点（`solve.ac`）方式处理，不再挑选第三方目标站 |
| 成员订阅 | 仍有旧系统渲染与兼容桥依赖，不能因新源码存在就视为已退出 |

目标修订等于已应用修订、现有配置健康，不证明已采用新内核或新清单可准备。容器中的运行时需要相应探测，宿主进程列表为空不是服务不存在。

## 已完成的结构结果

| 工作 | 结果依据 |
| --- | --- |
| 平台与九个领域拆分、单一链路和来源模型 | ADR 0128、0131、0132 |
| 后台周期、锁、出站、投递、供应商客户端统一 | ADR 0133 至 0136 |
| 会话、观测、流量发布、成本、作业、资产列归位 | ADR 0137，2026-11-14 已上线 |
| 普通用户使用迁移线路 | ADR 0138，2026-11-17 已上线 |
| REALITY 加固、0.3.9 发布与 hk 自建目标站切换 | ADR 0139、[当前交接](docs/handoff/2026-11-17-reality-hardening.md)、[`e78bfcad`](https://github.com/theLucius7/apsp/commit/e78bfcad1f1777e138388f020a4fd5e5be862b6c) |

历史提交、详细测试次数与当时限制见 [整合前的执行记录](https://github.com/theLucius7/apsp/blob/8fb798b14761681607973604c0ba15e605e18b4c/PROGRESS.md)。旧证据只对应当时源码，不能认证当前版本。

## 当前完成条件

完整目标以 [路线图](docs/roadmap.md) 为准：在线设备可持续收配置，成员原生采用和旧入口核对完成，旧桥退出，核心旅程与恢复经过验收。未执行的实机、生产迁移和 CI 分别列为待验，不因清理历史文档而消失。
