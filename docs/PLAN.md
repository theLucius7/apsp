# Apsp MVP 执行计划

以任务说明为验收基准，顺序推进 G1–G9。每阶段完成 fmt、clippy（-D warnings）、test，更新 PROGRESS 并提交 Conventional Commit。

## G1：基础结构
- Rust 2024 workspace、七个 crate、unsafe 禁令与内部依赖边界。
- AGENTS、术语表、11 条架构 ADR、CI、忽略规则。
- 工具链与数据库环境；cargo build 验证。

## G2：协议
- 信封、认证、注册、遥测、清单、配置包、应用结果和流量消息。
- 全部消息往返、未知字段与消息兼容测试；协议文档。

## G3：编译器
- 节点与授权模型；确定性服务端配置及用户订阅。
- 排序、无授权排除、密钥格式及黄金测试。

## G4：面板基础
- 数据库迁移、首次管理员、Cookie 会话。
- 服务器 CRUD、安装命令、原子一次性注册、签名挑战认证。
- 清单、配置包、制品下载与脚本渲染；数据库集成测试。

## G5：Agent 核心
- 配置、身份、注册、传输与重连、遥测、status。
- SQLite 迁移、事务化计量、确认重发、重启恢复。
- 通用适配器契约；超时、串行对账、版本合并、回滚与意图恢复测试。

## G6：运行时适配器
- 制品、版本和构建标签校验；check、apply、health。
- 上游统计 proto 和 reset=false gRPC 客户端。
- 二进制入口注册适配器；真实运行时可用部分的验证。

## G7：面板业务
- 节点、用户、授权 CRUD；端口和凭证生成。
- 五秒合并、哈希去重发布、状态和历史、流量去重汇总、订阅。
- 测试数据库与 FakeAdapter 的进程内全链路测试。

## G8：中文界面
- React + Vite + TypeScript、普通 CSS。
- 登录、服务器及详情、节点、用户、授权、订阅、安装与流量。
- 前端构建及面板内嵌静态页面验证。

## G9：部署与交付
- 幂等安装、systemd/OpenRC、Dockerfile、Compose。
- 原样上游构建、Agent musl 构建、Debian 手动验收脚本。
- README、最终验证与环境边界记录；检查后 push 指定仓库。

## 用户后续需求：NodeQuality 外插
- Agent 上报 IPv4/IPv6，支持 NAT 公网地址配置；兼容旧协议与旧 Agent。
- 面板聚合并缓存上游 IPQuality 数据库信息，显示来源、更新时间和部分失败。
- 固定上游 NodeQuality 制品、一键分发节点诊断、独立 systemd 服务、超时和清理。
- 任务与结果持久化、身份隔离、串行执行、重连/重启去重与重传。
- 中文服务器详情页展示 IP 质量、任务进度、文本与报告链接。
- 验证协议、HTTP/PostgreSQL、恢复和服务契约、上游包装脚本、Bun 构建；执行 Rust 全检查并提交、push。

## 交付加固：第 0–5 阶段

G1–G9 的实现完成后，按用户要求继续顺序推进。每阶段仍需 Rust 全检查、进度记录和 Conventional Commit；CI、真机和发布各自保留实际证据。

### 第 0 阶段：main 全绿
- 自动 CI 收敛为 musl amd64/arm64，Linux runner 固定 Ubuntu 24.04。
- release 开启 strip、LTO、单 codegen unit；同源码、同工具链对比原 profile 的 Agent 体积。
- NodeQuality 上传缺省关闭，创建任务时显式选择；旧制品不能绕过选择。
- 推送 main 并核对本阶段提交所有必需检查成功。

### 第 1 阶段：真机验收
- 检查指定 SSH 主机和既有服务，隔离准备面板、数据库及 Linux/systemd Agent，保留已有数据与未提交工作。
- 经指定 HTTPS 反代完成 Reality 客户端、健康部署、重启及重载后的稳定用量验收。
- 用真实 0.1.0 升级到 0.2.0，核对身份与运行时连续性；流程阻塞和 workaround 记录为 issue。

### 第 2 阶段：CI 固化
- PR 必跑的 Ubuntu 24.04 e2e：Compose、musl Agent、版本缓存的运行时、真实安装和 systemd、Reality 流量及面板账本断言。
- README 拆出部署与开发文档；仅删除已有真实 CI 证据覆盖的待验收说明；补充仓库 About 与 topics。

### 第 3 阶段：发布与信任链
- 先写签名 ADR，再实现 Ed25519 离线签名、可信公钥与应用前验签；私钥保存在仓库外，不进入 CI。
- tag 构建 Agent/运行时双架构、SHA256SUMS 和 Release；提供离线签名后完成发布的流程。
- 面板导入 Release 制品；Agent/面板版本独立，面板声明协议兼容范围。

### 第 4 阶段：安全补漏
- 在线 Agent 先退役再删除，离线保持既有行为；订阅重置、登录限速及 TOTP。
- 节点端口可手动指定（含 443），20000–29999 仅作自动默认。

### 第 5 阶段：仅设计文档
- 链式中转 ADR 定义内部凭证生成与轮换、仅入口计量、出口变更驱动入口发布。
- 评估编译器与计量表影响；本阶段不实现链式中转代码。
