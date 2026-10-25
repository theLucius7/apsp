# ADR 0100：OpenRC 的会话日志仅保存在内存

## 问题

Nexus 剩余一台可连接主机使用 OpenRC。ADR 0099 的设备数限制依赖 journald 命名空间，因此已发布 Agent 0.3.2 不能在这台主机上执行同样的限制。不能通过放宽接管检查、伪造 systemctl 或把逐连接日志写入持久目录来完成迁移。

## 决定

OpenRC 使用自身的 `output_logger`、`error_logger` 启动两个 `apsp-agent runtime-log` 进程，分别接收标准输出和错误流。它们属于运行时的专用账户，并继承运行时的限制；每个采集进程使用单线程异步运行时，独立于 Agent 的连接与监督进程。重启 Agent 不终止原运行时或日志采集。服务启动前分别验证 Agent 和外部运行时的签名。

日志正文只进入进程内的有界缓冲，不创建日志文件。每个流最多保存 8 MiB 编码数据及条目开销，最长一小时；单行超过 16 KiB 时丢弃到下一换行，不积累未结束的长行。每个流最多允许两个读取者。读取者持有的快照、广播队列和写入时间也有限制，慢连接被断开，不阻塞运行时输出。

服务同时把 core dump 的软硬上限设为零，避免崩溃时把内存日志写成转储文件；真实 OpenRC 测试从采集进程的 `/proc/<pid>/limits` 核对继承结果。

本机读取接口使用 `/run/apsp/runtime-logs/<通用服务名>/<流>.sock`。父目录由 root 保护，最内层目录属于运行时账户且权限为 0700，socket 为 0600。读取者核对目录、socket 及对端 UID；采集进程仅接受 root 或自身账户。正文不经过系统日志、磁盘临时文件或网络接口。Agent 重连时重放最近十分钟，短暂断开时重放最近两分钟，与现有 journald 路径一致；旧会话继续遵循 ADR 0099 的不追溯断开规则。

只有运行中的 OpenRC 系统及受 root 保护、包含完整内存日志合同的服务脚本才声明会话准入能力。普通日志文件和旧安装脚本不会声明。管理员运行时日志同样来自内存接口，并过滤逐连接级别；原有结果只保留通用事件分类。core 仅处理通用服务、流和日志，不加入代理协议或成员业务。

标准安装器根据已独立验签 Agent 的 `runtime-log --help` 判断是否支持新合同。旧签名 Agent 使用保留的兼容服务模板，继续不声明会话准入能力；不能把新服务模板下发给不具备该命令的旧二进制。systemd 原有 journald 路径保持。

## 验证与发布

隔离验证需覆盖容量和过期淘汰、无换行超长输入、读取者断开与重连、无正文文件、错误请求、服务合同与权限拒绝，以及真实 OpenRC 的服务独立性。生产接管仍须使用正式签名版本，并确认原客户端、会话归属、新旧设备计数和正流量投递。

本实现不恢复暂停的 CI，也不把本地源码或测试制品安装到生产。Agent 0.3.2 的临时发布例外已经结束；新版本的正式发布需单独按仓库流程推进。

参考：[OpenRC 服务脚本](https://github.com/OpenRC/openrc/blob/master/sh/supervise-daemon.sh)、[日志进程与账户切换的实现](https://github.com/OpenRC/openrc/blob/master/src/supervise-daemon/supervise-daemon.c)、[supervise-daemon 手册](https://github.com/OpenRC/openrc/blob/master/man/supervise-daemon.8)。
