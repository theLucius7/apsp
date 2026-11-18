# 设备平台与能力

每台设备只需安装一体化自带能力的 `apsp-agent`（单文件 `raw` 可执行文件，`<= 128 MiB`），运行时作为独立系统服务 `apsp-singbox@main` 运行（[ADR 0141](adr/0141-self-contained-agent-and-desired-service-config.md)）。签名发布、bootstrap 信任根与首次接入步骤见 [部署文档](deploy.md) 和 [发布文档](release.md)；平台扩展不能跳过这些校验。

## 平台与服务

| 平台 | Agent | 服务管理 | 内置运行时 |
| --- | --- | --- | --- |
| Linux glibc amd64/arm64 | musl 静态或 GNU 动态 | systemd / OpenRC | 对应架构 glibc（含 `libcronet.so`）或静态 musl |
| Linux musl amd64/arm64 | musl 静态 | OpenRC | 对应架构静态 musl |
| macOS arm64 | 原生 | launchd | 对应架构原生 |
| FreeBSD amd64/arm64 | FreeBSD 13 sysroot 构建 | rc.d / daemon | 对应架构原生 |
| Windows amd64/arm64 | MSVC 静态 CRT | 计划任务 | 对应架构原生及已签 `libcronet.dll` |

init、Agent 编译 ABI 与宿主运行时 ABI 分别判断。静态 musl Agent 可以运行在 glibc 主机上，静态遥测的 `libc` 表示 Agent 自身编译 ABI，Linux 字段 `runtime_libc`（仅读取宿主 `/bin/sh` ELF 解释器信息识别，不能可靠识别时回退为编译 ABI；拒绝显式 `unknown`、空值或畸形值）表示宿主可执行的运行时 ABI。按 [ADR 0141](adr/0141-self-contained-agent-and-desired-service-config.md)，一体化 `apsp-agent` 在构建期直接内嵌与自身目标 ABI 匹配的 `sing-box`（`1.14.4-apsp.2`）与原生 `apsp-agent tcp-probe` 子命令，面板不再单独挑选或下发运行时制品；Agent 接入与自动升级按 `os`、`arch`、`libc` 匹配签名 Agent 目标（Linux musl 按 `linux-musl-{arch}`、兼容旧 `{arch}` 匹配，Linux GNU 按 `linux-gnu-{arch}` 匹配，不把 GNU 目标发给纯 musl 宿主）。

Linux 正式发布构建（`APSP_RELEASE_EMBED_REQUIRED=1`）强制内嵌完整四组件闭环：`sing-box`（`1.14.4-apsp.2`）、`-r22` `nodequality`（`a92fca6c0067df29ddd03fdc2fee6f3000f64545-r22`）、`ipquality`（`87397e2c3196ec796f5477c83343c2354df601ea-node-r1` 离线 rootfs 捆绑包）与 `network-workbench`（`1.0.0` 自包含离线工具包，含内置 Python 与全部必需受控工具），外加原生 `tcp-probe`；任一必需输入缺失直接中止正式 Linux 构建。在线设备通过 `static_info.components` 与 `component_digests` 上报内置组件版本与 SHA-256，未内置的平台如实不声明对应能力。

