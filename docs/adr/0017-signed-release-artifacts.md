# ADR 0017：minisign 发布制品、构建时信任根与 Release 导入

> 2026-10-01 更新：[ADR 0038](0038-server-operations-and-public-dashboard.md) 按用户要求将 Agent 安装/自更新改为 GitHub Release 下载，支持独立 HTTPS 镜像且不发送设备凭据。下文面板同源下载仅继续约束运行时和配置；签名及编译时信任根要求不变。

- 状态：已采纳，2026-09-30；先完成本文，再实施。正式信任根与正式签署由用户提供；实现先使用明确的测试根。
- 用户决定：minisign、编译时多个公钥、`minisign-verify` 验签，CI 不取得发布私钥。

## 背景与范围

现有制品只有 SHA256，面板可以同时替换下载物和预期摘要。缓存 `.artifact.json` 记录的 binary digest 也没有签名；只给压缩包摘要加签，仍可能被“二进制和 marker 一起改”的缓存替换绕过。

本 ADR 认证发布者批准的 Agent、外部运行时、诊断制品和固定安装器。动态用户配置继续通过已认证面板、HTTPS、revision 和 bundle hash 对账，不需要本地私钥逐次签署。签名不证明面板配置无恶意，也不抵抗已掌握主机 root 的攻击者。NodeQuality 包和包装器被签署，上游诊断中的外部下载仍按 ADR 0016 工作，不宣称这些运行时下载也自动属于签名信任链。

Agent 继续只从已配置面板同源下载；面板负责从固定 GitHub 仓库导入 Release。仍保留完整快照、独立 systemd 服务、Privileged/ServiceManager 边界，不引入自动升级守护进程、独立 helper 或 TUF。

## 签名及新增依赖的必要性

