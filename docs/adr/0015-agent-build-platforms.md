# ADR 0015：扩展 Agent 编译产物的平台

状态：已采纳；按最新后续决策，九个目标统一由日常 CI 构建，Linux 按 libc 与架构区分。

## 背景

用户要求在已有 Linux musl 制品之外，增加 Ubuntu 24.04 glibc、macOS arm64、FreeBSD 13 系列及以上、Windows 的 Agent 编译产物，除 macOS 外均提供 amd64 和 arm64。用户已明确本次范围为“增加可下载的 Agent 编译产物”，因此这是对原 Linux 构建范围的明确扩展，服务部署仍限 Linux；后续 OpenRC 服务支持见 [ADR 0017](0021-openrc-services.md)。

## 决策

- 保留 Linux musl 双架构的原生构建和部署目录。glibc 使用 Ubuntu 24.04 双架构 runner 的动态库，显式关闭静态 CRT，并验证 ELF 架构、动态解释器、`libc.so.6` 依赖和库解析。
- macOS 使用 GitHub 最新 macOS arm64 runner；Windows 使用 Visual Studio 2026 的 x64/arm64 runner、MSVC 目标与静态 CRT；所有目标使用最新 Rust stable。
- FreeBSD 使用 `vmactions/freebsd-vm` 的 13.5 基线及对应架构构建，在同一架构的最新 14、15 系列 VM 内执行同一二进制的 `--version` 和 `--help`。13.5 之前的小版本及更高的未来主版本不宣称已验证。
- 新制品按完整 Rust target 分目录，包含原生二进制和 `SHA256SUMS`，避免 GNU/musl 或不同系统同名文件互相覆盖。Actions 上传名称包含平台、链接方式及架构，保留七天。
- GitHub Actions 工具使用本次核实的最新稳定版：[checkout v7.0.1](https://github.com/actions/checkout/releases/tag/v7.0.1)、[upload-artifact v7.0.1](https://github.com/actions/upload-artifact/releases/tag/v7.0.1)、[setup-python v7.0.0](https://github.com/actions/setup-python/releases/tag/v7.0.0)、rust-cache v2.9.2、setup-bun v2.2.0 和 freebsd-vm v1.5.8。Windows arm64 的 VS2026 runner 标签已有官方文档，但晚于 actionlint 的内置列表，在校验配置中单独列入允许列表。
- Agent 的命令解析与 `--help`、`--version` 可以在所有编译目标启动。`enroll`、`run`、`status` 在非 Linux 平台明确返回部署限制；不增加 launchd、FreeBSD rc.d 或 Windows 服务管理。
- Unix 系统操作与 Unix socket 传输仅在 Unix 目标编译。路径分量检查属于制品通用逻辑，移入 `artifacts`，Windows 无须引入伪造的特权或 socket 实现。
- 构建脚本优先尊重显式 `PROTOC`，FreeBSD 使用构建 VM 安装的 protobuf 编译器。其他目标仍使用已有 vendored 编译器，不新增 Rust 依赖。跟踪 `PROTOC` 变化，确保 Cargo 正确重建。

## 替代方案

直接对所有平台执行现有 Unix 代码会导致 Windows 编译失败。为各平台实现完整安装、权限、身份、IPC 和服务管理超出用户确认的范围。统一从 Linux 交叉编译需要额外 SDK、链接器和模拟器；使用原生 runner 或 VM 可以同时验证架构及实际启动。

## 验证边界

本地检查覆盖 Rust workspace、构建脚本的架构拒绝与制品不可覆盖行为。各系统、架构及 FreeBSD 跨版本执行以对应提交的 Actions 实际结果为准；添加矩阵不能视作远端已经构建成功，也不能视作非 Linux 的设备部署支持。


## 后续决策：2026-09-30 收敛自动构建

用户要求先让 main 的持续集成恢复全绿，再推进真实 Linux/systemd 部署验收。自动构建只保留 Linux musl amd64 和 arm64；glibc 动态版、macOS、Windows 和 FreeBSD 不再阻塞 push 与 pull request 的 CI。保留本 ADR 的历史平台扩展记录和通用构建脚本，不增加非 Linux 部署能力。

Linux 的自动检查和 amd64 构建固定使用 `ubuntu-24.04`，arm64 构建固定使用 `ubuntu-24.04-arm`，避免 `ubuntu-latest` 的基线迁移改变已验收环境。Rust stable 和既有工具版本策略保持原决策。

musl 双架构的 CI 先使用同一源码、目标、工具链和构建脚本，通过 Cargo 环境覆盖恢复旧 release profile（`strip=none`、`lto=false`、`codegen-units=16`），将构建目录隔离到 runner 临时目录；再按仓库的优化 profile 构建正式制品。每次记录两份二进制的精确字节数、缩减百分比到 job summary 与制品中的 `size-comparison.txt`，避免不同提交的二进制差异影响体积对照。

继承工作区中曾加入 `tools/test-freebsd-rust.py` 的主 CI 步骤，但当前仓库并无该测试文件，因此主 CI 不执行此不存在的步骤。原始 workflow 改动已经在仓库外保留，官方工具链安装器保持原样未提交。此次不新建依赖该未提交文件的手动 workflow；后续恢复可选平台时应将安装器与完整测试一并评审。

最新 main 提交 `fc8467d` 的 [CI 36681900051](https://github.com/theLucius7/apsp/actions/runs/36681900051) 中，检查、Compose smoke 和 musl 双架构均已通过；唯一失败是 FreeBSD arm64 在 rustup 安装阶段收到 404，尚未进入 Cargo。此次调整直接收敛到既定 Linux 部署范围。新的完整 CI 结果仍须以本次提交的实际 Actions 运行为准。

## 后续决策：参考多平台交叉构建

按用户要求对照本地 NodeFlare 的 Actions，保留日常 CI 的 Linux 范围，新增 `Agent platform artifacts` 工作流，手动触发或在 `v*` 标签触发。glibc 继续使用 Ubuntu 24.04 系统动态库，macOS ARM64 和 Windows 双架构继续使用原生 runner，不照搬参考项目的 glibc 2.28 基线或较窄的 Windows 架构范围。

FreeBSD 两个目标改为在 Linux 上安装 Rust stable 和目标标准库，使用参考项目相同的固定 cross 提交与 FreeBSD 13 sysroot 镜像摘要。构建后验证 ELF 架构，再在 FreeBSD 13.5、14、15 VM 执行同一个二进制；13.5/14 验证不依赖 EOL 包仓库，15 安装 Python 后运行共享校验和打包脚本。只有三个版本的验证均成功才上传制品。该流程避开 ARM64 FreeBSD 缺失的 rustup 安装器，也不放宽服务部署仍限 Linux 的边界。

交叉编译只增加 CI 工具，不增加 Rust 运行依赖。构建脚本新增 `--binary` 用于在目标系统验证已有二进制，校验架构、版本、CLI 与非 Linux 部署限制后才创建带 `SHA256SUMS` 的不可覆盖制品目录。实际双架构和跨版本结果须以新工作流执行为准。

## 后续决策：统一日常多平台 CI

用户进一步明确多平台应随日常 CI 构建，OpenRC 不单独拆出。删除临时的手动/标签工作流，将其目标移回 `ci.yml`，在 push、PR 和手动触发时一并构建九个目标。

Linux 构建合并为 libc × 架构矩阵：musl 静态 amd64/arm64、Ubuntu 24.04 glibc 动态 amd64/arm64。OpenRC 安装、缓存刷新、HUP、恢复与权限检查在 musl 任务内执行，不创建 OpenRC 二进制或单独任务。macOS 只构建 ARM64，Windows 与 FreeBSD 保留双架构；FreeBSD 继续使用固定 cross 工具链与 13.5/14/15 VM 验证。

Alpine/OpenRC 的 Agent 使用 musl 静态版，Ubuntu 24.04/systemd 可使用 glibc 动态版。服务后端仍在运行时自动识别，二进制的 libc 兼容性独立于 init；不把所有 OpenRC 系统都视为 musl，也不改变安装接口或 Linux 服务管理边界。

## 后续执行：第 2 阶段复用体积证据

第 0 阶段已在同一源码、架构和工具链下完成原 release profile 与优化 profile 的体积对照，实际字节数保存在对应 CI 摘要与制品中。第 2 阶段开始，常规 push/PR 的 musl job 只构建优化制品，不再重复编译旧 profile 或生成体积对照；真实端到端 job 复用本次运行的优化 Agent 制品。
