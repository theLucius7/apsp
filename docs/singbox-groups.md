# sing-box 策略与套餐使用说明

策略组与套餐组位于 **sing-box 插件 → 策略与套餐**；直连节点、订阅来源和有序链路在 **代理节点** 内管理，不属于服务器的通用监控配置。代理用户详情分别分配策略组和套餐。实现约束见 [ADR 0035](adr/0035-singbox-policy-package-groups.md) 与 [资源生命周期 ADR 0071](adr/0071-proxy-resource-batch-lifecycle.md)。

订阅来源的获取、导入、不可变版本，以及受管节点与订阅节点混合的有序链路已经实现。有序链路由一个受管公开入口和其后 1 至 8 个代理跳组成，订阅中的具体节点可以作为中间跳或出口；来源本身不是服务器，也不是对用户开放的入口。来源生命周期见 [ADR 0072](adr/0072-subscription-source-lifecycle.md) 和[订阅来源规则](chain-subscription-sources.md)，完整路径约束见 [ADR 0073](adr/0073-ordered-path-publication-and-native-probe.md)。源码、本地回归与原生流量证据见[有序链路验收记录](acceptance/ordered-paths.md)；真实 Agent 托管整链与正式部署仍待验。

## 使用顺序

先登记服务器与 Agent，再按[安装流程](singbox-installation.md)由管理员启用 sing-box 插件，等待签名运行时和配置的设备应用确认，然后在插件中创建节点、代理用户和策略授权。core 管服务器、心跳与网卡总流量；面板的 sing-box 插件管代理节点、用户、授权、订阅、套餐与链路发布，Agent 通用框架和 sing-box 适配器完成签名验证、配置应用及运行确认。普通直连节点支持现有协议，有序链路使用受管 Reality 入口及受管跳，也可选择订阅来源中的具体不可变节点版本。单批至多 32 条链路，每条入口后有 1 至 8 个有序跳；任一非法项整批回滚，创建不授予用户权限。旧两跳批量请求和幂等收据仍兼容，继续由原数值链路发布处理；新有序链路由有序发布与验证处理，两套路径在同一事务中形成同一个配置 revision。安装、保存与设备应用仍分别展示，真实整链验收单独记录。

统一资源列表将链路作为一个资源，专用入口不再列作可单独授权的直连，共享出口保留引用说明。删除完整链路资源同时清理专用入口而保留共享出口；有策略引用时先明确移除。结果不确定时重试保留同一批提交键和请求，删除后重放只返回原创建 ID，不复活资源。配置、设备应用和路径连通分别展示，不将结构可用当作业务健康。

创建策略组，勾选可以直接连接的节点，以及通过入口连接的链路。例如“常用节点”包含两个普通节点与一条两跳链路。然后到代理用户详情勾选策略组并保存。一个用户可以使用多个组；删除某一组分配不影响其他组或单独授权仍提供的节点。链路入口只通过链路策略授权，不能打开单独节点授权开关。

创建套餐组，例如每月 500 GiB、每月 1 日 00:00 按 Asia/Taipei 重置、使用 365 天。在代理用户详情点击“分配或更换套餐”并确认。用户拥有节点权限且套餐可用时才能获得可用订阅；只分配套餐不授予任何节点。

旧用户以及尚未分配套餐的新用户维持不限制额度和期限的兼容模式，页面显示“未设置流量与到期限制”。需要限制用户时必须明确分配套餐，不能仅创建套餐模板。

## 流量与期限

额度按用户所有普通节点与链路入口的上传、下载之和计算，各用户独立计量。链路内部转发身份不再次扣量。流量留空表示不限量，但仍受套餐有效期约束；0 字节不是不限量，会被拒绝。界面支持 GiB 或精确字节，API 使用十进制字符串避免超出 JavaScript 整数精度。

每月重置与套餐到期是两件不同的事。设置每月 31 日时，2026 年 2 月在 28 日重置，3 月仍在 31 日重置，不会永久漂移到 28 日。时区按 PostgreSQL IANA 规则计算；夏令时不存在的本地时刻按 PostgreSQL 标准时间解释后向前对应到有效时刻，重复时刻选标准时间对应的那次。不使用固定“30 天”代替日历月。

分配有效期从确认时刻开始，每天为 24 小时。更换套餐从当前时刻重新计算，不是在原有到期日上续加；提交前界面会提示。编辑模板或删除模板只影响之后的分配，已分配用户保留原有快照。

更换或重复分配套餐不会清空本期用量；首次分配也包括本日历周期已记录的用量。换用不同重置规则时，当前用量按新的日历边界重新汇总。没有用量结转、折算、购买订单或自动续期功能。

计量流水永远保留。跨重置时刻的一个批次按排他结束时刻前一秒整批归属，不能准确拆分批次内每个字节的时间。旧批次迟到/重放只影响其原周期。设备重启导致的计量 epoch 变化不会重置套餐。

## 到期、耗尽与设备状态

