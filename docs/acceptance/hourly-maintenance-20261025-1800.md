# 2026-10-25 每小时维护集中集成

## 输入与审查

- 开始时 main 为 `5db460f8f5c7bee171cb86c86e71d6645a5a54c0`，PR [#186](https://github.com/theLucius7/apsp/pull/186) 的 head 为 `75665a0b56e9dec1244620392a390e4819a49d4b`，目标 main、非草稿、无阻止合并的审查或未解决讨论。
- 验证期间 #186 由其他会话合并为 main `fa20ebf5d105b62e6f5db7f24156fdc28b55baa6`；重新 fetch 后确认其树与已审查的 #186 head 完全一致，复用本地验证输入。本任务没有重复执行该 PR 的合并。
- 扫描分支 `claude/sinan-scan-refactor-odrgr3` 的最新 head 为 `03a2213dff0536a84b6802d6c3a282626c9b3986`。旧 PR #158 合并后另有三个提交：`302a92f7`、`54accd57`、`03a2213d`；逐项审阅后集中整合。检查时没有该分支的打开 PR，不以旧 PR 的合并记录删除新提交。
- `claude/agent-0.3.1-published`、`claude/agent-0.3.1-release` 和 `claude/nexus-device-limits` 均已重新核对最新 SHA 是远端 main 的祖先，并核对非保护、非默认、未作为打开 PR 的 base 后按 SHA lease 删除。
- 原专用克隆的未提交合并保持原样，本轮使用该克隆新增的独立工作树。

## 集成处理

具体选择见 [开放问题](../open-questions.md) 的“每小时维护集中集成设备限制与缺陷修复”。保留 main 的稳定布局、离开保护、语言保存、可见对象批量操作和服务器续费投影，补入扫描分支独有的权限、会员认证、任务恢复及界面修复。

- 已发布迁移 0072–0074 内容不变；新迁移顺延为 0075/0076。
- Nexus 接管流量已进入兼容桥投递队列，补计原生用量时排除这些记录；待投递、已确认及普通原生节点混用均有回归覆盖，避免重复计费并保留倍率。
- 全局费用汇总、日历和付款历史统一检查全服务器授权。纯服务器费用 GET/HEAD 由处理函数按对象范围过滤，测试确认只返回已授权服务器。
- 费用汇总保留服务器费用与订阅到期引用，采用整数单位换算并标记过期汇率；前端制品由最终源码重建。

## 本地验证

先完成整批集成，再集中验证；全量失败修复后只补验受影响范围。

| 检查 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| 全工作区全目标严格 Clippy | 通过；最终修复后对面板和费用插件全目标补验也通过 |
| `cargo test --offline --locked --workspace --no-fail-fast`，隔离 PostgreSQL 16 | 1372 通过、1 失败、27 条件忽略 |
| 唯一失败修复后的定向补验 | 面板库、`control_center`、`expense_subscriptions` 共 11 通过、0 失败 |
| 核心边界检查及 Python 检查器回归 | 检查通过，13 项回归通过 |
| Python Nexus 回归 | 27 通过 |
| Python bootstrap 回归 | 24 通过、3 跳过 |
| Bun 冻结锁文件安装与 TypeScript/Vite 构建 | 通过，保留最新 `web/dist` |
| 前端单测 | 206 通过、0 失败，2580 断言 |
| 浏览器回归 | shell-continuity、interaction-consistency、shell-language、server-list-selection、member-commerce、nexus-nodes、node-catalog 七组通过 |
| 文档链接、差异及冲突标记检查 | 通过 |

唯一失败为 `scoped_operator_cannot_change_shared_settings_and_denied_reauth_is_audited`：外层守卫使纯服务器费用返回 403。仅为该精确只读路径增加对象级授权声明后，新增的服务器过滤断言及原有拒绝断言均通过。未把首次全量执行写成全部通过，也未重复运行不受影响的全量测试。

Rust 使用 PATH 中的工具链。首次构建缺少 pkg-config，通过显式指定本机已有的 OpenSSL 头文件和库目录解决，没有安装系统依赖。数据库与凭据均为隔离测试夹具。

最终受验源码 SHA-256：`1f7c511e25cb615dad00c8ad66b30589c0a09152c5a6bd80db4116bef2bd65bb`。计算范围为 Git 跟踪文件，排除 `docs/`、`PROGRESS.md`、`AGENTS.md`，按 Git 路径顺序累计“路径、NUL、文件内容、NUL”；结合以上两个源 head 和本提交可定位验证内容。

## 未验证范围与清理

- Rust 的 27 个忽略项未执行，包括固定上游运行时、真实系统服务/特权、ICMP、Pebble、支付夹具和虚拟 WebAuthn 的显式验收。不能据此签收真实设备、完整验机或跨平台运行能力。
- bootstrap 的两项 root 专属用例与一项缺少 minisign 的独立验签用例跳过。
- 本任务未触发、重跑或恢复 GitHub Actions，不把跳过或取消当作通过；没有发布签名 Release、安装 Agent、操作真实云资源/DNS/通知。main 更新后的面板部署由独立部署器接续，本任务不启动部署。
- 验证摘要及源码标识已保存。确认归属后删除本轮 PostgreSQL 容器、target、Cargo/Bun 缓存、前端临时依赖及完整原始日志，仅保留有界结果摘要、当前源码和最新已跟踪前端制品。其他运行材料与原克隆未提交工作未清理；本记录不承诺已删除的原始材料继续存在。
