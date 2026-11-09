# 2026-10-04 阶段三 S1d：mixed 链路转换与统一创建入口

2026-10-04 整合注：本文保留原分支对应提交与当时验收结果；原迁移 0049–0054 在主线依次顺延为 0059–0064，原 ADR 0077–0079 顺延为 0083–0085。正文的旧迁移号与性能、测试数字属于历史输入，不能认证当前整合源码；当前映射与集中验证见[本次整合记录](branch-main-integration-20261004.md)。

这是阶段三迁移方案第 5.2、5.3 节的实施，起点是提交 `c6bdaef`（S1c）。至此 S1 代码准备全部完成。

所有修改先集中完成并冻结，然后统一验证。远端 CI 继续暂停。合入本身不转换任何链路，也不改变现有配置包；没有访问设备或生产面板。

## 修改内容

- **迁移 `0054_mixed_conversions.sql`（只追加）：**
  - `singbox_mixed_conversions` 记录每条链路的转换状态。
  - `singbox_retired_path_scopes` 保存已退役 mixed 作用域在各服务器上的下限（墓碑），永不删除。
- **转换（`plugins/singbox/panel/mixed_conversion.rs`）：**
  - 检查接口 `GET /proxy-resources/chain/{id}/conversion`：运行全部检查，并在回滚的事务中试建有序候选。
  - 开始接口 `POST …/conversion`：须带上检查时看到的 mixed 活动代。
  - 跳映射：受管跳冻结为端点版本；订阅跳按数字编号找到迁移后的节点和版本。
  - 写入墓碑。
- **交接：**
  - 准备期链路仍是 mixed，有序生命周期在旁准备并探测候选，不改动入口用户。
  - 候选探测通过时切换为 ordered；切换前失败就退回 mixed。
  - 旧代保留在依赖服务器上，直到恢复屏障完成后撤下。
  - 编译层的“同一链路、未启用的候选”例外扩展到 mixed 路由。
  - 只有墓碑的服务器也下发 `runtime-constraints.json`。
- **统一创建入口：**
  - 来源迁移后 `/chains/batch` 返回 409。
  - `POST /chains` 停用，返回 409。
  - 页头“创建链路”在迁移后打开有序链路表单；迁移状态未确认时禁用。
  - mixed 链路详情新增“转为有序链路”，含检查与开始。
- **文档：**
  - 迁移方案第 13 节（含与方案的差异）。
  - ADR 0085 状态。
  - `docs/api.md`：转换接口；修正 `/chains/batch` 与 `/subscription-sources` 的过时描述。

## 统一验证

环境：本机 Rust 1.97.0，PostgreSQL 16（专用临时实例，回环 55432 端口），bun 1.3.14，Chromium（Playwright）。

- `cargo fmt --check`、分层检查、`git diff --check` 通过。
- 全工作区全 targets Clippy（warnings 视为错误）：首轮报新测试中三处可省略的 `clone`，修正后重跑通过。
- Rust 与 PostgreSQL（编译器、sing-box 插件与组装 crate）：首轮 450 项通过、2 项失败、14 项条件忽略。
  - 失败一：一处旧测试的迁移数量断言仍是 53，改为 54。
  - 失败二：新的退回测试在阶段切换后没有先发布并确认设备就等待探测；这是测试步骤问题，补上发布与确认。
  - 只重跑受影响的两个测试目标，全部通过；合计 452 项通过、0 失败。
- 新增或扩展的测试：
  - **编译器：** 入口已有 mixed 路由时，同一链路未启用的有序候选可以编入配置，不加拦截规则，也不路由；要求路由的候选，或其他链路的候选，被拒绝。
  - **转换成功：**
    - 准备期间入口一直路由 mixed 代，候选与它同时编入入口配置，用户订阅持续包含入口。
    - 探测通过后切换；切换期间依赖服务器保留旧身份。
    - 完成后入口只路由有序第 2 代，旧出站和旧身份从两台服务器消失。两台服务器的 `runtime-constraints.json` 仍带 `path-{id}` 的退役下限 1。
    - 用户订阅中的入口凭据不变；mixed 详情返回 404，再次检查报告 `not_mixed`。
    - 检查不写入任何数据。
  - **切换前失败：** 候选探测失败后链路回到 mixed，转换记为 `reverted` 并有原因；候选从入口配置撤下，用户订阅不变；再次转换使用第 3 代，尝试次数为 2。
  - **阻止条件和旧入口：** 未确认的路径、入口的节点策略授权、服务器缺少能力、过期的期望代都被拒绝且不写入；`POST /chains` 和迁移后的 `/chains/batch` 返回 409。
  - **订阅跳映射：** 数字编号来源的订阅跳在迁移前报告 `sources_not_migrated`；迁移后可以转换，有序跳指向同编号的迁移节点与版本，属于对应的有序来源，固定模式保留。
  - 旧两跳创建相关的测试改为断言 409，夹具改用直接导入；迁移数量断言更新为 54。
- 前端：`bun test` 174 项通过；`bun run build` 成功，`web/dist` 已更新（依赖版本与锁文件一致）。
- 浏览器回归 16 项通过：source-migration、mixed-chains、ordered-chains、node-routes、singbox-chain-snapshot-writes、singbox-resource-snapshots、subscription-sources、proxy-resources、node-catalog、node-options、node-settings、singbox-business、singbox-chains-write-guards、confirmed-cancellation、subscription、external-access。
  - source-migration 覆盖迁移后页头打开有序表单、状态未确认时禁用、迁移前仍打开 mixed 编辑器。
  - mixed-chains 覆盖转换检查的原因显示、开始转换的请求内容和转换中的状态，桌面与移动宽度。
- 宿主、DDNS、阿里云和云 API 没有改动，没有重跑。

## 未验证范围

- 远端 CI 继续暂停。
- 设备侧没有实际验证；本地只用 TEST_ONLY 的检查点、探测和屏障回执驱动面板状态机，没有运行原生进程。转换与退回须先在专用测试机上实机验收（方案第 6 节 S6）。
- 没有在真实数据快照上演练转换。
- 生产转换、发布与实机验收另行授权。