用户详情展示当前套餐快照、本期起点、已用流量、下次重置、到期时刻以及使用中/耗尽/到期状态。用完或到期后，订阅不再提供可用节点；服务端发布同样移除用户，不只是隐藏链接。下个周期恢复流量但不延长到期时间。

限制不是逐字节即时断流。设备尚未上报的流量、正常的 5 秒配置发布 debounce、设备应用与离线重连都会产生延迟，可能超出额度。没有两端在线成功应用的链路不进入订阅。订阅凭据泄露、离线旧配置与正在进行的连接仍依赖设备实际应用来撤销；不能承诺无损更新或离线立即停用。

## API

所有管理接口位于 `/api/plugins/sing-box`，沿用管理员会话与请求安全校验。新增资源如下：

| 接口 | 方法 | 内容 |
| --- | --- | --- |
| `/policy-groups` | GET / POST | 列出、创建策略组 |
| `/policy-groups/{id}` | PUT / DELETE | 完整替换策略组、删除未分配的组 |
| `/chains`、`/chains/{id}` | GET / POST、DELETE | 列出、创建、删除两跳链路 |
| `/proxy-resources`、`/proxy-resources/{kind}/{id}` | GET、GET / DELETE | 统一公开资源与完整资源删除；kind 为 direct 或 chain |
| `/chains/batch` | POST | 原子批量创建独立入口与两跳，绑定提交键 |
| `/package-groups` | GET / POST | 列出未归档模板、创建模板 |
| `/package-groups/{id}` | PUT / DELETE | 编辑模板、归档模板 |
| `/users/{id}/policy-groups` | GET / PUT | 读取或完整替换用户的策略组集合 |
| `/users/{id}/package` | POST | 幂等分配一个当前套餐快照 |
| `/users/{id}/entitlement` | GET | 当前套餐、周期、用量与资格 |

策略组请求：`{"name":"常用节点","node_ids":[1,2],"chain_ids":[1]}`。用户分配：`{"group_ids":[1,2]}`。集合按并集去重，空集合不授予新权限。旧单独授权仍由 `/users/{id}/accesses` 管理，返回值新增 `direct_grant`；撤销该接口只移除单独授权来源。

套餐模板请求：

```json
{
  "name": "月度 500 GiB",
  "monthly_bytes": "536870912000",
  "reset_day": 1,
  "reset_hour": 0,
  "reset_minute": 0,
  "timezone": "Asia/Taipei",
  "duration_days": 365
}
```

套餐分配请求：`{"package_group_id":1,"request_id":"<客户端新生成的 UUID>"}`。同一次提交失败重试必须复用 request_id。相同键、相同套餐返回原结果；相同键改套餐返回冲突。旧请求晚到不覆盖之后的新套餐。开始、到期和周期时间戳全部使用 Unix 秒，字节数字全部用字符串返回。

旧链路请求：`{"name":"入口到出口","entry_node_id":1,"exit_node_id":2}`。旧 DELETE 仍只解除关系并保留入口；界面使用新资源 DELETE 清理整条链路及专用入口。批量结构见 [API 文档](api.md#统一代理资源与批量两跳)。入口、出口保持不可变；改变拓扑需先从策略组移除旧链路，再删除／新建。链路或策略组存在引用时删除返回 409 清单，不自动扩大或改变用户权限。

## 本地验证与上线边界

使用隔离 PostgreSQL，禁止使用生产数据库运行集成测试。测试根只使用仓库提供的公开 TEST_ONLY 根。

```sh
export DATABASE_URL='postgres://apsp:apsp-test@127.0.0.1:5432/apsp_test'
export APSP_RELEASE_PUBLIC_KEYS="$(python3 scripts/ci-test-trust.py)"
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked -p apsp-panel --test singbox_groups
cargo test --locked -p apsp-compiler
# 原生配置检查需显式提供仓库固定的上游 1.14.2 构建。
APSP_GROUPS_RUNTIME=/path/to/sing-box-1.14.2 cargo test --locked -p apsp-compiler \
  --test relays pinned_native_runtime_accepts_entry_exit_and_client_configs -- --ignored
```

`web` 目录使用 `bun install --frozen-lockfile && bun run build`。浏览器用例 `tools/test-singbox-groups-ui.cjs` 面向构建目录的回环 HTTP 静态服务，使用 Playwright/Chromium 和隔离 API 夹具；可设置 `PLAYWRIGHT_MODULE`，再运行 `node tools/test-singbox-groups-ui.cjs http://127.0.0.1:4176`。它不会访问正式面板。原生配置检查与浏览器夹具不替代真实双机链路、长连接、计量延迟与离线恢复的上线验收。

迁移 `0016` 接在主线的现代协议 `0014` 与拨测索引 `0015` 后，必须与新版面板一起部署，不能混用新旧发布进程。此前 PR 草稿的同名 `0014` 只用于本地隔离验证，已改编号以避免碰撞；使用过旧草稿的临时测试库需重建，不作为正式升级路径。迁移后单独回滚旧面板会绕过新套餐资格，禁止这样回滚。PR 不修改正式服务、执行生产迁移、恢复暂停 CI 或发布新的 Agent/内核版本。
