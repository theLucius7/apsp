# 自维护 sing-box 运行时

当前 `1.14.4-apsp.2` 基于正式版 `v1.14.4`，上游提交 `c005f35300ce76427dc285481ec5695f5b179cd2`，补丁提交 `5d74251bd02a4b243a2cf23a6f0e7bdfc2259642`。见 ADR 0124、0125。

`user-limits.patch` 在认证后的 TCP/UDP 路由入口实施每用户每服务器合计限速与连接数限制；`sessions.patch` 在受密钥保护的回环控制接口提供认证主体。`apply.py` 只接受固定上游的干净 checkout，以固定身份/日期生成可复现提交。上游 checkout 不作为补丁编辑源。

`modules.txt` 来自固定上游的 go.sum，限制各平台链接的模块版本与校验和。基础标签包括 QUIC、uTLS、gRPC、WireGuard、Clash/V2Ray API、ACME、gVisor、Cloudflared、OpenVPN/OpenConnect 和 Tailscale。

- Linux glibc：`with_naive_outbound,with_purego`，Go 主程序通过 purego 加载动态库，随包固定 Cronet `libcronet.so`，要求 glibc ≥ 2.31。
- Windows：相同纯 Go 模式，随包固定 `libcronet.dll`。
- Linux musl：`with_naive_outbound,with_musl`，CGO 静态链接；缺少 `CC`/`CXX` 时，构建脚本通过 `prepare-musl.py` 自动准备固定 Cronet 提交的 Chromium/musl 工具链；只采用 `env` 输出的四个白名单环境变量，不执行其文本。Docker 构建环境包含所需 Python requests、解压与签名校验工具。不能把 glibc 动态库给 musl 使用。
- macOS：本机 Xcode/CGO 链接上游固定静态 Cronet 模块。
- FreeBSD：不包含 Naive 出站，服务器配置入口明确拒绝此组合。

`cronet.py` 固定库长度与 SHA-256，构建包和发布元数据均包含附件。Agent 对包与每个文件独立核对签名声明，库没有单独的未验签下载通道。版本化附件契约允许对已安装签名旧制品执行回滚。

`test-user-limits.py` 验证本机真实 TCP/UDP 限制；`test-sessions.py` 验证同一 NAT 地址下认证主体隔离及定向断开。配置检查不等于公网 ACME、Cloudflared、Realm 或 VPN 服务已完成连接。

本地构建不会上传 GitHub。正式签名发布和生产接入另行执行，CI 维持仓库当前暂停状态。
