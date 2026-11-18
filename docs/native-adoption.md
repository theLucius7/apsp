# 迁移成员采用原生订阅

> **两层事实区分：**（1）源码层已完成九领域收口、一体化自带能力 Agent（`apsp-agent` `0.4.0`，编译期内嵌同平台 `sing-box` `1.14.4-apsp.2` 与受控诊断载荷，移除面板独立制品库与运行时手工启用层，见 [ADR 0140](adr/0140-domain-consolidation.md)、[ADR 0141](adr/0141-self-contained-agent-and-desired-service-config.md)）及先预览后原子提交的成员原生采用实现（含兼容桥交换门禁 `native_control_ready` 与计量锚点表 `singbox_native_adoptions`）；（2）新版一体化 Agent 的正式签名 GitHub Release 尚未构建发布，生产上的存量设备整包升级、Nexus 成员原生采用、公开原订阅入口反代切换与兼容桥退役均**尚未执行**——待正式签发与上线读回核对，不凭源码实现视为已通过实机或生产上线。

APSP 的原生采用保留成员的 UUID、订阅令牌与地址，并把当前周期的原计量读数和账本位置固定为一次接管锚点。后续增量由原生账本计算；换周期或换套餐后使用普通原生规则。采用不会重置用户额度、开启暂停用户或给无套餐用户发放权限。

## 执行条件

先部署本次基线接入脚本（`scripts/baseline-adoption-native-control-plane-20261117.sql`，接入本身不切换任何成员）与同一版本的面板、兼容工具（`tools/nexus_compatibility.py`，必须以 `begin` / `exchange` / `complete` 完整包裹每轮成员同步以维持 `native_control_ready=true`；未包裹的旧交换会自动置回 `false` 并拒绝采用）。成员原权限组涉及的全部线路必须已由健康的 APSP 运行时接管，线路和真实客户端正常；若服务器仍为旧版非一体化 Agent（状态为 `upgrade_required`）或策略组存在未应用节点，须先完成已签名一体化 Agent 升级与目标配置同步。保留最新私有备份，旧地址的入口应已能反代到 APSP，不改变用户保存的 URL。

普通用户的 `/sub/{token}`、个人随机地址和已配置的原订阅入口由现有订阅网关交付。尚未采用的成员继续原流程；已采用的成员使用 APSP 编译器。入口的主机、路径和查询参数必须匹配配置，不能把任意 Host 或令牌当作已授权地址。

## 预览与采用

在用户的现有订阅界面使用原生采用入口。预览和提交均要求具有服务器范围的 `proxy:write` 权限及近期交互式身份验证；预览含连接凭据，不能写入公开日志、工单或仓库。

1. 生成预览，读取阻塞原因、原订阅地址、当前权益及可比较的客户端格式。
2. 与原渲染器的实际输出比较节点名、地址、端口、UUID、SNI、传输配置和响应头用量。确认实际使用的格式；暂停、耗尽、到期或无套餐用户比较的是相同拒绝状态，不能为了迁移临时启用。
3. 确认旧订阅入口已代理到 APSP 后提交当前预览。配置或授权改变会使摘要失效，必须重新比较，不能重用旧预览。
4. 核对同一 URL 的客户端结果、原生发布状态和后续账本增量；新旧控制面不能同时扣减同一份用量。

接口与现有界面使用相同路径：

| 方法与路径 | 用途 |
| --- | --- |
| `POST /api/access/users/{id}/native-adoption/preview` | 只读影子输出、摘要、阻塞原因和既有地址 |
| `POST /api/access/users/{id}/native-adoption/adopt` | 校验摘要、已比较格式和入口确认后原子采用 |
| `POST /api/access/native-adoption/retire` | 所有者在全部依赖解除且旧端停用已确认后关闭桥 |

提交体含 `expected_digest`、`compared_formats`、`comparison_confirmed`、`old_subscription_route_ready`。重复提交同一份已采用预览不会再次写入计量锚点；不同摘要不能覆盖已采用状态。

## 关闭旧桥

已采用成员不再参与旧成员写入或用量投递，兼容工具将旧账号停用并回传确认。未确认的用量、成员交换或旧端停用会阻止退役。只有退役接口成功、读回成员服务来源与旧端状态后，才停止兼容服务。

原身份、历史账本和保留旧 URL 所需的映射不随关闭桥删除。确认恢复边界后，再按所有者的保留策略处理已经无读写方的数据，不通过删除旧表来掩盖未完成的切换。

## 设备升级工具

`tools/fleet-upgrade.py` 按显式服务器顺序推进一体化签名 Agent 升级，默认只读输出 `fleet_plan` JSON，只有 `--apply` 修改设备设置或发起运行时实时核对。工具**只接受目标稳定 Agent 版本 `--agent-version`**，不设独立的 `--runtime-version` 参数：期望内置 `sing-box` 版本与摘要严格从面板已验签的自我包含 Agent 发布清单及升级后设备上报的 `self_contained_agent`、`static_info.components["sing-box"]`、`static_info.component_digests` 派生并交叉核验（历史 `0.3.9` 及更早发布不含一体化组件清单，正式升级须先按[发布流程](release.md)构建签署并收录 `0.4.0` 或更新的稳定版）。登录材料使用当前用户独有、权限 `0600` 的私有 JSON（或 `APSP_PASSWORD`），不在命令行传密码。

