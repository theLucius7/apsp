# 每小时维护集中验收（2026-10-30 14 时）

## 源码与取舍

- main 基线：`318fc3ab2ba158d1cc4570d3aecec012f52fa0dd`；#194 原 head：`b699921fd36c595869415098e4c878eafd9a1061`，已包含该 main；#190 的 `939ca1b13c6e1daa134924134ea9e9c814f28f38` 仍为草稿，不纳入。
- 复用 #194，仅修编译器对标准化 HTTP 空路径和等价 HTTP/2 方法、空请求头的处理，补充真实标准化输入回归；选择与上游依据见 `docs/open-questions.md`。
- 独立重算原 head 代码与内嵌产物 SHA-256 为 `19e26d14e72f258858b69eeb264536a4e3921287e470e790f1aa93e7aa97f664`，与[19 时验收](hourly-maintenance-20261028-1900.md)一致；前端、宿主、阿里云插件和依赖相对该验收无变化。原记录仍证明当时结果，本轮修复只补验受影响的编译器范围。

## 验证

最终验证代码与内嵌产物 SHA-256：`18d8d8c9247e09d35c4843808b40f83579ae702860df9ae3ef7dd86f29a58ba0`。按 Git 跟踪路径排序，以路径、NUL、文件内容依次计算，排除 `docs/`、`PROGRESS.md`；结果补记不改变代码。

完成整批实现、七项回归与文档后，使用 PATH 中的 Rust/Cargo 1.99.1 集中执行：

| 验证 | 结果 |
| --- | --- |
| `cargo fmt --all -- --check` | 通过 |
| `python3 tools/check-core-boundary.py`、`git diff --check` | 通过 |
| `cargo test --locked -p apsp-compiler --all-targets` | 78 通过、0 失败、7 条件忽略 |
| `cargo clippy --locked -p apsp-compiler --all-targets -- -D warnings` | 通过 |
| mihomo v1.19.29 `-t` | 既有 10 节点样例及新增 HTTP/HTTP2 五节点样例均通过；仅离线解析，不连接代理节点 |

- 新增回归覆盖九协议标准化默认字段、HTTP 认证和单值头保留、非空代理路径拒绝、VMess/VLESS HTTP/2 空或 PUT 方法、其他方法及自定义头拒绝、既有 TLS/Trojan 边界。独立源码复核通过。
- mihomo 使用官方发布的兼容 amd64 制品，压缩包 SHA-256 为 `5612e698e96c8b8ad15abc4c0a4f098eba9234354b4f248cb97f2528e215b094`，与 GitHub asset digest 一致。
- 前端、宿主、业务插件、数据库集成与 Passkey 代码未改，保留之前对应源码的验收证据，不重复记为本轮执行。七项编译器条件测试仍需要指定上游运行时，不用离线配置校验替代。

## 清理与边界

仅使用本任务运行目录中的隔离工作树，保留历史专用克隆的暂存修改。Cargo 缓存与编译目录均位于本轮目录。核对精确路径所有权、无活动测试进程及挂载后，已删除本轮 `target`、`cargo-home`、mihomo 工具与状态目录、两个测试样例和原始日志，保留当前源码与有界结果摘要。未创建容器或临时数据库，不清理其他轮次及生产材料；本记录证明当时结果，不承诺原始测试环境持续存在。

GitHub Actions 保持暂停。完整工作区、七项上游运行时条件测试、真实代理连接和真实认证器未在本轮执行；不进行 Agent 安装、签名发布、真实诊断或云资源操作。面板部署由独立 `apsp-deploy.timer` 接续，不重复启动或修改其凭据和数据卷。
