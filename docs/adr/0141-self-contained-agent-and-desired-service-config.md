# ADR 0141：一体化自带能力 Agent 与服务配置驱动生命周期

状态：已接受（2026-11-18）。延续并更新 ADR 0017、0038、0079、0128、0131、0140。

用户明确要求：“不要有制品这些概念了，这些都是 Agent 自带的；功能都集中由 APSP Agent 管理，不需要再点单独安装 sing-box 等，Agent 和面板连接后接配置即可；让功能做回，精准实现需求，代码简洁清晰。”

## 决策

1. **单一 Agent 安装与升级单元**：`apsp-agent` 维持单文件已签名 `raw` 可执行文件形态（体积受控在既有自更新器 `128 MiB` 上限内），构建时直接内嵌对应平台的真实压缩运行时归档（`sing-box` `1.14.4-apsp.2` 及其必需同签运行库/许可说明）与真实受控诊断能力（内置完整 `crates/tcp-probe` 多模式日志引擎的 `apsp-agent tcp-probe` 子命令；Linux 正式发布构建在 `APSP_RELEASE_EMBED_REQUIRED=1` 下强制要求 `sing-box`、`-r22` `nodequality`、`ipquality` 与 `network-workbench` 四项真实组件归档完整内嵌闭环，缺任一输入即编译失败；开发构建或原本不支持对应 Linux 专用工具的非 Linux 平台如实上报自身已内嵌组件能力，绝不嵌入空 stub 或伪报就绪）。Agent 二进制自身的发布签名、编译时信任根与 GitHub Release 下载/自更新边界（ADR 0017、0038）保持不变；Supervisor 升级试用（trial）覆盖内嵌提取校验与活跃运行时的切换和健康恢复，试用失败时 Agent 与运行时引用一并回退。
2. **移除独立制品库与运行时手工安装层**：面板删除独立制品目录（`/api/artifacts`、`/settings/artifacts`）、设备制品下载接口（`/api/agent/v1/artifacts/...`）、独立运行时库存与制品灰度（`/api/traffic/runtime-inventory`、`/api/traffic/runtime-rollouts`）、手工启用表与接口（`runtime_enablements`、`POST /api/traffic/servers/{id}/enable`）以及“首次安装空运行时”和阻塞普通节点发布的人工预检确认流（`POST .../preflight/bootstrap`、`POST .../preflight/confirm`）。系统级 Agent 发布版本管理收敛为 `/api/system/agent-releases*`，组件版本由在线 Agent 通过 `StaticInfo` 直接上报。
3. **当前有效服务配置是唯一期望运行状态**：
   - 运行需求（`configured = true`）仅来自当前未删除且启用的节点、受管链路跳点或已显式激活（`active`）的切流服务意图；历史 `deployments` 记录或默认/孤立的 `ServerProfile` 调优项不单独制造运行需求。
   - 未配置有效服务的服务器处于 `unconfigured`（纯监控模式），面板下发空模块期望（`modules: []`），Agent 不启动或自动停止已有的受管 `sing-box`，并在 `state.db` 清除活跃模块标记，确保宿主机或 Agent 重启后绝不因历史安装目录反开。服务器删除、退役、`--monitor-only` 与 Nexus `prepared`/`paused` 挂起门禁优先。
   - 管理员保存启用节点或链路后，面板自动编译并下发目标配置（允许离线或旧 Agent 先保存排队）；在线一体化 Agent 收到清单后幂等补齐受管系统服务定义、提取校验内置运行时、写入配置、拉起或重载服务并回报健康状态。
4. **同配置不健康自动恢复与安全回滚**：新配置版本应用失败时，Agent 自动回滚至上一已验证版本并重验健康；当当前期望模块仍要求运行、目标修订号未变但健康检查未通过或因宿主机重启脱离受控实例时，Agent 在满足恢复代数门禁的前提下自动执行受控重启恢复并重新认证实例，不再要求管理员人工介入点击重部署。既有诊断工具的全部模式、未知与零值区分、部分报告、资源预算（CPU、内存、任务数、超时）、确认式取消与 NodeQuality 完整验机安全门禁完整保留。
5. **面板端 DNS-01 证书签发内置受控官方 `lego`**：节点本地 HTTP-01 / TLS-ALPN-01 证书继续由设备侧内置 `sing-box` 自行签发续期；按 ADR 0079 由维护方明确选择面板执行的 Cloudflare DNS-01 签发保留在面板端，Cloudflare API 凭据与证书私钥继续加密保存在面板凭据中心，绝不下发给 Agent，也不恢复独立制品库或插件路由。标准面板镜像从钉死多架构摘要的官方镜像（`goacme/lego:v5.6.0@sha256:6f1583fc026a455ea46a7a5858c576105bb69c2e2de69db391cab7f14750350c`）将官方 `lego`（`5.6.0`）置于只读绝对路径 `/usr/local/libexec/apsp/lego`（`0:0`、`0555`）并在构建期与运行期双重校验目标架构二进制头、固定 SHA-256 及 `--version` 真实版本身份；任务执行直接在独立 `0700` 临时工作区调用该只读可执行文件并完成临时材料清理，不再按任务复制二进制。既有权限与凭据引用复验、MIT 许可与服务条款确认、默认预发布环境、证书版本历史轮换、部署手工签收及未知结果人工核对保持不变；历史计划若固定了不匹配版本号，明确拒绝并返回更新为当前内置版本的修复提示，不静默冒充旧版本。

发布校验复用锁文件已有的 `ring` 与 `memchr` 计算大型原始 Agent 的 SHA-256 和查找内置清单帧。直接用调试配置的通用散列实现处理约 98 MB 的测试包需要 6.9 秒，现有签名、摘要和长度校验规则不变。
