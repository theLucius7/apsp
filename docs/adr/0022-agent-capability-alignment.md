# ADR 0022：补齐 Agent 监控、任务、升级和多系统部署

状态：已接受。

## 背景与范围

用户在核对本地 NodeFlare 后明确要求「全部差距一起补齐」，包含此前仅提供编译产物的 macOS、FreeBSD、Windows 常驻部署，以及 Agent 自动更新、通用远程命令。此要求覆盖原 MVP 对非 Linux 运行、自更新和高频监控的排除项；代理协议、单实例、三层依赖、特权 trait、独立运行时、不可变制品和中文界面约束保留。

## 决策

- sing-box 继续固定上游 1.14.2、原样源码、平台对应的官方默认标签和 `with_v2ray_api`。Linux 增加官方 `with_musl` 与 `build-naive --libc=musl`，同时保留 glibc 构建；按平台、libc 和架构选择制品，不将 init 等同于 libc。
- Agent 编译 ABI 与宿主运行时 ABI 分开：`StaticInfo.libc` 保留 Agent 编译目标，新 Linux 字段 `runtime_libc` 由仅读宿主 `/bin/sh` ELF 解释器信息识别，不执行宿主文件或根据发行版猜测。GNU 宿主上的 musl Agent 为非 Agent 制品保留 compiled musl → legacy 架构键 → host GNU 的候选顺序，避免旧缓存 proof 同时含 legacy/GNU 时改变选中摘要而拒绝升级；仅有 GNU 制品时也可运行，Agent 自动升级仍只按编译 ABI。识别不明时兼容沿用 Agent 编译 ABI，但不将回退视为宿主识别证据；面板仍对显式未知、null 或畸形值拒绝，不将这些值当作缺字段。旧设备缺少新字段时沿用原 `libc`。面板只在候选不存在时尝试后项，真正 musl 宿主不回退 GNU sing-box；core 保留其他旧插件的通用签名兼容，不引入运行时名称特例。非 Linux 不发送此字段，协议主版本不变。
- 系统遥测采用一秒采样、默认三秒批量上传；补充 SWAP、进程数、磁盘 I/O、GPU、公网地址。缺失指标保持缺失。持久 outbox 限制大小与保留期，面板按样本身份去重；迟到样本不能倒退当前指标。
- 持续 TCP/ICMP 拨测通过认证面板配置和下发。通用命令另受节点本地顶层配置 `allow_remote_commands` 控制，默认 `false`，面板设置不能开启；只有操作者在本机改为 `true` 并重启 Agent 才声明 `command:execute` 能力并领取命令。面板和前端根据该能力限制提交，旧响应缺少能力字段时默认禁用。任务具有期限、超时、输出上限与持久终态，重启后不重复执行状态不明的命令，结果确认后才清理。所有外部命令继续经过 `Privileged`。
- 自动更新从固定 GitHub Release 下载经过编译时可信发布根校验的对应平台制品，绑定面板只提供已验证的版本与证明，可配置 HTTPS 镜像（下载来源已按 ADR 0038 覆盖原决策），配置可关闭；保留身份和账本，服务监督独立于代理运行时。更新前保存恢复信息，启动验证失败自动恢复旧 Agent，避免反复更新同一失败版本。
- Linux 使用 systemd/OpenRC；macOS 使用 launchd；FreeBSD 使用 rc.d；Windows 使用计划任务和受保护状态目录。各系统均保持 Agent 与代理服务独立，状态查询仅开放本机且有访问保护。NodeQuality 维持 Linux 适配器，增加 OpenRC 任务监督；不将该 Linux 外插宣称为跨系统插件。
- 优先使用已有依赖、标准库和系统工具，新增依赖须另行说明。协议主版本保持 1，通过新增消息和能力声明兼容旧 Agent。
- 遥测等辅助表使用幂等的增量建表，不提高旧账本的 SQLite `user_version`，使旧 Agent 在安装或升级回退后仍可打开身份、对账和流量账本。压缩上传复用已有 `flate2`，仅将面板的测试依赖移至运行依赖，不新增库。

## 原生服务与更新边界

