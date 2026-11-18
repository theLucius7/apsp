# 签名发布与信任根

Agent 在 `crates/agent/Cargo.toml` 独立声明版本，当前工作树源码版本为 `0.4.0`（尚未打标签、尚未离线签名、尚未发布 GitHub Release 或接入生产）；面板使用根 `Cargo.toml` 的 workspace 版本 `0.3.0`。Agent 标签格式固定为 `agent-v<版本>`，wire 兼容范围由已签 `release.json` 的 `protocol_min/protocol_max` 决定，不以面板产品版本代替协议兼容判断。当前 GitHub Actions 工作流按协作规则临时暂停，全部任务完成并统一恢复验证前，不执行下文的工作流触发与发布步骤。

## 已发布历史版本（0.3.9 及更早）与新源码流程的边界

必须严格区分线上已公开的历史 Release 与当前工作树的一体化源码流程：

- **已公开的最新正式版本为 `agent-v0.3.9`**（2026-11-17 从 `bf71ea39` 签名发布；历次发版记录见 [Release 历史](https://github.com/theLucius7/apsp/releases) 与 [当前交接](handoff/2026-11-17-reality-hardening.md)）：`0.3.0` 至 `0.3.9` 均属于 [ADR 0141](adr/0141-self-contained-agent-and-desired-service-config.md) 之前的历史形态，当时 Release 同时发布独立的 `sing-box`（`0.3.9` 为 `1.14.4-apsp.2`，含认证会话、Cronet 与 [ADR 0139](adr/0139-reality-hardening-on-stable-runtime.md) 的 REALITY 加固补丁 `b8c49498`；`0.3.8` 为 `1.14.4-apsp.1`）与 `nodequality` 归档，由旧面板制品目录导入下发。已发布的 GitHub Release 资产永久不可变，不随当前源码重打包或替换；也不得用旧 Release 的签收记录声称新版一体化 Agent 已经上线。`agent-v0.3.0`（`75cd846`）与 `agent-v0.3.1`（`5a436e3a`，首次启用第二把根 `4D645C2885298F58`）的构建与签署记录保留在 [执行进度](../PROGRESS.md)。
- **当前工作树源码（`0.4.0` 候选设计，[ADR 0141](adr/0141-self-contained-agent-and-desired-service-config.md)）改为单单元自包含 `raw` Agent 发布**：用户与管理员无需再单独安装或导入任何可选装运行时与诊断组件；面板删除独立制品库与运行时手工启用流程，只管理签名 Agent 发布（`/api/system/agent-releases*`）并按当前有效节点/链路下发服务配置。`Signed release draft`（[`.github/workflows/release.yml`](../.github/workflows/release.yml)）、`tools/release.py` 与 `tools/publish.py` 不再发布独立的 `sing-box`、`nodequality`、`tcpquality` 或 `ipquality` 可选装归档，仅发布各目标平台的已签 `raw` `apsp-agent` 单文件（`<= 128 MiB`）及受签名声明约束的对应源码包（Corresponding Source）。

## 一体化源码构建与草稿组装流程

在新的源码流程中，所有实装运行库与受控诊断载荷都在编译 `apsp-agent` **之前**按目标架构与 ABI 独立准备并校验，再由 `crates/agent/build.rs` 在编译期内嵌进单文件 `apsp-agent`：

1. **构建前供应目标匹配的运行时归档（`runtime-inputs`）**：
   - 使用固定 Go `1.26.8` 工具链与 [`tools/singbox-builder.Dockerfile`](../tools/singbox-builder.Dockerfile) 从锁定上游 `v1.14.4`（`c005f35300ce76427dc285481ec5695f5b179cd2`）加 `tools/singbox-runtime/` 补丁队列（`b8c49498e4ec16c883bc5bc5002c5221a08c3dc5`）构建 `1.14.4-apsp.2` 的四个 Linux 目标：`linux-gnu-amd64`、`linux-gnu-arm64`（`with_purego`，附带对应架构且固定摘要的 `libcronet.so`）与 `linux-musl-amd64`、`linux-musl-arm64`（`with_musl`，静态链接 Cronet，不含动态库）。
   - 缓存命中与新构建均先通过 `tools/verify-release-runtime.py --arch <amd64|arm64> --libc <gnu|musl>` 静态检查归档摘要、ELF64 架构、链接方式、`libcronet.so` 摘要契约以及 `go version -m` 的 Go 版本、构建标签、CGO、源码 revision 与 `tools/singbox-runtime/modules.txt` 依赖闭包（不执行缓存二进制；Go 嵌入 metadata 仅用于拦截错误构建，不替代人工核对与离线签名）。同时通过 `tools/prepare-agent-linux-inputs.py package-singbox-sources` 生成 `sing-box-1.14.4-apsp.2-sources.tar.gz`。
2. **构建前在双架构原生环境供应实装诊断包与对应源码（`diagnostic-inputs`）**：
   - 在 `ubuntu-24.04`（`amd64`）与 `ubuntu-24.04-arm`（`arm64`）原生环境执行 `tools/prepare-agent-linux-inputs.py prepare-bundled-tools`，生成自包含 `network-workbench`（`1.0.0`，内置受控 Python 解释器、标准库、CA 证书与全部必需已授权诊断工具）和 `-r22` `nodequality`（`a92fca6c0067df29ddd03fdc2fee6f3000f64545-r22`，保留 17 份固定上游来源、4 份完整许可证、`execution-admission.json` 的 `full_start_allowed=false` 完整验机门禁，并附带目标架构基础运行工具），同时产出各自的 `<arch>.sources.tar.gz`。打包过程不运行基准测试；外部诊断下载禁令继续遵循 ADR 0016。
   - 执行 `tools/prepare-agent-linux-inputs.py prepare-ipquality` 构建 `87397e2c3196ec796f5477c83343c2354df601ea-node-r1` 离线 Debian 12 rootfs 捆绑包及配对 `<arch>.sources.tar.gz`。该步骤强制从受控仓库变量读取经独立审批的构建镜像摘要（`APSP_IPQUALITY_BUILDER_IMAGE_SHA256[_AMD64/_ARM64]`）、许可审查文件（`APSP_IPQUALITY_LICENSE_REVIEW`）与 Debian 密钥环来源证明（`APSP_DEBIAN_KEYRING_PROVENANCE[_AMD64/_ARM64]`）；未配置或未获审批时直接拒绝构建并阻断草稿，绝不降级为空输入或不受控下载。
3. **按精确目标编译四路 Linux 自包含 `raw` Agent（`agent`）**：
   - 设置 `APSP_RELEASE_EMBED_REQUIRED=1` 与 `APSP_NATIVE_TCP_SOURCE_COMMIT=$GITHUB_SHA`，每个构建矩阵单元只下载与自身 `(arch, libc)` 完全匹配的单个 `input-runtime-*` 归档及同架构 `input-diagnostics-linux-<arch>`，从源头杜绝 GNU/musl、CPU 架构或 Cronet 动静态库错配。
   - Linux 静态 musl 双架构由 `tools/build-agent.sh <amd64|arm64>` 在对应原生 runner 构建，产出 `agent/<版本>/<amd64|arm64>`（资产名 `agent-<版本>-linux-musl-<amd64|arm64>`）；Linux 动态 GNU 双架构由 `tools/build-agent.py <x86_64|aarch64>-unknown-linux-gnu` 构建，产出 `agent/<版本>/linux-gnu-<amd64|arm64>`（资产名 `agent-<版本>-linux-gnu-<amd64|arm64>`）。每个二进制均内置 `apsp-agent tcp-probe` 子命令与唯一 `APSP_EMBEDDED_COMPONENTS_V1` 结构化组件清单帧，体积严格受控在 `128 MiB` 以内。
4. **组装无独立可选装组件的草稿与对应源码声明（`draft`）**：
   - `tools/release.py assemble`（不传旧 `--runtime-version` / `--include-musl-runtime`）显式要求 `--target amd64 --target arm64 --target linux-gnu-amd64 --target linux-gnu-arm64 --require-paired-sources`。
   - `release.json` 保持 `schema: 1` 不变，`artifacts` 数组**仅收录 `name == "agent"`（`format: "raw"`，无 `auxiliary_files`）条目**；`SHA256SUMS` 按 ASCII 路径排序（小写 SHA-256、两个空格、规范路径、LF），仅含 `release.json`、`install.sh` 与 `agent/<版本>/<target>`，使线上已部署的 `0.3.7`/`0.3.9` Agent 自更新器与 `--trusted-agent` 校验器无需修改协议即可直接验签升级。
   - 七份 GPL/AGPL 对应源码归档（`sing-box-1.14.4-apsp.2-sources.tar.gz` 以及 `amd64`/`arm64` 双架构的 `nodequality`、`network-workbench`、`ipquality` `*-sources.tar.gz`）在计算 `SHA256SUMS` 前以 `# APSP_PAIRED_SOURCE: <sha256> <size> <asset_name>` 写入 `install.sh` 头部注释，随 `install.sh` 一起受离线 `SHA256SUMS.minisig` 签名保护，并作为独立 Release 资产随草稿一并上传。工作流不持有任何签名私钥，仅创建未公开的 GitHub Release 草稿。

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

2026-10-24 用户决定在部署面板的 VPS 上签署发布，新增第二把正式公钥，key ID `4D645C2885298F58`，与原公钥并列记录在同一文件、仓库变量 `APSP_RELEASE_PUBLIC_KEYS` 和生产面板环境中。私钥带口令加密，保存在 VPS 的仓库外受保护目录，口令另存于该机的独立受保护文件，以便无人值守签署；它不进入仓库、CI、面板数据或聊天。这把密钥所在机器联网并运行公网服务，保护等级低于维护者 Mac 上的原密钥：该机被攻破即可签出受信任的 Agent，应按下文 [ADR 0017](adr/0017-signed-release-artifacts.md) 的泄漏流程处理。

只信任原公钥的已安装 Agent（`agent-v0.3.0` 及更早）不认识第二把公钥，无法验证仅由 VPS 密钥签署的 Release；这类设备升级时仍需由原 Mac 密钥签署过渡版本。自 `0.3.1` 起编译进 Agent 的根集合与重新生成的安装入口都包含两把公钥。

正式 Release 仍需完成候选构建、本地签署与发布校验；提供公钥本身不代表某个版本已发布。CI 集成测试继续使用明确标识的测试根。

生产公钥 JSON 数组通过仓库变量 `APSP_RELEASE_PUBLIC_KEYS` 固定在正式 Agent 编译时，每项可以是 minisign 公钥 base64 记录或完整 `.pub` 文本。公钥可公开；CI 不生成或读取正式私钥。Agent 使用 `minisign-verify` crate 验证签名，最多同时信任 8 个公钥。面板、安装器和运行期配置都不能给 Agent 追加或替换根；缺失、空、重复或无效的根集合会拒绝制品。自行构建的用户可以在编译时设置自己的 `APSP_RELEASE_PUBLIC_KEYS`，其产物属于自己的信任域。

远程命令是单独的节点授权：本机顶层配置 `allow_remote_commands` 默认 `false`，面板设置不能将其开启。节点操作者设置为 `true` 并重启 Agent 后，等于授权绑定面板以 Agent 服务账号执行任意 shell；制品签名不能约束这些命令。具体启停步骤见 [部署文档](deploy.md#远程命令的本地授权)。

已公开的 `crates/protocol/tests/fixtures/TEST_ONLY.key`、同目录 `TEST_ONLY_ROTATION.pub` 与其他 `TEST_ONLY*.pub` 只用于自动测试，不能成为生产根。正式构建与正式发布校验按实际 32 字节公钥拒绝这些 fixture，而非只比较可替换的 key ID。共享发布校验器内置两把已知测试公钥的拒绝名单，即使复制到仓库外仍可在正式发布校验中拒绝它们；仓库内还会扫描新增测试根。普通 bootstrap 允许操作者显式预置测试根，供隔离 CI 验收使用；正式安装必须独立配置并核对生产公钥。

操作者从 Release 草稿取得全部资产，核对 `release.json`、`install.sh`（含 `# APSP_PAIRED_SOURCE:` 对应源码声明）、全部 `raw` Agent 与配对 `*-sources.tar.gz` 均来自本次构建后，在使用正式私钥的离线设备执行（以候选标签 `<版本>` 为例）：

```sh
minisign -S -m SHA256SUMS -s /离线设备中的私钥路径 \
  -x SHA256SUMS.minisig -t 'Apsp release agent-v<版本>'
```

只上传完整四行 `SHA256SUMS.minisig`（签名文件本身不在 `SHA256SUMS` 内），不上传私钥。随后运行 `Signed release draft` 的手动校验（`verify-publication`），提供 `tag`；默认仅验证，显式选择 `publish` 才在完整签名、`release.json`、`install.sh`、每个平台 `raw` Agent 内嵌组件清单帧（核对 ABI、架构、GNU/musl Cronet 隔离与 `tcp-probe` 构建提交绑定）以及全部配对对应源码包校验通过后公开草稿。任何独立可选装组件条目、未知资产、重复路径、缺平台配对、旧式签名、篡改的可信注释或测试公钥都会拒绝发布。

tag 的最终 commit 必须与草稿记录的完整 build SHA 一致，并有同一 commit 的最新 `main` push CI 成功记录；以下七个必选 job 缺失、跳过、未完成或失败都会拒绝：`check`、`compose-smoke`、`Agent Linux musl (amd64)`、`Agent Linux musl (arm64)`、`Agent Linux gnu (amd64)`、`Agent Linux gnu (arm64)`、`Reality installation and accounting`（若发布包额外纳入 `macos-arm64`、`windows-*` 或 `freebsd-*` 原生 Agent 目标，对应平台的 `ci.yml` 任务也必须全部成功）。因此必须先等待 `main` CI 全部通过，再创建指向该提交的 Agent tag。Ubuntu runner 固定为 `24.04` / `24.04-arm`；[Platform validation](../.github/workflows/platforms.yml) 手动入口继续保留，详情见 [设备平台与能力](platforms.md)。

`tools/publish.py` 记录每个 GitHub asset 的 ID、name、digest、size、state，按已选不可变 ID 下载，先检查 GitHub digest 与实际字节一致，再以独立生产根验证完整 minisign、所有 `raw` Agent 及全部配对对应源码包（GitHub digest 不替代离线签名）。验完后重新读取 tag 对象与 commit、完整 asset 集合及 CI run/attempt/job ID，必须与验证前完全一致才执行唯一的 `draft -> published` `PATCH`；发布后再次复核资产与 tag，并输出包含 `embedded_components` 与 `source_offers` 下载地址的 `publication-evidence.json`。workflow `concurrency` 串行同 tag 的流程操作。

草稿通过已认证的 List releases 接口发现：每页 100 条、最多 10 页，遍历结束后要求 tag 精确且唯一匹配，再按 release ID 读取并复核 ID、tag 与完整 build SHA。缺失、重复 ID、重复 tag、格式异常或达到页数上限仍未遍历结束时均拒绝；每轮发布复查都重新执行此查找，身份变化时拒绝继续。

GitHub REST 没有把 tag、全部 assets 和 Release 发布合成一个条件原子操作的接口；最后复查到 PATCH 之间仍有极短的外部写入竞争窗口。本流程不宣称能阻止拥有仓库写权限的并发管理员在该窗口更换对象。发布时应暂停其他管理员对该 tag/Release 的写入，并可由仓库所有者开启 [GitHub immutable releases](https://docs.github.com/en/code-security/concepts/supply-chain-security/immutable-releases) 限制发布后的资产和 tag 变更。发布后复查失败属于需要人工处理的已发布事件，不能冒称草稿仍未公开。

## 首次安装与过去已签 Agent 升级

首次信任必须来自面板之外。操作者通过自己信任的仓库源码取得 `tools/bootstrap.py`、`tools/release.py` 与 `tools/legacy_agent_checkpoint.py`，预先安装 minisign、独立核对并渲染当前 Linux 静态执行器为相邻受保护的 `trusted-install.sh`，并将独立核对的公钥 JSON 放入 root 拥有且不可被组或其他用户修改的路径（例如 `/etc/apsp/trust/public-keys.json`，所有父目录同样受保护）。不要从面板下载公钥后将其视为信任根，也不要把面板返回的文本直接管道交给 `sudo`。手动准备步骤见 [部署文档](deploy.md#手动准备可信-bootstrap)。

面板只提供目标版本/标签、面板 origin 和一次性令牌，用已有的可信 bootstrap 执行（以已发布版本 `agent-v0.3.9` 为例）：

```sh
sudo python3 /可信源码副本/tools/bootstrap.py \
  --tag agent-v0.3.9 --panel https://panel.example.com \
  --token='<一次性令牌>' \
  --trusted-keys /etc/apsp/trust/public-keys.json
```

bootstrap 从固定官方 GitHub Release 取得完整 proof 与对应目标平台的 `raw` Agent 二进制，不使用环境代理。先验证完整 minisign、`release.json`、`install.sh` 摘要和 Agent 长度/摘要；官方单行 Linux 入口执行自身固定 blob 内嵌的受信静态安装器，原生平台使用已签 Agent 的 `install-service`。首次请求与每一跳重定向均要求 HTTPS、443、无 URL 凭据，默认仅允许 `github.com`、`release-assets.githubusercontent.com`、`objects.githubusercontent.com`（最多五跳，单文件总期限 300 秒、单次连接/读取上限 20 秒）；可选 `--mirror https://mirror.example.com` 仅改变 GitHub 资产传输路径，不携带令牌或设备凭据，失败绝不回退面板。

安装器仅使用本地已下载验签的包，再次按已签长度与 SHA-256 核对 `raw` Agent（`python3 -I` 显式拒绝，不依赖 `assert`），并由新 Agent 内置编译根执行自检：

```sh
/暂存/<版本>/apsp-agent verify-installed \
  --binary /暂存/<版本>/apsp-agent --name agent --format raw
```

已有配置还必须通过 `--config /etc/apsp/agent.toml verify-cache`：只读核验已激活组件（支持新版内嵌释放的 `.bundled.json` 清单摘要与历史已签释放目录的 `release.json` proof）、未完成操作的前后版本及未完成诊断任务。全部通过后才保留原身份与账本、切换版本链接并重启 Agent；独立系统服务 `apsp-singbox@main` 在设备收到面板有效服务配置时由一体化 Agent 自动提取校验内置运行时并管理。

已有受信签名 Agent（如已部署的 `0.3.7` / `0.3.9`）手动重装升级时可增加 `--trusted-agent /opt/apsp/core/current/apsp-agent`，先验证旧 Agent 自身，再由其编译根执行 `verify-release` 离线核验新版本 proof（由于新版本 `release.json` 与 `SHA256SUMS` 严守 `schema: 1` 及 `raw` `agent` 路径集合，旧版已签 Agent 可直接验签过渡）。旧版无签名 Agent 没有该能力，须按首次安装方式预置独立信任根。

## 历史无签名缓存迁移与本地测试目录

若极早期无签名环境升级时，旧运行时、未完成操作或诊断缓存缺少匹配签名或 `.bundled.json` 清单，预检会在修改身份、服务单元和 `current` 链接前直接拒绝升级，保留原身份、账本与运行服务，不会把旧 `.artifact.json` 或面板返回摘要当作可信 proof。操作者若需为旧二进制补齐与之字节完全一致的历史已签 proof（不能用重编同版本号二进制替换）：

1. 独立核对生产根，下载包含对应旧版本与架构的完整历史已签 Release，运行 `tools/release.py verify --bundle <发布目录> --trusted-keys <独立公钥文件> --publication` 验证完整签名与资产。
2. 在 root 私有暂存目录复制当前实际引用的旧二进制及匹配的 `release.json`、`SHA256SUMS`、`SHA256SUMS.minisig`，用已通过首次信任检查的新 Agent 执行 `verify-installed --binary <暂存>/<版本>/<binary_name> --name <预期模块名> --format <预期格式>`；失败即停止迁移，不得清空身份或账本绕过。
3. 验证通过且确认旧版本目录尚无任何 proof 文件后，以不覆盖方式补入上述三个原始 proof 文件（对已应用、未完成操作的 previous/target 与诊断缓存逐一执行）。
4. 对原配置运行 `--config /etc/apsp/agent.toml verify-cache` 通过后，再执行可信 bootstrap。迁移期间避免并发修改缓存；启动恢复若在连接面板前发现无效 proof 会在本地记录并停止启动，应检查本机日志而非以面板未收到 `ApplyResult` 视为成功。

本地受控验证可使用 `--release-dir <本地已签测试发布目录>` 代替 GitHub 下载（目录须包含完整 proof 与对应平台 `raw` Agent 平铺资产，Linux 执行器另由独立受保护来源预置；`fixture` 测试根仅用于隔离测试，不构成正式发布或实机验收）。`tools/release.py assemble --arch amd64 --no-auto-paired-sources` 可在隔离 CI 中生成仅本机单架构的测试 bundle；正式发布校验器 `tools/publish.py` 强制要求 Linux `musl` 与 `gnu` 双架构完整集合及全部配对对应源码包，拒绝任何单架构或含独立组件条目的测试 bundle。

## 公钥轮换与私钥泄漏

正常轮换按以下顺序执行：

1. 用户生成并独立核对新公钥；构建同时内置旧、新公钥的过渡一体化 Agent，用旧私钥签署该 Release。
2. 节点通过已信任的旧公钥验证并升级至过渡 Agent。确认节点已支持新根后，用新私钥签署后续 Release。
3. 确认节点活跃运行时与诊断目录均已切换至新一体化 Agent 校验的内置组件（或具备新根认可的证明），再安装移除旧根的 Agent。尚未升级的节点继续经过过渡 Release，不能从面板直接领取新公钥跳过此步骤。

私钥泄漏后，立即停止使用该钥签发并暂停相关发布，记录受影响根、时间和资产。通过独立可信渠道向用户公布新公钥和事件说明；用户重新核对信任起点，安装移除泄漏根的新 Agent，并审查实际运行制品及主机完整性。泄漏旧私钥已经能替攻击者背书，因此不能仅用它签一个新根就宣称完成可信恢复。删除 Release 或停止面板下载也无法撤销已编入离线 Agent 的旧根。

换根会使仅由旧根签署的历史缓存失效，恢复前须迁移到可信新版本的一体化内置组件，或补充新根签署且字节完全一致的证明。当前没有联网撤销列表、可信时间或完整抗回退机制。完整取舍见 [ADR 0017](adr/0017-signed-release-artifacts.md)。

## 验证状态与 Agent 自动更新

本地和 PR 验证使用明确标识的测试公钥、测试私钥与隔离环境；验收范围包括完整 minisign、篡改资产与源码包拦截、公钥轮换、升级前缓存预检、Supervisor 试用回退（覆盖内置组件提取校验与活跃运行时切换恢复）及面板 Release 收录。测试通过不替代生产信任根配置、受控审批变量准备、用户离线签署及正式发布校验。当前实测结果与未验证范围以 [PROGRESS.md](../PROGRESS.md) 为准。

面板通过 `/api/system/agent-releases/import` 收录并验签 GitHub Release（要求 `artifacts` 仅含 `raw` Agent，且核对已缓存实体包内的内置组件摘要归属），自动升级接口（`/api/agent/v1/update?download_source=github`）按节点上报的 `os`、`arch`、`libc` 返回协议兼容的更高稳定版本候选（Linux `musl` 匹配 `linux-musl-<arch>` 与兼容 `<arch>`，Linux `gnu` 匹配 `linux-gnu-<arch>`）。更新描述中的下载地址指向固定官方 GitHub Release，Agent 按已签 `source_repo`、`tag` 与 `asset_name` 重建地址比对后，以独立匿名客户端逐跳校验公网 DNS 与固定解析地址下载（支持服务器配置的独立 HTTPS 下载加速镜像，拒绝面板同源、非公网目标、URL 凭据与非法重定向；设备凭据绝不发往 GitHub 或镜像）。面板不再提供任何二进制下载路由（旧 `/api/bootstrap/{version}/{arch}` 与设备制品下载接口均已关闭），仅通过认证通道向在线一体化 Agent 下发声明式服务配置与诊断任务指令。
