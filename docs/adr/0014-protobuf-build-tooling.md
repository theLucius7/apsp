# ADR 0014：在构建时生成统计协议消息

状态：已接受。

## 背景

统计客户端必须使用上游 1.14.2 的原始 proto，并且不能依赖宿主机安装 protoc。任务已经允许 tonic、prost 和 protoc-bin-vendored，仍需要连接这些组件的构建工具。

## 决策

适配器增加构建依赖 tonic-build，配合 protoc-bin-vendored 从原始 proto 生成消息。保留上游文件原样，不手写或复制生成的 Rust 消息。

上游运行时把服务名改成 `v2ray.core.app.stats.command.StatsService`，与 proto 的包名不同；客户端明确使用上游实际注册的路径。原始 proto 的 package 不做修改。

## 影响

新增依赖仅在构建时运行。适配器不访问面板、不持久化状态，也不修改上游源码。替代方案是手写消息或修改 proto，但都会增加与上游协议漂移的风险。