- Windows 复用系统 PowerShell、计划任务和 NTFS ACL；目录切换使用受保护的原子引用文件，避免多步删除/重建 junction 的空窗。运行时归普通专用账户，Agent 归 SYSTEM；启动触发器无需交互登录。文件 fsync 后替换，但不承诺标准库尚未提供的 Windows 目录元数据断电刷盘语义。
- 监督进程仍属于 Agent 服务，没有新增特权 RPC/helper。SQLite 独占锁防止重复监督；仅停止/替换 Agent 子进程，代理服务始终独立。健康检查使用本地版本和 PID，允许面板离线时启动旧配置。
- 更新限制为面板导入的稳定版本；默认关闭，六小时加抖动轮询，失败五分钟重试。阶段状态先落盘，保留上一版本与最多 32 个失败版本，未确认启动或监督进程中断均恢复旧版本。旧 Agent 账本兼容依赖增量辅助表设计。
- Windows 运行时含原生 DLL，使用适配器描述的文件白名单及逐文件摘要，不在 core 引入任何运行时名称或特例。
- Windows 账户参数遵循 [New-LocalUser](https://learn.microsoft.com/en-us/powershell/module/microsoft.powershell.localaccounts/new-localuser?view=powershell-5.1) 的独立参数集，后台任务采用 [密码登录任务](https://learn.microsoft.com/en-us/windows/win32/taskschd/security-contexts-for-running-tasks)，凭据由系统任务计划程序保存。

- macOS 的受管 Agent 子进程留在 launchd 服务进程组，遵循 [Apple launchd 的进程组清理规则](https://github.com/apple-oss-distributions/launchd/blob/main/man/launchd.plist.5)，更新时只向该 Agent PID 发送信号。正常服务退出先等待 Agent 收尾；运行时日志在启动前按专用账户权限创建。
- FreeBSD 的 `daemon` 使用 `-f` 关闭继承的标准描述符，再用 `-S` 保存子进程日志；否则后台监督进程会保留服务调用方的输出管道，使安装或重启命令一直等待 EOF。选项语义依据 [FreeBSD daemon 源码](https://github.com/freebsd/freebsd-src/blob/releng/15.0/usr.sbin/daemon/daemon.c)。

发布签名按 [ADR 0017](0017-signed-release-artifacts.md) 保留：Agent 原始二进制、运行时及辅助 DLL 均须有可独立验证的签名证明；服务启动、更新试运行和回退前重新核验实际文件。平台扩展不增加运行时换根开关。

开启远程命令等于授权绑定面板以 Agent 服务账号执行任意 shell（Unix 通常为 root，Windows 为 SYSTEM）。发布签名验证制品来源和内容，不能约束命令的内容或效果；`Privileged` 是内部操作边界，也不能将任意 shell 限制为受签名操作。保留这一功能的代价是操作者显式扩大对面板的信任，因此开关必须只由节点本机控制，不能作为远程 Agent 设置下发。

## 实现阶段与验证

1. Linux musl/glibc 运行时制品与自动选择、安装失败恢复。
2. 扩展遥测、可配置批量上传与持久补报。
3. 拨测、命令、OpenRC 诊断和面板管理入口。
4. 多系统注册、常驻运行、独立服务、安装和状态查询。
5. Agent 自动升级、故障回退和完整 CI 验证。

每阶段执行格式、Clippy、完整 Rust 测试及与变更对应的专项检查。自动 CI 仅构建 Linux musl amd64/arm64 Agent，并保留代码、Compose、OpenRC 与 systemd Reality 计量检查；Ubuntu runner 固定为 24.04。GNU、完整运行时矩阵和其他原生平台的服务验证保留在仅 `workflow_dispatch` 触发的 [platforms.yml](../../.github/workflows/platforms.yml)，不删除多平台实现。真实代理、公网 Reality、硬件 GPU 和整机重启结果单独记录，不将进程夹具等同于实机验收。

参考：[sing-box 官方源码构建](https://sing-box.sagernet.org/installation/build-from-source/)、[cronet-go 工具链](https://github.com/SagerNet/cronet-go)。
