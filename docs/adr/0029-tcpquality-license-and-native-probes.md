# ADR 0029：TcpQuality 许可核对与原生 Rust 路径

状态：采用。对应 Issue #58。

## 已核对的证据

检查 [ibsgss/TcpQuality 固定提交 c2295ae096437859ce4bbc36f170428fcac47be9](https://github.com/ibsgss/TcpQuality/tree/c2295ae096437859ce4bbc36f170428fcac47be9)。完整仓库树共 16 条记录，涵盖入口、核心、rootfs 包装器与构建工作流、C/eBPF 辅助文件；没有 LICENSE/COPYING，README 没有明确分发许可。GitHub 的许可接口未发现许可文件。

已向作者提交[授权询问 #27](https://github.com/ibsgss/TcpQuality/issues/27)。2026-10-01 核对时没有回复；这个状态需要在未来分发前重新核对。公开仓库本身不等于代码使用和分发许可，参见 [GitHub 许可说明](https://docs.github.com/en/repositories/managing-your-repositorys-settings-and-features/customizing-your-repository/licensing-a-repository)。

## 决定

当前不复制、打包或执行上游入口、核心脚本、C/eBPF、rootfs 或内置目标，也不运行在线 `main`。按照用户明确给出的无授权备选方案，在 Apsp 中独立编写 Rust TCP 连接探测，继承项目 AGPL-3.0-only 许可。产品和报告称“原生 TCP 连接诊断”，记录实际引擎名称与版本，不冒充上游 TcpQuality 的同等实现。

第一版测量 TCP 建连耗时和连接成功率，不宣称测得原始分组丢失、大包性能、重传或单线程吞吐。使用有限 DNS 解析和单个同地址族 SocketAddr，不写入应用数据，不安装外部程序，不使用 raw socket，不修改宿主网络、内核或挂载。允许的参数和时间、次数、并发上限在独立参数 PR 中验证。

禁止报告或排行上传及测速。调用契约明确要求 `--no-rank-upload`，拒绝 `--allow-speedtest-staged`、`--no-rootfs` 和未知选项；原生实现没有第三方 rootfs 下载流程。实际无上传由私有 HTTP/TCP 观察夹具验证，不能仅检查命令行字符串。

原生制品按 Apsp 的固定源码提交和 Cargo.lock 构建，作为独立工具进入既有校验、离线签名、不可变版本及 Agent 验签流程，附自身源代码和许可。签名及构建流程单独 PR，不用固定入口的哈希代替核心及依赖链的验证。若未来作者授权，以新 PR 重新核对覆盖文件和第三方许可，不自动改回上游在线脚本。

## 顺序与验收

本 PR 只完成许可审计和路径决策。原生工具、制品流程、参数白名单、无上传与宿主修改、同机互斥及报告元数据分别独立提交与验收。NodeQuality 迁入共用框架并通过历史、重启、取消和部分报告回归后，才登记第二个插件。

当前没有把 TcpQuality 登记进面板或 Agent，也没有声称原生工具、网络探测或签名制品已完成。专用节点完整验机的验收仍须节点恢复并满足预算后进行。