```sh
python3 tools/fleet-upgrade.py \
  --api-url https://panel.example.com \
  --credentials /private/apsp-login.json \
  --agent-version 0.4.0 \
  --servers 16 17 18 3 6 7 1 2 5 13 14 15 19 8 21
```

执行同一计划时增加 `--apply --receipt /private/apsp-upgrade.json --client-check /private/check-apsp-client`；升级前需顺带收录面板签名目标时增加显式 `--import-targets <目标ABI ...>`（调用 `POST /api/system/agent-releases/import` 并核验 `signature_verified`）。已配置有效代理服务（`configured=true`）的服务器必须提供 `--client-check`：工具在核验新 Agent 在线、恢复原 `auto_update`、确认目标修订健康并通过 `POST /api/servers/{id}/runtime-control/checkpoint`（`{"module":"singbox"}`）实时核对后，以 `<服务器编号> <派生的内置运行时版本>` 直接执行该程序（不经 shell）验证该台实际客户端线路；纯监控机（`configured=false`）仅核验 Agent 版本与在线状态，不调用客户端检查。任何超时、候选漂移、检查失败或中断均停止剩余目标并恢复原 `auto_update` 设置；恢复失败在私有回执保留 `restore_required`，以同一计划重试会先恢复设置再执行。

当前 API 的自动更新按目标平台选择最高已签稳定版本，没有单机原子版本锁；工具在启用前和等待期间持续核对候选必须等于 `--agent-version` 并拒绝漂移，执行期间不得并发导入其他 Agent 发布。这个操作约束不能当作已实现发布与生产采用的原子解耦。

## 平台受支持能力与 NodeQuality 许可门禁

- **Unix / macOS / Windows 平台能力边界**：Linux（`linux-musl-amd64`/`arm64` 及兼容旧键 `amd64`/`arm64`、`linux-gnu-amd64`/`arm64`，`systemd` 或 `OpenRC`）、macOS（`macos-arm64`，`launchd`）、FreeBSD（`freebsd-amd64`/`arm64`，`rc.d`，普通账户运行，默认使用 `1024` 以上端口，且无 Cronet 故不支持 Naive 出站）与 Windows（`windows-amd64`/`arm64`，计划任务，随包带已签 `libcronet.dll`）均支持验签接入、指标与持续拨测、本地显式开启的远程命令（`allow_remote_commands`，默认关闭）、带试用回退的自动更新以及由有效服务配置驱动的内置 `sing-box`（`1.14.4-apsp.2`）独立服务管理。服务端受控诊断组件（`-r22` `nodequality`、`ipquality`、`network-workbench` 与内置 `apsp-agent tcp-probe`）**仅在 Linux 构建内嵌并注册适配器**（Linux 正式发布在 `APSP_RELEASE_EMBED_REQUIRED=1` 下强制四组件闭环）；**macOS、FreeBSD 与 Windows 不内嵌、不注册上述诊断适配器**，按实上报组件而不伪造就绪。在 Linux 上，诊断启动还要求 `systemd`/cgroup v2 的 CPU 硬上限与 swap 系统调用保护，**OpenRC 主机无法证明该保护，Agent 一律拒绝创建诊断服务或运行负载**。macOS、FreeBSD、Windows 的正式签名制品与实机服务安装，以及新版 Linux 一体化 Agent 的正式签名发布，均待按流程单独构建签署与验收（详见[设备平台与能力](platforms.md)）。
- **NodeQuality 完整验机与工具许可门禁**：Linux 内嵌 `-r22` `nodequality` 仅固化 AGPL-3.0 首层脚本与离线基础运行环境，面板与 Agent 的 **NodeQuality 完整验机启动门禁（`FULL_START_DENIAL` / `diagnostic:nodequality-full-start-gate`）仍保持关闭**，`mode="full"`（含网络检测工作台中的完整验机项）一律拒绝启动，当前仅允许 Linux `systemd`/cgroup v2 下的 `mode="daily"` 日常检查、独立最小派生的 `ipquality` 及历史报告读取、回收与确认式取消。Geekbench Pro、Ookla Speedtest 等第三方工具的分发/商业使用许可、二级离线载荷与禁未授权上传/改 swap 证明及专用测试机 P0→P1→P2 顺序实机总验均未解除（见[整改顺序与验收状态](acceptance/ordered-remediation.md)）；`network-workbench` 仅内嵌许可证与来源哈希齐备的开源工具（`ping`、`mtr`、`iperf3`、`sysbench`、`fio`、`stress-ng`、`smartctl`、`traceroute`），外部 `speedtest` 执行或报告导入继续受 `network_workbench_tools.licensed` 与 `license_acknowledged` 显式许可门禁约束，不凭内嵌归档或界面入口视为完整验机已获许可或已签收。
