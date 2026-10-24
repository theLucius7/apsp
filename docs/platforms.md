# 设备平台与能力

设备继续使用独立 Agent 和代理运行时服务。签名发布、bootstrap 信任根与首次接入步骤见 [部署文档](deploy.md) 和 [发布文档](release.md)；平台扩展不能跳过这些校验。

## 平台与服务

| 平台 | Agent | 服务管理 | 运行时 |
| --- | --- | --- | --- |
| Linux glibc amd64/arm64 | musl 静态或 GNU 动态 | systemd / OpenRC | 对应架构 glibc |
| Linux musl amd64/arm64 | musl 静态 | OpenRC | 对应架构 musl |
| macOS arm64 | 原生 | launchd | 对应架构原生 |
| FreeBSD amd64/arm64 | FreeBSD 13 sysroot 构建 | rc.d / daemon | 对应架构原生 |
| Windows amd64/arm64 | MSVC 静态 CRT | 计划任务 | 对应架构原生及已签 DLL |

init、Agent 编译 ABI 与宿主运行时 ABI 分别判断。静态 musl Agent 可以运行在 glibc 主机上，因此静态遥测的 `libc` 只表示 Agent 自身编译 ABI，Linux 新字段 `runtime_libc` 表示宿主可执行的运行时 ABI。面板用两者决定运行时候选，Agent 自动升级继续只使用 `libc`。例如 `{"os":"linux","arch":"amd64","libc":"musl","runtime_libc":"gnu"}` 保留已有 musl 运行时的选择优先级，也允许在缺少 musl 制品时获取 GNU 运行时；Agent 更新仍选 musl。

新 Agent 仅读取宿主 `/bin/sh` ELF 的解释器信息识别运行时 ABI，不能可靠识别时兼容沿用 Agent 编译 ABI；这一回退不代表已经识别宿主。面板仍拒绝显式 `unknown`、空值或畸形的 `runtime_libc`，不会把这些值当作字段缺失；旧设备未提供该字段时沿用原 `libc` 选择行为。

GNU Agent 已通过兼容层运行在 musl 宿主时，仍优先复用其旧 GNU 完整标识及架构兼容键，再尝试新的 musl 运行时；面板和离线缓存验签使用同一顺序，避免已签 GNU 缓存突然换选摘要。此处保留既有缓存兼容，不作为 gcompat 实机验收结论。musl Agent 在 musl 宿主仍只获取显式 musl 运行时。

GNU 宿主上的 musl Agent 按 `linux-musl-{arch}`、旧 `{arch}`、`linux-gnu-{arch}` 的顺序选择运行时，保留旧版本对已签缓存的选择，避免新增宿主识别后切换到同一证明中的其他摘要；GNU 宿主上的 GNU Agent 按 GNU 完整标识再旧目录选择。只有候选不存在才继续查找，签名或内容损坏直接失败。musl Agent 在 musl 宿主上的运行时只选择 `linux-musl-{arch}`，面板不能回退到 GNU。旧签名发布中的架构目录对 Agent 表示静态 musl、对 sing-box 表示 GNU，不能跨组件混用这一兼容规则；core 对其他旧插件保留通用签名兼容，不能据此让面板向这种设备提供 GNU sing-box。