日常 CI 的 Agent 使用公开 TEST_ONLY 信任根，制品名称带 TEST_ONLY，禁止用于正式节点或发布。四个平台的单行入口现已实现：Shell 用于 Linux/macOS/FreeBSD，PowerShell 用于 Windows，自动检测 CPU/ABI 并选择兼容签名版本；见 [部署文档](deploy.md#复制安装命令)。当前已公开的正式标签（`agent-v0.3.0` 至 `agent-v0.3.9`）均为 ADR 0141 之前的 Linux 发布；包含上述四组件内嵌闭环的新版一体化 Agent 正式签名发布，以及 macOS、FreeBSD、Windows 的正式签名制品与实机服务安装，均待按发布流程另行构建、签署与验收。

原生生产安装先独立验证签名、metadata 和待执行 Agent 的实际内容，将 proof 三文件保存在版本目录后再注册服务；`install-service` 在任何账户或服务修改前以编译根复验自身，并验证安装后的副本。缺少匹配 proof 的生产接入保持拒绝。

Unix 默认配置 `/etc/apsp/agent.toml`、Agent `/opt/apsp/core`、状态 `/var/lib/apsp/core`；macOS/FreeBSD 状态套接字 `/var/run/apsp/agent.sock`。Windows 默认根目录 `%ProgramData%\Apsp`，本机状态使用受保护命名管道。`agent_root` 与内置组件落盘目录 `install_root`（Unix 默认 `/opt/apsp/plugins`，保留既有磁盘路径契约并写入 `.bundled.json` 摘要标记）分开配置；未配置有效服务的设备默认不启动 `sing-box`，`run --monitor-only` 可显式锁定纯监控模式。

## OpenRC

OpenRC 安装要求 `supervise-daemon` 支持 capabilities/no_new_privs，并安装 CA、curl、基础账号/文件工具与 `unshare`（Alpine Linux 接入入口安装 `shadow` 账号工具时要求启用 `/etc/apk/repositories` 中的 `community` 软件源，仅 `main` 源会明确报错提示）。安装后将两个服务加入 default runlevel，刷新依赖树。Agent 与代理分别监督，日志在 `/var/log/apsp/`；更新只重启 Agent，reload 将 HUP 发送到实际代理子进程。

**Agent 0.3.5 及更早版本在 OpenRC 上的升级风险（2026-11-05）：** 这些版本用 `rc-service` 启停运行时时，运行时的 `supervise-daemon` 留在 Agent 工作进程的进程组里。Agent 升级或重启会向整个进程组发 SIGTERM，于是运行时被一起停掉，服务显示 `stopped`；新 Agent 只报「上一次应用的运行时不健康」，不会自己拉起。修复版本把这些命令放进独立进程组，但升级到修复版本的那一次仍由旧代码执行。所以在 OpenRC 主机上升级 Agent 前，先比较两个进程组：`cut -d')' -f2 /proc/$(cat /run/apsp-singbox@main.pid)/stat | awk '{print $3}'` 与 `apsp-agent ... run` 进程的 PID。两者相等时，先在管理员 shell 里执行 `rc-service apsp-singbox@main restart` 让运行时脱离该进程组，再升级；升级后用 `rc-service apsp-singbox@main status` 确认仍为 started。

NodeQuality 使用独立一次性服务、独立挂载命名空间、超时与持久终态；Agent 重启不重启已开始的诊断。自 2026-10-03 起诊断启动要求 systemd/cgroup v2 的 CPU 硬上限与 swap 系统调用保护，OpenRC 无法证明这些保护，真实 Agent 一律拒绝且不创建服务、不运行负载；OpenRC CI 校验这一拒绝边界。公网 NodeQuality 性能结果仍须独立验收。详见 [ADR 0021](adr/0021-openrc-services.md)、[ADR 0022](adr/0022-agent-capability-alignment.md)。

## 监控、任务与更新

系统指标默认一秒采样、三秒上传，可调整；离线样本持久保留最多一天且不超过 100000 条。SWAP、进程、磁盘 I/O、GPU 和公网地址按设备能力提供，缺失数据保持缺失。公网地址识别可本地关闭。

持续 TCP/ICMP 拨测最多 32 项，间隔 10–3600 秒，离线继续使用的配置最多一天；结果最多保留两小时、4096 条。

远程命令由节点本地顶层配置 `allow_remote_commands` 控制，默认 `false`；只有节点操作者修改为 `true` 并重启 Agent 才能开启，面板设置不能启用。关闭时不领取命令，也不声明 `command:execute` 能力；前端缺少这项能力时禁用提交。开启即授权绑定面板以 Agent 服务账号执行任意 shell，Unix 通常为 root，Windows 为 SYSTEM；制品签名不能约束这些命令。命令最多运行 600 秒，标准输出和错误输出分别限制 256 KiB；执行前持久记录，重启后不重放未确认终态的命令。

自动更新默认关闭，仅使用绑定面板内协议兼容且通过发布签名的稳定 Agent 发布。监督进程与运行时分开，更新保留身份和账本；升级试用（trial）期覆盖内置组件提取校验与活跃运行时的切换及健康恢复，启动检查失败或中断未确认升级后 Agent 与运行时引用一并回退到上一版，不重复尝试已失败版本。Windows 目录元数据断电刷盘仍属验证边界。

FreeBSD 的代理使用普通账户，默认应选择 1024 以上的节点端口。低端口权限由管理员配置系统策略，安装器不修改全局端口授权。

## 验证边界

[CI](../.github/workflows/ci.yml) 包含 Linux musl amd64/arm64、代码与 Compose 检查、OpenRC 监督与诊断、systemd Reality 安装和计量定义，以及 GNU、macOS、Windows 和 FreeBSD 的全平台验证定义。Ubuntu runner 固定为 `ubuntu-24.04` / `ubuntu-24.04-arm`。所有 workflow 当前按用户要求暂停，没有由本任务触发、重跑或恢复。

GNU 动态 Agent、完整 Linux 运行时矩阵、macOS ARM64、Windows 与 FreeBSD 双架构也保留仅手动触发的 [Platform validation](../.github/workflows/platforms.yml)，用于单独验收。FreeBSD 使用同一二进制在 13.5、14、15 验证；原生任务检查注册、遥测补报、命令去重、拨测、升级回退、服务与回环代理流量。自动 CI 通过不代表手动平台任务已通过；成功结果须对应当前提交的 Actions。

GPU 实际负载、公网 Reality 客户端、真实 NodeQuality 性能和整机断电/重启由专用设备验收，见 [验收文档](e2e.md)。
