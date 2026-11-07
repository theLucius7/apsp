# 自维护 sing-box 运行时

当前版本 `1.14.4-apsp.1` 基于上游正式版 `v1.14.4`（`c005f35300ce76427dc285481ec5695f5b179cd2`），加上 `user-limits.patch`。见 [ADR 0124](../../docs/adr/0124-stable-runtime-and-user-limits.md)。旧服务内核的历史选择保留在 ADR 0119。

补丁在认证后的 TCP/UDP 路由入口实施逐用户、每服务器合计的上下行限速和连接数限制。配置位于 `route.apsp_user_limits`，每组显式列出认证主体；0 表示不限，32 KiB 突发，TCP 连接和 UDP 会话合计。面板与编译器负责生成，不需要修改 Agent core。

`apply.py` 只接受固定上游的干净 checkout，以固定身份/日期生成可复现提交；构建和校验脚本固定此提交。`modules.txt` 固定运行时实际链接模块的版本和校验和。`wire-probe.py` 保留为 REALITY 目标站对照工具，不代表本次已执行外部探测。

构建保持 `CGO_ENABLED=0`，标签 `with_quic,with_utls,with_wireguard,with_clash_api,with_v2ray_api,with_acme`。支持原生 Snell v6、自动证书配置、Hysteria2 BBR 参数和 TLS 握手超时；Naive 入站可用，Naive 出站仍需未打包的 Cronet 原生库，不能宣称已支持。glibc 与 musl 使用相同静态二进制。

更新补丁后按顺序：在干净固定 checkout 应用、更新提交固定值、运行补丁单元测试、构建并更新模块清单、原生配置检查和回环协议验证。之后另行签名发布；本地构建不会上传 GitHub。数据库约束只保留历史部署事实并允许新版本，生产基线按显式接入脚本处理，不增加自动升级路径。
