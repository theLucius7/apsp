# 签名发布与信任根

Agent 在 `crates/agent/Cargo.toml` 独立声明版本，当前源码为 `0.3.1` 候选，尚未签名发布；面板仍使用根 `Cargo.toml` 的 workspace 版本 `0.3.0`。Agent 标签使用 `agent-v<Agent版本>`。当前 wire 协议兼容范围为 `1..1`，记录在已签 `release.json`，不以面板产品版本代替协议兼容判断。

CI 为两种架构构建 musl Agent、固定上游运行时和当前 NodeQuality 包装器，生成六个平铺资产、静态 `install.sh`、`release.json` 与规范 `SHA256SUMS`，只建立 GitHub Release 草稿。运行时按固定版本、架构和构建脚本内容缓存；固定 Go 工具链在 amd64 构建机交叉编译 arm64。Agent 两种架构都使用对应原生 runner。NodeQuality 当前源码默认构建 r14：固定上游提交、原入口 SHA-256 和 AGPL-3.0 许可证不变，包装器新增能力需使用新的不可变组件版本。打包不运行基准测试；外部诊断下载继续遵循 ADR 0016。

[agent-v0.3.0](https://github.com/theLucius7/apsp/releases/tag/agent-v0.3.0) 已从冻结源码 `75cd846` 正式发布，包含 r2 包装器；维护者本机签署、发布验证和真实面板导入的结果见 [执行进度](../PROGRESS.md)。该 Release 的制品保持不可变，不随当前源码重打包或替换。r14 及后续新增能力须完成各自前置验收后使用新 Release 发布。当前 CI 按用户要求临时暂停，全部任务完成后才统一恢复；暂停期间不执行下文的工作流触发步骤。

缓存命中和新构建都先经过 `tools/verify-release-runtime.py`：检查归档与 SHA256、单个普通二进制、ELF 架构，以及 `go version -m` 读取的 Go 版本、目标平台、构建标签、CGO 和固定源码 revision。这个步骤只读取缓存内容，不执行缓存二进制。源码 revision 与预期版本的关联用于发现错误构建；这些可写入二进制的 metadata 不构成独立构建证明，运行时版本的链接参数也不一定保留在 Go metadata 中。维护者仍需核对候选和构建证据后签名。

`SHA256SUMS` 按 ASCII 路径排序，格式为小写 SHA-256、两个空格、规范路径、LF。制品路径为 `name/version/arch`，GitHub 平铺文件名由已签 metadata 的 `asset_name` 映射；另包含 `release.json` 与 `install.sh`。签名本身不在 SUMS 内，签名资产必须是完整四行 `SHA256SUMS.minisig`。

官方在线部署可直接复制面板接入命令，无需手动预装 bootstrap；命令下载固定官方 GitHub 入口并核对摘要，入口内置正式公开根并自动准备验证工具，再独立验证已签 Release。本文的手动预置流程继续用于自建信任域、离线部署或独立审查方式；调整范围见 [ADR 0037](adr/0037-bootstrap-and-selective-import.md) 与 [ADR 0041](adr/0041-cross-platform-enrollment.md)。

## 生产根与离线签名

用户在自己的设备上生成带口令的 minisign 密钥，只向项目提供 `.pub`。例如在仓库外的受保护目录执行以下命令，并在 minisign 的交互提示中设置口令：

```sh
minisign -G -p /受保护目录/apsp-release.pub -s /受保护目录/apsp-release.key
```

不要使用取消口令保护的选项，也不要把私钥或口令写入仓库、面板、CI、命令参数或聊天。私钥及加密备份由用户自己保管；要实现离线签名，应在与网络隔离的设备或签名环境中使用它。仅在联网本机生成带口令私钥，不代表已经完成离线保管。

首个正式公钥记录在 [`deploy/release-public-keys.json`](../deploy/release-public-keys.json)，minisign key ID 为 `44B019C8269669B8`。key ID 只便于辨认，建立信任时应核对完整公钥记录。该文件只有公开信息；生成时私钥保存在维护者本机的仓库外，未提交或上传。将已独立核对的公钥用于面板或 Agent 构建：

```sh
export APSP_RELEASE_PUBLIC_KEYS="$(cat deploy/release-public-keys.json)"
```

正式 Release 仍需完成候选构建、本地签署与发布校验；提供公钥本身不代表某个版本已发布。CI 集成测试继续使用明确标识的测试根。

生产公钥 JSON 数组通过仓库变量 `APSP_RELEASE_PUBLIC_KEYS` 固定在正式 Agent 编译时，每项可以是 minisign 公钥 base64 记录或完整 `.pub` 文本。公钥可公开；CI 不生成或读取正式私钥。Agent 使用 `minisign-verify` crate 验证签名，最多同时信任 8 个公钥。面板、安装器和运行期配置都不能给 Agent 追加或替换根；缺失、空、重复或无效的根集合会拒绝制品。自行构建的用户可以在编译时设置自己的 `APSP_RELEASE_PUBLIC_KEYS`，其产物属于自己的信任域。

远程命令是单独的节点授权：本机顶层配置 `allow_remote_commands` 默认 `false`，面板设置不能将其开启。节点操作者设置为 `true` 并重启 Agent 后，等于授权绑定面板以 Agent 服务账号执行任意 shell；制品签名不能约束这些命令。具体启停步骤见 [部署文档](deploy.md#远程命令的本地授权)。

已公开的 `crates/protocol/tests/fixtures/TEST_ONLY.key`、同目录 `TEST_ONLY_ROTATION.pub` 与其他 `TEST_ONLY*.pub` 只用于自动测试，不能成为生产根。正式构建与正式发布校验按实际 32 字节公钥拒绝这些 fixture，而非只比较可替换的 key ID。共享发布校验器内置两把已知测试公钥的拒绝名单，即使复制到仓库外仍可在正式发布校验中拒绝它们；仓库内还会扫描新增测试根。普通 bootstrap 允许操作者显式预置测试根，供隔离 CI 验收使用；正式安装必须独立配置并核对生产公钥。

操作者从 Release 草稿取得全部资产，并在使用正式私钥的离线设备执行：

```sh
minisign -S -m SHA256SUMS -s /离线设备中的私钥路径 \
  -x SHA256SUMS.minisig -t 'Apsp release agent-v0.3.0'
```

签名前需确认全部资产、源码版本与清单来自本次构建。只上传完整 `SHA256SUMS.minisig`，不上传私钥。运行 `Signed release draft` 的手动校验，提供 tag；默认仅验证，显式选择 publish 才在完整签名、metadata、安装器及所有资产校验通过后公开草稿。未知资产、重复路径、缺模块或缺架构、旧式签名、篡改的可信注释或测试钥都会失败。

tag 的最终 commit 必须与草稿记录的完整 build SHA 一致，并有同一 commit 的最新 main push CI 成功记录；以下五个 job 缺失、跳过、未完成或失败都会拒绝：`check`、`compose-smoke`、`Agent Linux musl (amd64)`、`Agent Linux musl (arm64)`、`Reality installation and accounting`。因此应先等待 main CI 通过，再创建指向该提交的 Agent tag。正式生产根尚未提供时，发布流程保持 fail closed；测试根可用于本地和 PR 验收，不能生成正式候选。

Ubuntu runner 固定为 24.04；GNU、macOS、Windows、FreeBSD 与完整运行时的源码和 [Platform validation](../.github/workflows/platforms.yml) 手动入口继续保留。当前主线 `ci.yml` 也包含全平台检查定义，但所有工作流按用户要求暂停，待全部任务完成后统一确定恢复范围。正式 Linux musl Release 的既有验收不代表其他平台已经通过，自动发布门禁仍按所选发布源码的实际成功记录校验。

发布工具记录每个 GitHub asset 的 ID、name、digest、size、state，按已选 ID 下载，先检查 GitHub digest 与实际 bytes 一致，再以独立生产根验证完整 minisign 和所有制品。GitHub digest 不是签名替代。验完后重新读取 tag 对象与 commit、完整 asset 集合及 CI run/attempt/job ID，必须与验证前一致才调用唯一的 draft→published PATCH；发布后还复查资产和 tag，并保存公开验证证据。workflow concurrency 串行同 tag 的本流程操作。

草稿通过已认证的 List releases 接口发现：每页 100 条、最多 10 页，遍历结束后要求 tag 精确且唯一匹配，再按 release ID 读取并复核 ID、tag 与完整 build SHA。缺失、重复 ID、重复 tag、格式异常或达到页数上限仍未遍历结束时均拒绝；每轮发布复查都重新执行此查找，身份变化时拒绝继续。

GitHub REST 没有把 tag、全部 assets 和 Release 发布合成一个条件原子操作的接口；最后复查到 PATCH 之间仍有极短的外部写入竞争窗口。本流程不宣称能阻止拥有仓库写权限的并发管理员在该窗口更换对象。发布时应暂停其他管理员对该 tag/Release 的写入，并可由仓库所有者开启 [GitHub immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases) 限制发布后的资产和 tag 变更。发布后复查失败属于需要人工处理的已发布事件，不能冒称草稿仍未公开。

## 首次安装

首次信任必须来自面板之外。操作者通过自己信任的仓库源码取得 `tools/bootstrap.py` 与相邻 `tools/release.py`，预先安装 minisign、独立核对并渲染当前 Linux 静态执行器为相邻受保护的 `trusted-install.sh`，并将独立核对的公钥 JSON 放入 root 拥有且不可被组或其他用户修改的路径，例如 `/etc/apsp/trust/public-keys.json`；所有父目录同样受保护。不要从面板下载公钥后就把它视为根，也不要把面板返回的脚本文本直接管道交给 sudo。

自建根或离线部署若使用 `sudo apsp-bootstrap ...`，先按[部署文档](deploy.md#手动准备可信-bootstrap)把这两个可信源码文件安装到 root 控制的 `/usr/local/lib/apsp/`，并建立 `/usr/local/bin/apsp-bootstrap` 启动脚本；脚本和目录不得由普通用户修改。也可以直接使用下面的完整 Python 路径形式。这个预置步骤独立于面板与待安装的 Agent。

面板只提供目标 tag、面板 origin 和一次性令牌，用已有的可信 bootstrap 执行：

```sh
sudo python3 /可信源码副本/tools/bootstrap.py \
  --tag agent-v0.3.0 --panel https://panel.example.com \
  --token='<一次性令牌>' \
  --trusted-keys /etc/apsp/trust/public-keys.json
```

bootstrap 从固定官方 GitHub Release 取得完整 proof、静态安装器和 Agent 二进制，不使用环境代理。先验证完整 minisign、正式 Release 安装器摘要和 Agent 长度/摘要。官方单行 Linux 入口执行自身固定 blob 中嵌入的受信静态安装器；手动预置同一可信安装器后也可安装旧 0.3.0，不修改已发布安装器或签名。原生平台使用已签 Agent 的 install-service。首次请求和每一跳重定向都要求 HTTPS、443、无 URL 凭据；默认只允许 `github.com`、`release-assets.githubusercontent.com`、`objects.githubusercontent.com`，最多五跳。每个文件共享 300 秒绝对下载期限，单次连接/读取最多 20 秒，接近期限时缩短读取超时；持续慢流不能刷新总期限。可加 `--mirror https://mirror.example.com`，以镜像前缀加完整 GitHub URL 下载；只额外允许该镜像来源，不发送一次性令牌或设备凭据，失败不回退面板。

安装器仅使用 bootstrap 已下载的本地签名包，再次按已签长度和 SHA256 核对 Agent；缺失或损坏立即拒绝，完全不向面板下载二进制。检查使用 `python3 -I` 和显式拒绝逻辑，不依赖可被优化模式移除的 `assert`。`--release-dir` 离线目录必须同时包含签名、清单、安装器和对应 Agent 的 GitHub asset 文件。面板地址通常必须是 HTTPS，HTTP 只允许字面回环地址或 `localhost`，仅用于注册和后续配置通信。新 Agent 内置根进一步验证：

```sh
/暂存/0.3.0/apsp-agent verify-installed \
  --binary /暂存/0.3.0/apsp-agent --name agent --format raw
```

`--name` 和 `--format` 是调用方要求的制品身份，不能根据不可信缓存猜测。Agent unit 固定 `agent/raw`，运行时 unit 固定 `sing-box/tar.gz`，防止另一个合法签名制品被放到错误执行位置。已有配置还必须通过 `--config /etc/apsp/agent.toml verify-cache`：只读检查已应用运行时、未完成操作的前后版本及未完成诊断任务。全部通过后才注册、保留原身份与账本、切换版本链接和重启 Agent；运行时保持独立服务。

已有受信签名 Agent 升级时可增加 `--trusted-agent /opt/apsp/core/current/apsp-agent`，先验证旧 Agent 自身，再让它以编译根离线验证新 proof，不需要从面板取得新公钥。旧版 unsigned Agent 没有这个能力，必须按首次安装方式预置独立信任根。

## 无签名旧缓存迁移

旧 `0.1.0 → 0.2.0` 实机升级验证了身份、配置与账本连续性；它没有验证旧无签名缓存向签名版本迁移。安装器不会把旧 `.artifact.json`、已有 SHA256SUMS 或面板返回的摘要当成可信 proof，也不会在切换后再发现运行时无法恢复。只要旧运行时、未完成操作或诊断缓存缺少匹配签名，就在注册、版本链接和服务修改之前拒绝升级，保留已有身份、账本和服务。

操作者可以在升级前补齐与旧二进制完全匹配的已签 proof，但不能用新构建的同版本号替换旧二进制。具体顺序是：

1. 独立核对生产根，下载包含对应旧版本与架构的完整已签发布；用 `tools/release.py verify --bundle <发布目录> --trusted-keys <独立公钥文件> --publication` 验证完整签名、所有资产与安装器。
2. 按已签 metadata 的版本建立 root 拥有的私有暂存目录，将当前实际引用的旧二进制复制为 `<暂存>/<版本>/<binary_name>`，同时复制 `release.json`、`SHA256SUMS`、`SHA256SUMS.minisig`。用已通过首次信任检查的新 Agent 执行 `verify-installed --binary <暂存>/<版本>/<binary_name> --name <预期模块名> --format <预期格式>`；例如运行时必须是 `--name sing-box --format tar.gz`，Agent 必须是 `--name agent --format raw`。失败时停止迁移；不能用清空身份、账本或重装运行时绕过。
3. 验证成功后，确认旧版本目录还没有任何 proof 文件，以不覆盖的方式将这三个原始 proof 文件补入原版本目录。任何现有 proof 或不同二进制都需要先查明原因。对已应用、未完成操作的 previous/target 和诊断缓存逐一执行这个步骤，而非仅检查 `current`。
4. 使用新 Agent 对原配置执行 `--config /etc/apsp/agent.toml verify-cache`；只有完整检查通过才重复运行可信 bootstrap。没有能匹配旧二进制的正式签名发布时，升级保持阻塞，先通过 issue 记录所缺的发布证据。

迁移期间应避免其他管理员同时修改缓存或安装版本。预检与人工补证明不构成覆盖旧 Agent 的跨版本事务锁。若启动恢复在连接面板之前发现无效 proof，Agent 在本地记录失败并停止启动；此时没有连接可发送 `ApplyResult`，应从本机日志和状态定位问题，不能把面板尚未收到失败报告当作恢复成功。

本地受控验证可使用 `--release-dir <本地已签测试发布目录>` 代替 GitHub 下载，该目录同时包含完整 proof 与对应 Agent flat asset；Linux 执行器须另由独立受保护来源预置。Agent 不从面板下载，fixture 根只用于明确隔离的测试。此选项不构成正式发布或实机验收，当前 CI 仍按用户安排暂停。`tools/release.py assemble --arch amd64` 可生成仅本机架构的测试 bundle，仍包含 Agent、运行时和 NodeQuality 三个已签模块；正式发布入口强制六制品，拒绝单架构测试 bundle。

## 公钥轮换与私钥泄漏

正常轮换按以下顺序执行：

1. 用户生成并独立核对新公钥；构建同时内置旧、新公钥的过渡 Agent，用旧私钥签署该 Release。
2. 节点通过已信任的旧公钥验证并重复安装过渡 Agent。确认节点已支持新根后，用新私钥签署后续 Release。
3. 提供新根认可的运行时、诊断及回滚缓存证明，确认所有正在使用和未完成操作引用的制品均可验证，再安装移除旧根的 Agent。尚未升级的节点继续经过过渡 Release，不能从面板直接领取新公钥跳过此步骤。

私钥泄漏后，立即停止使用该钥签发并暂停相关发布，记录受影响根、时间和资产。通过独立可信渠道向用户公布新公钥和事件说明；用户重新核对信任起点，安装移除泄漏根的新 Agent，并审查实际运行制品及主机完整性。泄漏旧私钥已经能替攻击者背书，因此不能仅用它签一个新根就宣称完成可信恢复。删除 Release 或停止面板下载也无法撤销已编入离线 Agent 的旧根。

换根会使仅由旧根签署的缓存和回滚包失效，恢复前须补充新根签署、字节完全一致的证明，或迁移到可信新版本。当前没有联网撤销列表、可信时间或完整抗回退机制。完整取舍见 [ADR 0017](adr/0017-signed-release-artifacts.md)。

## 验证与发布状态

本地和 PR 验证使用明确标识的测试公钥、测试私钥与隔离环境；验收要求包括完整 minisign、错误或篡改资产、轮换、安装前缓存预检、systemd 重启和 Release 导入。测试成功不能替代生产信任根配置、用户本地签署及正式发布验证。当前阶段的实测结果和仍未验证范围以 [PROGRESS.md](../PROGRESS.md) 为准；正式发布证据来自 `Signed release draft` workflow 的验证产物和最终 Release 状态。

## Agent 自动更新与旧版本衔接

面板仍通过导入并验证 Release 来选择协议和平台兼容的版本，但更新描述中的 URL 指向固定 GitHub Release，Agent 根据已签仓库、标签和资产名称重建地址后比较，使用独立匿名客户端下载。可在服务器新增或编辑窗口设置「Agent 下载加速」；镜像只改变传输路径，不改变发布信任根、版本和 ABI 检查。自更新下载还逐跳检查公网 DNS 并固定解析地址，拒绝面板来源、非公网目标、凭据和不受支持的重定向。运行时与配置仍从绑定面板下载。

旧 `/api/bootstrap/{version}/{arch}` 及 Agent 专用面板二进制路由已关闭，持有有效令牌也不会返回二进制。依赖这些接口的旧安装器可改用本次源码固定 blob/SHA 保护的单行入口：它核对完整 Release proof，直接 GitHub 下载已签 Agent，并使用内嵌受信 Linux 执行器。旧 Agent 若只接受面板同源更新 URL，后续升级需使用独立入口手动执行。不要修改既有签名或覆盖旧 Release 来兼容。原生平台继续从 GitHub 获取已签发布目录，独立验签后运行 `install-service`；已弃用的面板模板已移除。

新 Agent 的自动升级请求显式携带 `download_source=github`，旧/未知/重复查询返回空候选；设备凭据仅用于面板认证，不进入 GitHub 或镜像请求。当前入口的旧 Linux 首装兼容不等于旧 Agent 已具备 GitHub 自动更新能力。
