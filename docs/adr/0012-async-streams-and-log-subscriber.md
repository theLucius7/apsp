# ADR 0012：异步流工具和日志订阅器

状态：已接受。

## 背景

规定栈中的 axum WebSocket 与 tokio-tungstenite 暴露 Stream/Sink，需要组合发送、接收和分流。tracing 仅产生事件，本身不把日志写到终端。

## 决策

增加 futures-util 0.3 用于标准异步流扩展，增加 tracing-subscriber 0.3 用于进程日志初始化和级别过滤。两者均不引入新的产品能力，仅补全规定技术栈的基础设施。

## 替代与影响

手工轮询 Stream/Sink 或自行实现日志订阅器会增加无关代码和出错面。依赖仅用于传输和诊断；不得记录令牌、密码、私钥或包含敏感参数的完整 URL。