使用 minisign 的现代默认签名格式，对 `SHA256SUMS` 的原始字节签署，文件名为 `SHA256SUMS.minisig`。不加私有域前缀、不手工预哈希、不把完整 minisign 签名缩成裸 64 字节。Rust 验证者使用 `PublicKey::verify(checksums.as_bytes(), &signature, false)`，验证正文及 trusted comment 的签名，不允许 legacy 降级。minisign 底层使用 Ed25519，算法背景可参阅 [RFC 8032](https://www.rfc-editor.org/info/rfc8032/)，完整格式以 [minisign 官方文档](https://jedisct1.github.io/minisign/)为准。

新增 `minisign-verify = "=0.2.5"`，放在 workspace dependency，由 protocol 复用。该版本提供公钥/签名解析与验证且无外部依赖；实施时检查下载源码和 lockfile，保留 `forbid(unsafe_code)`。选择固定版本以便审查，不因网页 `latest` 变化自动改版本。[该版本 API](https://docs.rs/minisign-verify/0.2.5/minisign_verify/struct.PublicKey.html)、[源码与依赖声明](https://github.com/jedisct1/rust-minisign-verify/tree/0.2.5)。

替代方案：已有 ed25519-dalek 仅提供原始 Ed25519 验签，不完整处理 minisign 的 prehash、key ID、双层签名及注释；自己补格式会增加密码代码与测试负担。完整 `minisign` Rust 库也支持签名/私钥管理，而 Agent 和面板只需验签。外部调用 minisign CLI 会使每次 Agent 应用依赖另一个可执行制品及其可信来源。故选择仅验证的 Rust crate；设备身份继续使用现有 ed25519-dalek，不混用设备密钥和发布密钥。

共享 wrapper 在库解析前限制 `.minisig` 恰好四行、固定注释前缀、长度和尾部，拒绝额外隐藏行；库解析本身不替代完整格式边界检查。key ID 用于匹配，不是认证凭据；trusted comment 可用于展示，但路径、版本、架构和仓库身份只从已签 SUMS 所覆盖的 metadata 中取。

测试另直接引用 lockfile 已有的 `blake2 0.10`，仅作为 dev-dependency，配合已有 ed25519-dalek 生成标准 prehashed minisign 测试签名；正式路径只调用 minisign-verify。相比要求每次 cargo test 安装 CLI 或为每个动态载荷提交静态签名，这能覆盖事务与恢复组合而不引入生产签名器；另用真实 minisign CLI 静态夹具交叉验证测试编码。

面板将既有测试依赖 `tar 0.4` 和 `flate2 1` 提升到生产依赖，用于在导入时核对归档内的单个普通文件、大小和已签二进制摘要。替代方案是只核对压缩包 SHA256，但这会把归档内容与 metadata 的不一致推迟到节点安装才发现；调用外部解包工具则增加运行环境依赖。复用 core 已有的库版本，不引入另一个归档格式。

## 构建时固定多个公钥

Agent/面板共享 `option_env!("APSP_RELEASE_PUBLIC_KEYS")`，值为 JSON 字符串数组，每项包含公钥 base64 记录或完整两行 `.pub` 文本，最多 8 个。正式公钥是公开信息，可以进入仓库、编译日志和 Actions 配置；私钥与口令不可进入 Git、聊天、CI、面板或制品包。根公钥缺失、为空、非法或重复时 fail closed，不提供运行时开关关闭验签。

Agent 的所有生产执行路径只使用编译好的根集合。面板响应、ReleaseProof、安装参数和 `/etc/apsp` 文件都不能向该集合新增公钥；也不能把面板返回的 key ID 当作受信根。面板和安装器不得以方便升级为由更改正在安装的 Agent 所信任的公钥。

用户自行构建时可明确把自己的公钥集合传给构建过程；这种 Agent 属于自建信任域，不冒称官方制品。普通发布不需要信任测试钥。测试可显式使用 fixture 公钥或构建测试根，不能通过生产 runtime 参数注入测试根；正式发布 workflow 检查批准的公开根集合，拒绝缺根或包含已知测试根的 Agent。

## 发布身份及 canonical 内容

保留逻辑路径 `name/version/arch`，arch 只接受 `amd64` 和 `arm64`。Agent 是 raw musl 二进制，运行时/诊断是只包含一个预期普通可执行文件的 tar.gz。路径组件使用明确安全语法，禁止绝对路径、空组件、`.`、`..`、反斜线、百分号转义和查询串。

Release 包含 `release.json`、固定 `install.sh`、两架构 Agent、两架构运行时；仍支持诊断时一并包含两架构 NodeQuality r2。`SHA256SUMS` 覆盖 `release.json`、`install.sh` 和全部 canonical artifact paths：小写 64 位摘要、两个空格、LF、路径 ASCII 升序、每项一次。上限 32 项/8 KiB；不接受注释、CR/BOM、缺项、未知项或重复项。

`release.json` schema=1，字段固定为 source_repo、tag、protocol_min/max、artifacts。每个条目绑定 name/version/arch/format/binary_name/archive_size/binary_sha256/binary_size/asset_name。metadata 原始字节自身由 SUMS 的 `release.json` 摘要绑定，随后 SUMS 由 minisign 签署。安装后 binary digest 必须来自这个已签字段，不能来自未签本地 marker。

GitHub 资产采用固定扁平公式：raw 为 `name-version-linux-musl-arch`；tar.gz 为 `name-version-linux-arch.tar.gz`。导入器验证 metadata 中的 asset_name 与公式一致，不接受任意远程 URL。metadata/SUMS 项一一对应。源码 tag 指向和构建 commit 由发布流程及维护者核验记录检查，当前最小 schema 不另加 source commit 字段；签名直接认证已批准的成品字节。

不能覆写相同 name/version/arch 的不同内容。运行时同上游版本重新打包若产生不同字节，必须使用制品修订号，区分包装版本与上游程序版本；诊断 r2 固定提交与包装修订也继续分开。

## Agent 执行与缓存

共享纯验签放在 `protocol::release`；IO、缓存、安装、对账和诊断应用仍在 core。wire 字段及纯验签接口由 `protocol::release` 定义，协议文档同步记录。

`Artifact` 新增 `proof: Option<ReleaseProof>`，缺省 None 保持旧消息可解析；proof 包含 metadata_json、checksums 和完整 `.minisig` signature 文本。新 Agent 应用任何制品必须有合法 proof。安装前检查期望 name/version/本机 arch、binary_name、format、原始摘要及大小；证明中的另一制品不能替代当前身份。

归档先下载并检验，再经现有安全解包逻辑安装到受控 staging；检验实际 binary digest/size，写完整 proof，完成同步后才公布版本目录。使用已有 `Privileged::execute` 的 `/bin/mv --no-clobber --no-target-directory`，不扩展 SDK。同名目录竞争时重新验证最终内容，不把命令成功退出当成“自己的 staging 已发布”。禁止覆盖已存在不同内容或因验签失败破坏当前运行版本。

缓存命中同样重新验签 proof 和真实 binary。`.artifact.json` 只能作为签名证据容器，里面的未签摘要不能成为预期值。缓存目录、proof、二进制必须是受控普通文件，限制大小并拒绝软链逃逸。`current` 的受控链接仅用于定位，解析后校验普通版本目录和已签身份。

验证覆盖下载前、适配器 prepare 执行 version/check 前、switch/apply 前、同 revision/同版本快捷返回、失败 rollback、启动 pending intent recovery、诊断 prepare/start，以及 systemd 的外部重启入口。proof 持久保存使面板离线仍可验证；不能把“曾经安装过”当作可信。

Agent CLI 提供无网络 `verify-installed --binary <path> --name <expected-name> --format <expected-format>`，只读 compiled roots 和本地 proof，不读取 root 私有身份配置。名称和格式必须由执行入口指定，不能从待检查的缓存推导；Agent unit 固定要求 `agent/raw`，运行时 unit 固定要求 `sing-box/tar.gz`，避免另一个合法签名模块被放到错误执行位置。运行时 unit 的 ExecStartPre 使用同一 Agent 命令，阻止手动/开机重启执行被替换的缓存。诊断启动也在 core 验证；恢复 Started 只观察，不再次执行任务。

旧未签缓存或回滚目标不会自动被认可。升级安装器在替换 Agent 前预检当前及 pending intent 所引用的制品，只有实际 binary 与正式签名完全相符才能补证明。否则保留旧进程、身份和账本，报告需要迁移或维护窗口。core 若遇到未签恢复/回滚，持久记录失败和非健康状态、保留可调查 intent，并输出明确错误；已有连接的处理路径可上报失败，但启动恢复发生在 transport 建连之前，失败时只能在本地记录并中止启动，不能声称面板已经收到 `ApplyResult`。不得为兼容继续执行 unsigned previous。只读预检和人工补证明不提供对旧 Agent 的跨版本事务锁，迁移时需避免并发管理操作。

## bootstrap 信任起点

2026-10-01 官方在线安装与本地架构下载的调整见 [ADR 0037](0037-bootstrap-and-selective-import.md)：普通部署改为复制固定官方 GitHub 自包含入口命令，免手动预置 bootstrap；完整 proof 和 Agent 编译根验签不变。下述手工预置步骤继续用于独立审查、自建根与离线安装。

面板的 `curl <panel>/install.sh | sh` 可被篡改，不能作为信任起点。公钥跟脚本从同一面板下载，也不能修复这一问题。

首次安装必须先从面板以外的可信来源核对公钥和获得可信 minisign 验证器，验证官方 Release 的 SUMS 和固定 install.sh 摘要之后再执行安装器。根不由面板自动提供；安装器获得的注册 token、panel origin、Agent 版本都是参数数据，不是生成的新 shell 程序。

可信 bootstrap 先用独立公钥和 minisign，或已有受信 Agent 的编译根，验证清单及固定安装器。安装器据此校验下载物的已签身份、大小和 SHA256，先完成独立字节验证，再执行新 Agent 的 `verify-installed` 检查其编译根，最后才允许 enroll。它不能下载一个未知 Agent 后直接执行其 verify 命令来证明 Agent 自己可信。下载流最多接受已签大小，拒绝声明或实际长度不符，设置超时并在失败时移除部分文件。校验使用 `python3 -I` 和显式条件拒绝，不使用可能被优化移除的 `assert`。重复安装可由当前受信 Agent 验证下一版，然后再次运行已信安装器；升级仍是用户执行安装程序，不做正式自动更新。

bootstrap 的 GitHub 下载只允许固定 HTTPS/443 主机、逐跳检查重定向且禁环境代理；其信任依据为独立公钥和 TLS，不宣称实现面板导入器的逐地址 DNS 固定策略。Agent 下载始终来自面板同源，安装器不允许重定向或环境代理。面板 origin 默认要求 HTTPS；HTTP 只接受字面回环 IP（含规范化的 IPv4-mapped 回环地址）或 `localhost`，不把其他主机名的 DNS 解析结果当作明文传输豁免。Agent 配置验证采用相同边界。

Agent 的根编在成品里，安装器不写 Agent 动态信任文件、不从面板领取“最新公钥”。根集合轮换通过受旧根批准的新 Agent 成品实现。初次 root 核对步骤及本机验证器来源必须写清，不能把这一步藏在面板复制的复杂命令中。

## 面板从 Release 导入与原子可见

选择固定仓库 `theLucius7/apsp`，管理员只输入规范 tag，不接受 URL/任意 repo。只导入正式已发布 Release；draft、错 tag、重复/未知 asset、未完成资产均拒绝。先获取 metadata/SUMS/minisig，compiled roots 验签，通过后才下载和核验所有制品。API 输出 proof，Agent 仍从面板 canonical 同源地址取得制品。

GitHub 下载客户端单独禁环境代理、禁止自动跳转，显式检查 HTTPS/443、精确主机白名单、重定向次数与每跳域名。解析全部地址并拒绝非公网/私网/localhost/IPv4-mapped 私网，固定通过检查的地址用于连接而保留原 hostname 做 TLS 验证，防止 DNS 重绑定。流式硬大小限制和超时适用于每个 asset。GitHub 自身 digest 不能替代 minisign；库不执行下载物来“检查版本”。

整个候选 Release 在同文件系统私有 staging 完成签名、大小、归档和安装后 binary 校验，fsync 后一次 rename 成 `data/artifacts/releases/<tag>`；列表与 descriptor 只读完整已验目录。同 tag 或相同组件键相同摘要幂等，不同摘要拒绝；半下载和失败 import 不改变现有集合。内存 mutex 串行 import，重启只扫描完整目录，不把残留 staging 曝光。

签名 Release 通过已鉴权的管理员运维导入接口准备，取代正常部署的 docker cp。2026-10-01 的插件目录调整后，网页仅展示插件及真实版本、架构，已移除全局 Release 导入表单；维护流程见[部署文档](../deploy.md#导入签名-release)。离线数据卷导入可保留完整签名树路径，但不能允许旧 unsigned 目录成为兜底。

## 独立版本与兼容旧 Agent

Agent 二进制 crate 版本改为 `0.3.0`；面板保持 `0.2.0`，两者不再共享产品版本号。internal crate/package 版本不等于 wire version。面板声明协议范围 `1..=1`，只接受实际实现的版本，按协议和 capabilities 判断兼容，不要求 Agent 与面板 semver 相同。

Agent 入口向 core 传自身构建版本，hello、static telemetry、本地 status、原生服务安装目录及自动升级比较均使用这个版本，不使用 core 的 CARGO_PKG_VERSION 代替 Agent 身份。监督器仍以已签候选及持久 current/pending 中对应的版本核对候选启动结果，不改为父进程版本。直接构建和预编译二进制打包都从 Agent 的独立 manifest 取得预期版本。面板 bootstrap 从已导入且协议兼容的签名 metadata 选择 Agent，可显式选择 version；不把 panel package version 或硬编码 0.3.0 当作默认版本。后续导入不自动替换运行节点。

新能力为 `artifact:minisign-v1`。旧 Agent 忽略新增 proof 字段，无法事后让它验签，因此面板在 manifest、制品下载、新诊断创建/领取处拒绝没有能力的设备并给出中文升级提示；继续接受旧状态、流量 batch 与确认，保留已有 running config。新 Agent 遇到旧面板 unsigned 目标拒绝应用，不停止当前已运行程序。能力缺失与根缺失都不能 fallback 到只校验 SHA256。

## CI 候选、用户本地签署和正式发布

1. Agent tag（如 `agent-v0.3.0`）触发构建，核对 tag 与 Agent crate 版本，在固定 Ubuntu 24.04/24.04-arm runner 原生构建 musl Agent；运行时依既有 Linux/amd64 工具链为两个目标架构构建，并构建两架构诊断包，生成固定安装器、metadata、canonical SUMS，并创建 draft Release。只使用公开根，不在 CI 配置签名私钥。
2. 用户下载精确候选及构建证据，本机重新核对 SHA256、tag/构建 commit/架构和元数据，使用带口令 minisign 私钥签 `SHA256SUMS`；只上传 `SHA256SUMS.minisig` 到同一 draft。CI 从不取得私钥或口令。
3. 手动 final-publish workflow 指定 tag，重新验签、校验整个资产集合、安装后二进制摘要和 tag commit 的必需 checks，发布前重取 tag commit、完整资产身份与摘要并确认未变；唯一最终变更是 draft → published。缺签名、缺架构、测试根、错误摘要或检查未通过均保持 draft。GitHub REST 不提供 tag、全部资产与发布状态的原子比较交换；最后复查到发布 PATCH 之间仍有仓库管理员并发写入的短窗口，当前流程不能宣称完全消除这种竞争。
4. 发布后不覆写资产。key root 改变必须先出受旧根签署的 transition Agent；不能只更新 Actions 的 public key 然后假定旧安装认识它。

运行时按上游版本、架构和构建脚本内容缓存。`tools/verify-release-runtime.py` 在命中缓存与新构建后均检查归档 SHA256、唯一普通二进制、ELF 目标架构及 `go version -m` 提供的固定 Go 版本、目标、CGO、标签和源码 revision；它只读取缓存，不执行缓存二进制。Go metadata 不一定保留设置运行时版本的链接参数，固定 revision 与预期上游版本的关联也不是不可伪造的构建证明。离线签名仍表示维护者批准具体成品字节，需要结合构建记录审查，不能把自描述 metadata 当成独立 attestation。

用户已选择在本机生成带口令私钥，由用户交互输入口令，只向项目提供 `.pub`。密钥是否完成离线保管需用户实际安排隔离的签名环境；本方案不将联网本机生成的带口令密钥宣称为已离线。口令不放在命令参数、环境变量、工具输出或聊天中；私钥保存在仓库外受保护位置并由用户保管备份。先使用 `crates/protocol/tests/fixtures/TEST_ONLY.key` 等明确标识的测试钥完成验证；首个正式公钥现记录在 `deploy/release-public-keys.json`（key ID `44B019C8269669B8`）。`agent-v0.3.0` 已完成候选构建、本地签署和正式发布；私钥仍由用户保存在 Mac，离线保管待完成，证据见 [执行进度](../../PROGRESS.md)。正式构建与发布拒绝已知测试公钥材料；隔离 CI 可显式预置测试根运行 bootstrap，不改变生产 Agent 的编译根边界。

2026-10-24 追加：用户决定在部署面板的 VPS 上无人值守签署，新增第二把正式公钥 `4D645C2885298F58`，原 Mac 公钥保留。私钥带口令加密，口令另存于该机受保护文件；这把密钥不满足离线保管，保护等级低于原根。根集合同时出现在 `deploy/release-public-keys.json`、仓库变量与生产面板环境，安装入口随之重新生成。只内嵌原根的 0.3.0 Agent 不认识新根，下列“正常轮换”的过渡要求对它们仍然有效。

## 多根轮换和泄漏应对

正常轮换：旧私钥签署一个内嵌“旧 pub + 新 pub”的 transition Agent；已安装旧 Agent/可信旧安装器可用旧 pub 验证它。完成用户重复安装、确认节点采用新 Agent 后，改用新私钥签后续 Release；再发布移除旧 pub 的 Agent 并安排替换，旧根才退出信任集合。未升级旧 Agent 不能读懂新根，需继续使用 transition Release 路径；不能从面板自动下载 pub 来绕过。

私钥泄漏时，攻击者能签出仍信任该根的任意制品。立即停止使用泄漏私钥和相关正式发布，记录受影响时间、根及资产；通过独立可信渠道发布新公钥和事件说明，用户核对后安装移除旧根的新 Agent/安装器，并审查已应用制品。不要让泄漏旧钥为新根背书而声称完成可信恢复；已受攻击节点需人工确认主机完整性。仅删 GitHub asset 或停面板下载不能撤销旧 Agent 内嵌的根。

新 Agent 移除旧根后，旧根签署的缓存/回滚包也失效；移除前需要提供新根签署的、内容匹配的证据或迁移到新制品，并确认已应用与 pending rollback 引用，避免无声破坏恢复。此版本不做联网撤销列表或可信时间服务；离线验证者无法凭空获知密钥泄漏，完整抗回退体系另行设计。

## 实现分工和验收

实现维持既定分层：protocol 提供 DTO/纯验签，core 管理制品 IO 与执行，Agent 二进制传入自己的版本；面板导入、安装器和发布工具共享相同格式，不修改适配器的持久化职责。

必要测试：有效与错误/缺失/超限签名；checksum 换行/正文篡改；四行 `.minisig` 与 trusted comment 篡改；错误根及多根；canonical 路径/错版本/架构/仓库/tag；signed archive 但 binary 不符；binary+marker 同改；同 revision、prepare、apply、rollback、intent recovery、诊断启动均拒绝未签；离线缓存恢复；旧 Agent gating 和账本连续；unsigned upgrade 预检失败保持运行/身份/ledger；systemd 外部重启；SSRF/重绑定/redirect/代理；import 原子失败/幂等；tag 双架构 draft 与 final publish 缺签保留。

密钥轮换测试用不同测试公钥证明旧根可验 transition Agent、transition 可验新根成品、旧 Agent 拒绝新根、移除旧根后拒绝旧签名。与 minisign CLI 交叉验证固定测试 proof，所有测试钥明确标记为测试，测试私钥可以作为公开夹具提交。正式密钥只公开公钥和签名，不提交生产 private key；测试成功不等于本机私钥已离线、正式签署完成或正式 Release 已发布。
