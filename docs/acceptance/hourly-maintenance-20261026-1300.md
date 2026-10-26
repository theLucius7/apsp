# 2026-10-26 13:00 每小时维护集中验收

输入：main `f6e362caed46a540770bff5f9d73d55354ac5412`、PR #190 `bea60e06c8e183f8c6c6144771e9a37885dc9236`、PR #193 `4ccf5108704667310acd3f1050ac833d1a701c22`。两个 PR 的草稿、目标、审查线程、检查及分支保护均已核查；无拒绝审查或未解决线程，GitHub Actions 暂停且不作为通过证据。

本轮在专用克隆的隔离工作树上集成，保留原克隆未提交工作。进度保留双方段落，前端产物由合并源码重建，取舍见 `docs/open-questions.md`。验证结果如下。

验证时暂存树为 `f8deb3166486459188f43fcf7c0355389044581d`。其后只补充本验收和进度记录。源码索引的 1630 条记录摘要为 `a8b099be19de3a43bd0ec1d74909a9c8cc2345bbfda9eef075fa97bfb9564a95`，计算口径沿用本日 00:00 验收记录。

| 本地验证 | 结果 |
| --- | --- |
| `cargo fmt --all --check`、核心边界和 `git diff --cached --check` | 通过 |
| `cargo test --locked --offline -p apsp-panel-host -p apsp-plugin-singbox --lib` | 宿主 184 项、插件 121 项通过 |
| `cargo test --locked --offline -p apsp-panel --test auth_security --test control_center --test passkeys --test frontend --test member_lines --test platform_artifacts` | 22 项通过；1 项浏览器测试默认忽略，随后单独执行 |
| `cargo test --locked --offline -p apsp-panel --test passkeys virtual_authenticator_browser_roundtrip -- --ignored --nocapture` | 1 项通过，包含 29 个 Chromium 虚拟认证器场景 |
| `cargo clippy --locked --offline -p apsp-panel-host -p apsp-plugin-singbox -p apsp-panel --all-targets -- -D warnings` | 通过 |
| `python3 -m unittest discover -s tools -p 'test_nexus*.py'` | 38 项通过 |
| `bun test`、`bun run build`（`web/`） | 210 项通过，生产构建通过 |
| `bun web/tests/member-lines.mjs`、`bun web/tests/member-commerce.mjs` | 两组均在 1440、390、320 宽度通过 |

使用 PATH 中的 Rust/Bun 与独立 PostgreSQL 16 容器，只用仓库公开 TEST_ONLY 密钥和本地数据库。Cargo、Bun 缓存及构建目录均位于本轮运行目录。显式指定已有 OpenSSL 开发库路径，不安装或修改系统依赖。浏览器首次因临时目录过长而无法创建 Unix socket；改用指向本轮临时目录的短路径符号链接，并将浏览器配置指向本轮目录，重跑成功，无应用源码修改。已通过范围没有重复运行。

本轮未执行 GitHub Actions、完整工作区 Rust 测试、真实设备或真实认证器验收、生产兼容桥更新、签名发布或面板部署。既有全工作区基线失败与忽略项不据此标为通过。旧节点状态仍需要另行更新生产兼容桥；面板部署由现有 `apsp-deploy.timer` 在 main 更新后接续，本轮不启动或修改部署器。

清理在保存结果摘要与源码身份后执行：停止并删除标签为 `apsp.maintenance=20261026-1300` 的测试容器，删除本轮 Cargo/Bun 缓存、编译目录、浏览器临时数据及原始日志，保留源码、最新内嵌前端和本记录。未改动原克隆暂存工作、其他项目缓存、生产卷或凭据；本记录不承诺原始测试材料持续存在。