日常 CI 的 Agent 使用公开 TEST_ONLY 信任根，制品名称带 TEST_ONLY，禁止用于正式节点或发布；正式发布工作流当前生成 Linux 双架构六个组件制品；原生平台的生产签名 bundle、独立来源核验与发布验证须另行完成。四个平台的单行入口现已实现：Shell 用于 Linux/macOS/FreeBSD，PowerShell 用于 Windows，自动检测 CPU/ABI 并选择兼容签名版本；见 [部署文档](deploy.md#复制安装命令)。当前正式 0.3.0 仅有旧 Linux 目标，原生正式制品与实机服务安装仍待独立发布验收。

原生生产安装先独立验证签名、metadata 和待执行 Agent 的实际内容，将 proof 三文件保存在版本目录后再注册服务；`install-service` 在任何账户或服务修改前以编译根复验自身，并验证安装后的副本。缺少匹配 proof 的生产接入保持拒绝。

Unix 默认配置 `/etc/apsp/agent.toml`、Agent `/opt/apsp/core`、状态 `/var/lib/apsp/core`；macOS/FreeBSD 状态套接字 `/var/run/apsp/agent.sock`。Windows 默认根目录 `%ProgramData%\Apsp`，本机状态使用受保护命名管道。`agent_root` 与代理 `install_root` 分开配置，`run --monitor-only` 用于监控设备。

## OpenRC

OpenRC 安装要求 `supervise-daemon` 支持 capabilities/no_new_privs，并安装 CA、curl、基础账号/文件工具与 `unshare`。安装后将两个服务加入 default runlevel，刷新依赖树。Agent 与代理分别监督，日志在 `/var/log/apsp/`；更新只重启 Agent，reload 将 HUP 发送到实际代理子进程。

NodeQuality 使用独立一次性服务、独立挂载命名空间、超时与持久终态；Agent 重启不重启已开始的诊断。自 2026-10-03 起诊断启动要求 systemd/cgroup v2 的 CPU 硬上限与 swap 系统调用保护，OpenRC 无法证明这些保护，真实 Agent 一律拒绝且不创建服务、不运行负载；OpenRC CI 校验这一拒绝边界。公网 NodeQuality 性能结果仍须独立验收。详见 [ADR 0021](adr/0021-openrc-services.md)、[ADR 0022](adr/0022-agent-capability-alignment.md)。

## 监控、任务与更新

系统指标默认一秒采样、三秒上传，可调整；离线样本持久保留最多一天且不超过 100000 条。SWAP、进程、磁盘 I/O、GPU 和公网地址按设备能力提供，缺失数据保持缺失。公网地址识别可本地关闭。

持续 TCP/ICMP 拨测最多 32 项，间隔 10–3600 秒，离线继续使用的配置最多一天；结果最多保留两小时、4096 条。

远程命令由节点本地顶层配置 `allow_remote_commands` 控制，默认 `false`；只有节点操作者修改为 `true` 并重启 Agent 才能开启，面板设置不能启用。关闭时不领取命令，也不声明 `command:execute` 能力；前端缺少这项能力时禁用提交。开启即授权绑定面板以 Agent 服务账号执行任意 shell，Unix 通常为 root，Windows 为 SYSTEM；制品签名不能约束这些命令。命令最多运行 600 秒，标准输出和错误输出分别限制 256 KiB；执行前持久记录，重启后不重放未确认终态的命令。

自动更新默认关闭，仅使用绑定面板内协议兼容且通过发布签名的稳定制品。监督进程与运行时分开，更新保留身份和账本；启动检查失败或中断未确认升级后回退到上一版，不重复尝试已失败版本。Windows 目录元数据断电刷盘仍属验证边界。

FreeBSD 的代理使用普通账户，默认应选择 1024 以上的节点端口。低端口权限由管理员配置系统策略，安装器不修改全局端口授权。

## 验证边界

[CI](../.github/workflows/ci.yml) 包含 Linux musl amd64/arm64、代码与 Compose 检查、OpenRC 监督与诊断、systemd Reality 安装和计量定义，以及 GNU、macOS、Windows 和 FreeBSD 的全平台验证定义。Ubuntu runner 固定为 `ubuntu-24.04` / `ubuntu-24.04-arm`。所有 workflow 当前按用户要求暂停，没有由本任务触发、重跑或恢复。

GNU 动态 Agent、完整 Linux 运行时矩阵、macOS ARM64、Windows 与 FreeBSD 双架构也保留仅手动触发的 [Platform validation](../.github/workflows/platforms.yml)，用于单独验收。FreeBSD 使用同一二进制在 13.5、14、15 验证；原生任务检查注册、遥测补报、命令去重、拨测、升级回退、服务与回环代理流量。自动 CI 通过不代表手动平台任务已通过；成功结果须对应当前提交的 Actions。

GPU 实际负载、公网 Reality 客户端、真实 NodeQuality 性能和整机断电/重启由专用设备验收，见 [验收文档](e2e.md)。
