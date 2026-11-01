# 每小时维护集中集成验收（2026-10-26 21 时）

## 源码范围

- main 基线：`f6e362caed46a540770bff5f9d73d55354ac5412`。
- PR #194 Clash 订阅：`3f1ec724758149f500c1d50c1afa7f0231e101f1`；PR #193 Passkey 再次验证：`4ccf5108704667310acd3f1050ac833d1a701c22`。
- 复用 #194 集中交付；#190 仍为草稿，未纳入。保留双方祖先关系与完整进度记录；业务源码自动合并，内嵌前端由整合源码重建，未选择单方产物覆盖。
- 验证时源码树（补记本文前）：`9a8231abfcabed6429c4d6cba523e8f986a891eb`。按 Git 跟踪路径排序，以路径、NUL、文件内容依次计算的代码与产物 SHA-256（排除 docs/、PROGRESS.md）：`90a4f4753fd8740a32a58e24820108424d26279aa329cf3cf699eee1dca0f3b2`。
- 增补浏览器回归：Clash 格式切换、预览、YAML 下载与外部节点格式保持；增补接口回归：显式 singbox 格式覆盖 Clash User-Agent 自动识别。

## 集中本地验证

使用 PATH 中的 Rust/Cargo 1.99.1、Bun 1.4.2，独立 PostgreSQL 16 容器和 Chromium 虚拟认证器；编译输出、临时数据与缓存均在本轮运行目录。显式指定宿主已有 OpenSSL 头文件和库目录，未安装系统依赖。

| 验证 | 结果 |
| --- | --- |
| cargo fmt --all --check、核心边界、git diff --check | 通过 |
| 前端冻结锁文件安装、生产构建 | 通过；web/dist 已重新生成 |
| 前端单元测试 | 207 通过 |
| 编译器 / 面板宿主 / sing-box 插件单元测试 | 8 / 184 / 116 通过 |
| auth_security、control_center、passkeys、frontend、platform_artifacts、modern_protocols、subscription_preview、external_node_access 集成测试 | 30 通过；默认忽略的浏览器测试随后单独运行 |
| Passkey 虚拟认证器 | 1 个测试通过，覆盖 29 个场景 |
| subscription、external-access、member-commerce 浏览器回归 | 桌面与两档窄屏全部通过 |
| compiler、panel-host、plugin-singbox、panel 全目标 Clippy，-D warnings | 通过 |

没有测试失败或被省略的失败补验。测试后仅补记验收文档与进度，代码和内嵌产物保持验证时内容。

## 清理与限制

- 结果摘要与精确源码身份已保存。已删除本轮 PostgreSQL 容器及临时数据库、编译目录、Bun 缓存、前端依赖和原始日志，只保留源码、内嵌前端与有界摘要。该记录证明当时结果，不承诺原始测试环境持续存在。
- 专用克隆原有暂存工作保留；其他轮次与生产数据未清理。
- 未运行完整 Rust 工作区及无关历史失败用例；未执行真实认证器、真实代理线路、Agent 安装、签名发布或云资源操作。
- GitHub Actions 保持暂停，未启用、触发或重跑；不据此宣称 main 全绿。
- 此记录只确认本地验证。远端合并与分支删除以结束前 GitHub 复核为准；面板部署由独立 apsp-deploy.timer 接续，本轮未重复启动或更改部署器。
