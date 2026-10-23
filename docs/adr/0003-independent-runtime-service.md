# ADR 0003：运行时使用独立 systemd 服务

- 状态：已确定，MVP 不得更改。
- 对应任务说明：第 4 节第 3 条。
- 后续扩展：用户追加的 OpenRC 支持见 [ADR 0021](0021-openrc-services.md)，运行时独立生命周期的约束保持不变。

## 背景

Agent 升级、重启或暂时故障不应中断已经提供的代理服务。把运行时作为 Agent 子进程会把两者生命周期耦合。

## 决策

sing-box 由独立的 `apsp-singbox@main.service` 管理，不能作为 Agent 的长期子进程运行。Agent 通过服务管理接口请求重载、重启和检查状态。MVP 只管理 `main` 一个实例。

Agent 自身由 `apsp-agent.service` 管理。运行时使用专用系统用户 `apsp-singbox`，服务模板采用最小所需能力与 `NoNewPrivileges=true`。配置和二进制使用版本目录及 `current` 符号链接选择。

## 影响

Agent 停止时已有代理服务继续工作。运行时的重载与重启属于显式对账操作，需要独立处理流量终值、意图与健康检查。短时调用版本查询、配置检查命令不改变运行时服务的独立生命周期。

## 验证

检查 systemd 单元与安装流程；在真实 Debian 12 环境重启 Agent，确认运行时服务保持运行；对账测试通过假服务管理器验证重载与重启请求。
