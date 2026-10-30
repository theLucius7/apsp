# 每小时维护集中验收（2026-10-28 19 时）

## 源码与修复

- main 基线：`318fc3ab2ba158d1cc4570d3aecec012f52fa0dd`；#194 原 head：`951747b69f85d1b2043ccbca7740b7086967a9b4`，已包含该 main。
- #190 的 `939ca1b13c6e1daa134924134ea9e9c814f28f38` 仍为草稿且有冲突，保持原状。
- 复用 #194 修复 Clash 转换边界，增加 8 项外部编译入口回归；取舍与依据见 `docs/open-questions.md`。前端、宿主、CDT 与 Passkey 源码未改。

## 验证

最终验证的代码与内嵌产物 SHA-256：`19e26d14e72f258858b69eeb264536a4e3921287e470e790f1aa93e7aa97f664`。按 Git 跟踪路径排序，以路径、NUL、文件内容依次计算，排除 `docs/`、`PROGRESS.md`；结果补记不改变代码。

使用 PATH 中的 Rust/Cargo 1.99.1、Bun 1.4.2、Playwright 1.61.1、独立 PostgreSQL 16 容器与 Chromium 虚拟认证器；编译缓存和测试工具仅写本轮目录，浏览器使用本轮拥有的 `/tmp/apsp-m2819-*` 短目录。

| 验证 | 结果 |
| --- | --- |
| Rust 格式、核心架构边界、diff 检查 | 通过 |
| 编译器全部目标测试 | 71 通过、7 条件忽略；含 8 项新增回归 |
| sing-box 插件单元测试 | 116 通过 |
| 面板宿主、阿里云插件单元测试 | 184、64 通过；这两个模块与验证基线内容相同 |
| alicloud、auth_security、control_center、external_node_access、frontend、modern_protocols、passkeys、platform_artifacts、subscription_preview 集成测试 | 37 通过；默认忽略的浏览器用例随后单独运行 |
| Passkey 虚拟认证器 | 1 项通过，覆盖 29 个场景 |
| compiler、panel-host、plugin-singbox、plugin-alicloud、panel 全目标 Clippy，`-D warnings` | 通过 |
| mihomo v1.19.29 离线 `-t` | 扩充后的 10 节点样例通过；覆盖数组请求头、策略重名与 Unicode 注释边界，不连接代理服务器 |

- 首轮基线验证完成 372 项相关单元测试；发现转换缺陷后主动中止尚未完成的集成编译，不将中止步骤记为通过。集中修复后完整执行上表受影响范围，没有测试失败。
- 7 项编译器忽略测试需要指定版本和构建特征的上游 sing-box 运行时，本轮未执行，不以 mihomo 离线校验替代这些条件验收。
- 修复前代码与产物摘要确认为 `b0452a6673997956e6be15330f9fd326c47e8f48dd257253da49e30acc017091`，与[15 时记录](hourly-maintenance-20261028-1500.md)完全一致。本轮未改前端，保留该记录的生产构建、208 项前端单元与四组浏览器证据；没有重复运行或将其记为本轮新执行的结果。

## 清理与边界

仅使用本任务工作树、缓存与隔离 PostgreSQL；旧专用克隆的暂存状态摘要与开始时相同。保存源码身份和结果摘要后，已停止并删除本轮 PostgreSQL 容器（无持久数据卷），删除本轮编译目录、依赖缓存、浏览器和 mihomo 测试工具、临时补丁副本与原始日志。保留当前源码、内嵌前端及有界摘要；其他轮次和生产数据未清理。本记录证明当时结果，不承诺原始测试环境持续存在。

GitHub Actions 维持暂停。未执行完整 Rust 工作区、真实代理连接、真实认证器、Agent 安装、签名发布、真实诊断或云资源操作。合并与删除以结束前远端核对为准；面板部署由独立 `apsp-deploy.timer` 接续，本任务不重复启动、不修改其凭据和数据卷。
