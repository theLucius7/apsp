# 2026-10-26 00:00 每小时维护验收

集成输入为 PR #190 `f0eeb0338f683dbac3c2d64fe0077df71190a7c6` 与 main `f6e362caed46a540770bff5f9d73d55354ac5412`。复用原 PR，合并进度记录，保留两项功能并将成员线路 ADR 顺延为 0101。兼容桥同时保留线路状态导出和 main 的旧订阅心跳更新；补齐返回合同及组合回归。选择与理由见 `docs/open-questions.md`。

验证时暂存树为 `8ca594013642f1d8391b7632f94aa7ac63628e33`；随后只补充本验收与进度结果。源码索引摘要为 `4febc2ae47a83a655bb841558b37c2617fc47d5d802043915cae38ba71adb533`，计算方式是对 `git ls-files -s` 中 `crates/`、`plugins/`、`web/`、`tools/`、`tests/`、`scripts/`、`deploy/`、`Cargo.toml` 和 `Cargo.lock` 条目按原顺序连接（保留末尾换行）后计算 SHA-256。

| 本轮本地验证 | 结果 |
| --- | --- |
| `python3 -m unittest discover -s tools -p 'test_nexus*.py'` | 38 通过，含新鲜心跳、缓存拒绝、状态导出和可选状态写入失败的组合回归 |
| `cargo test --locked -p apsp-plugin-singbox --lib` | 121 通过 |
| `cargo test --locked -p apsp-panel --test member_lines --test platform_artifacts` | 成员线路 1 项、运行时制品选择 2 项通过，使用独立 PostgreSQL 16 容器 |
| `cargo clippy --locked -p apsp-plugin-singbox --all-targets -- -D warnings` | 通过 |
| `cargo fmt --all --check`、架构边界、diff 与 ADR 引用检查 | 通过 |

Rust 使用 PATH 中的工具链，构建目录和测试临时目录均位于本任务运行目录。使用仓库公开 TEST_ONLY 密钥夹具，缺少 pkg-config 时显式指定既有 OpenSSL 头文件与库目录。

前端行为与已验提交 `f0eeb033` 相同，本轮仅更新两处 ADR 注释；`web/dist` 完全相同。复用 2026-10-25 21:00 维护摘要中的 209 项前端单测、构建与 dist 一致性，以及 `member-lines`、`member-commerce` 两组 1440/390/320 宽度浏览器验收，不重复运行未受影响范围。

未验证范围：本轮未运行 GitHub Actions、全工作区 Rust 测试、真实设备、正式签名发布、生产兼容桥更新或面板部署。CI 的 Agent 0.3.3 临时例外归原发布会话，本轮不启用或触发工作流；提交保留 `[skip ci]`。生产兼容桥未更新时，旧节点线路可以显示「状态未知」。面板上线由独立 `apsp-deploy.timer` 在 main 更新后接续。

清理：已保存源码身份、命令和结果摘要，移除本轮标签为 `apsp.maintenance=20261026-0000` 的临时数据库容器、编译目录、临时文件、Python 缓存与原始日志。未改动原有暂存工作、其他项目、生产卷或凭据；本记录保留结果，不承诺原始测试材料持续存在。
