# 看板缓存与分钟对比存储优化验收

日期：2026-11-07。源码以 `199777bd` 为基础，本记录与实现一起提交到 `main`；决策见 [ADR 0123](../adr/0123-dashboard-cache-and-comparison-storage.md)。

## 改动与结果

- 看板收到旧响应时保留新样本，页面与内存缓存保存同一结果及时间。1440、390、320 像素下，CPU 73% → 收到旧 11% → 离开 → 返回且暂缓新响应，始终保持 73%，修复此前三个尺寸都回退为 11% 的问题。短暂隐藏、提前续费、布局和公开范围清理回归通过。
- 分钟对比仅保存 CPU、内存、磁盘、负载、交换区、TCP/UDP 连接数七项非负数值。数据库用例覆盖相同分钟内的 10%、40% 和乱序的 100%：均值仍为 50%，分钟最后观测仍为 40%；重复上传不重复聚合，下一分钟无 CPU 值返回 null，零和大于 JavaScript 安全整数的原始整数准确保留。
- 含 16 KiB 扩展字段的测试样本，其分钟记录 `pg_column_size` 小于完整 JSON 的十分之一；完整 `telemetry_samples` 和 `servers.latest_metrics` 仍与原始 JSON 一致。此为夹具上的记录大小验证，不是生产整体数据库空间节省率。现有历史随原保留期限过期，没有一次性清理生产数据。
- 两套表服务不同口径，不能用均值直接代替最后观测；本次减少重复大 JSON，而非删除当前业务表。PostgreSQL 初始化 SQL 和 Agent SQLite 基线均无改动，不需要数据库迁移。
- 上游 #248 测试的 SQLite `pragma_query_value(None, "user_version", ...)` 补充精确表达式例外，使全仓分层检查通过。

## 本地验证

使用本地 Bun 1.4.4、Rust/Cargo stable 和 PostgreSQL 18，依赖按现有锁文件离线使用，数据库仅监听回环端口 55439。

| 范围 | 结果 |
| --- | --- |
| Bun 前端单元测试 | 248 通过 |
| TypeScript / Vite | 通过 |
| `notification_rules` | 3 通过 |
| `server_operations` | 7 通过 |
| `server_traffic` | 6 通过 |
| `telemetry_history`（含新增投影回归） | 6 通过 |
| `dashboard-cache.mjs` | 1440 / 390 / 320 通过 |
| `dashboard.mjs` | 1440 / 768 / 390 / 320 通过 |
| `display-data.mjs`、`server-display.mjs` | 各 1440 / 390 / 320 通过 |
| `connection-resilience.mjs` | 9 个连接、首次加载、导航、公开看板和订阅操作夹具场景通过 |
| Clippy | `apsp-panel-host --all-targets`、`apsp-panel --test telemetry_history`，`-D warnings` 通过 |
| 格式、分层、产物、补丁空白检查 | 通过；分层检查器自身 13 项测试通过 |

所有浏览器 API 使用隔离替身，没有生产写请求或真实外部通知。新增浏览器夹具为合成数值，不依赖 NodeFlare 数据库或开发机绝对路径。

本轮未执行整个 Rust 工作区测试、旧 WebKit/Firefox、生产规模压力测试、真实设备、数据库迁移、部署或远端 CI。当前记录只证明上述本地范围。

## 材料清理

结果记录后停止本轮 `.local/dashboard-optimization/postgres/data` 实例，删除该目录中的隔离库、编译目标和临时日志；删除本轮安装的 `web/node_modules` 与 TypeScript 构建缓存。保留源码测试、本文和最新 `web/dist`（Git 忽略）。未操作其他项目数据库、缓存或服务，不提交构建产物。
