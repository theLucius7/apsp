# 原生 TCP 固定源码与签名制品：独立验收

> **现行架构对齐说明（2026-11-18，[ADR 0140](../adr/0140-domain-consolidation.md)、[ADR 0141](../adr/0141-self-contained-agent-and-desired-service-config.md)）**：`crates/tcp-probe` 现已直接编入单文件一体化 `apsp-agent` 作为内置 `apsp-agent tcp-probe` 子命令交付（构建期以 `APSP_NATIVE_TCP_SOURCE_COMMIT` 固化来源提交），正式发布工作流（`.github/workflows/release.yml`）只产出各平台自包含 `raw` `apsp-agent` 二进制、不可变安装器、签名清单与配对对应源码包，**不再单独构建、组装或经面板分发 `tcpquality` 与 `sing-box` 独立安装制品**。面板 `POST /api/system/agent-releases/import` 仅收录单文件 `raw` `agent` 条目。下文「当前证据」起完整保留 2026-10-01 独立包阶段的源码、Bookworm 启动修复与 bundled musl 1.2.5 版权复核证据；现行可复制构建与组装命令以本节下方为准。包含 Linux 四组件内嵌闭环与原生 `tcp-probe` 的新版一体化 Agent 正式签名 Release 及 P2 TCP 实机签收尚未执行。

## 现行一体化 Agent 构建与发布组装

在干净且具备目标平台内嵌输入（同 ABI `sing-box` `1.14.4-apsp.2`、`-r22` `nodequality`、`ipquality` 离线 rootfs 包与 `network-workbench` `1.0.0` 自包含离线包，准备入口见 `tools/prepare-agent-linux-inputs.py` 与 `.github/workflows/release.yml`）的 Linux 构建环境中，编译内置 `tcp-probe` 的自包含 `raw` Agent 并组装候选发布目录：

```sh
# 1. 构建内嵌四组件闭环与原生 tcp-probe 子命令的 Linux 静态 musl 自包含 Agent（GNU 动态目标改用 python3 tools/build-agent.py）
export APSP_RELEASE_EMBED_REQUIRED=1
export APSP_NATIVE_TCP_SOURCE_COMMIT=<完整40位源码SHA>
bash tools/build-agent.sh amd64 <内嵌输入与产物根目录>

# 2. 渲染不可变安装器并组装自包含 Release（不传已废弃独立分发的 --runtime-version / --tcp-probe-version）
python3 tools/release.py render-installer \
  --template deploy/install.sh.tmpl \
  --agent-unit deploy/apsp-agent.service \
  --runtime-unit deploy/apsp-singbox@.service \
  --output /tmp/apsp-install.sh

python3 tools/release.py assemble \
  --source <内嵌输入与产物根目录> \
  --output <新输出目录> \
  --agent-version <Agent版本> \
  --tag agent-v<Agent版本> \
  --installer /tmp/apsp-install.sh \
  --target amd64 --target arm64 \
  --target linux-gnu-amd64 --target linux-gnu-arm64 \
  --require-paired-sources
```

离线正式签名与发布核验继续沿用 [ADR 0017](../adr/0017-signed-release-artifacts.md) 与 [签名发布说明](../release.md)（测试夹具公钥 `TEST_ONLY` 禁止用于正式安装或发布）。`tools/release.py` 中保留的历史兼容参数仅服务旧归档回归测试，不能用于现行产品发布或面板导入。

## 历史独立制品阶段接受标准（2026-10-01 记录背景）

以下条目对应 2026-10-01 原生引擎 PR #68 合入时独立 `tcpquality` 归档与五辅助文件阶段的验收标准（现行交付已转为上文一体化 `apsp-agent` 内置子命令与配对源码包）：

- 合法历史 commit 可构建；main/ref、不存在对象、脏仓库、unknown pin 均拒绝。旧固定配方实际从归档执行，当前配方修改不能替换它。
- 实际构建使用 `--locked`、明确 musl 目标、固定编译期 SHA；二进制静态 ELF 与本机架构一致，`version`/`help`/`build-info` 可执行，`help`/`build-info` 不打开诊断工作目录。
- 当轮独立包签名精确覆盖二进制和五辅助文件；改动任一文件、缺源码、unknown 来源、未锁定依赖或锁文件来源冲突均拒绝。
- 两个架构在独立 CI 矩阵构建/运行，错误架构 ELF fixture 拒绝；正式 trust root 加载公开 `TEST_ONLY` 根必须拒绝。

## 当前证据

原始 10 项及第五许可文件加入后的 11 项 Python 来源/签名契约通过；旧 Release 32 项和 build-script 5 项通过。真实锁定原文收集与验证为 35 个依赖、2,006,844 字节，包含 Unicode 组合许可、Rust 标准库与原生库、系统 musl 通知。最终引擎基线为 `0a6b8499ca663be0177ee3edaf70b89f24b3c854`。叠加最终基线后的脚本、Rust fmt/clippy/workspace、真实 amd64 musl 构建及双架构 CI 待记录；不得把引擎既有测试结果视为本项完成验收。

此项不运行 NodeQuality、网络质量探测或真实 systemd 诊断，资源预算、取消和章节仍由既有框架测试承担。

## main 整合审查阶段

正常保留作者 `00cfa9e` 并合入 main `2c3c1e5`（业务插件归位）。实际 [check job 110129554033](https://github.com/theLucius7/sinan/actions/runs/36786654457/job/110129554033) 在 Clippy 阶段因 `DiagnosticWorker::new` 未使用的辅助文件变量失败；本地删除多余读取，准备阶段仍只读取一次集合，并同时用于下载描述、签名集合验证及启动前重验，没有压制 warning。固定快照配方、locked 原生 musl 目标、完整五文件来源校验和旧三模块默认契约保留。

本阶段 Python 来源/签名10项通过、旧Release32项中28通过/4既有root条件跳过、build-script5项通过，core门禁/六项行为、直接rustfmt、actionlint与差异检查通过。原生 bundle 本机条件不满足，1项明确跳过；作者PR的[双架构原生制品CI](https://github.com/theLucius7/sinan/actions/runs/36786654508)实际成功，但不替代本地补修后最终HEAD的Rust/Clippy验证。共享Cargo槽仍由前项使用，本阶段没有运行Cargo、实际musl构建或启动服务，后续须在正式main包含#68补修后集中验证；未持有正式签名私钥，也未发布Release或登记插件。

继续正常保留作者 `7716fcf`、共用诊断主线 `e3a41ed` 与正式引擎主线 `2574a84`。新增第三方原文为第五辅助文件（共六文件），签名/精确集合与 Agent 缓存、启动前重验夹具同步保留；引擎 journal/engine 与正式 #68 原字节一致，main 只增加 build-info 入口，UID、DNS 家族选择、有界 stderr 和各自回归保留。新版 Python 来源/签名/原文11项通过，直接rustfmt、core门禁及actionlint通过；作者该版[双架构原生CI](https://github.com/theLucius7/sinan/actions/runs/36787615446)实际成功。本地仍未启动Cargo，最终合入后续主线再执行Rust专项及Clippy，不能把前版或作者CI转记为本地最终补修已验。

## 最终主线与专项

正常保留最新作者 `bf56d5e` 与正式主线 `fb79388`（#55/#60/#68/#70），源码验证基线为 `644e785`。作者最终删除了与本地修复相同的多余读取；准备阶段仍使用同一次辅助文件集合，旧适配器默认空集合、签名前检查、下载、缓存重验和启动前重验全部保留。发布仅在显式选入 TCP 时要求它的双架构，同时保持旧三模块完整性。刷新九个变更 Rust/manifest 文件的时间后，独占共享编译槽集中验证：**90 项通过 / 0 失败 / 0 忽略**（Agent 制品14、release6、wire/预算12、诊断/取消/预算与来源40、SDK1、TCP13库与4真实CLI）。覆盖五辅助文件实际下载及逐文件缓存篡改、未声明/缺失/prepare后篡改不启动、旧签名适配器、build-info、UID与DNS回归、有界阻塞stderr、实际停止及零应用payload。workspace全targets Clippy（warnings为错误）、fmt、core门禁及六项行为、actionlint和差异检查通过。

最终源码下 Python 来源/签名/原文/选入契约12项与模拟GitHub发布22项全部通过；仅合成签名和回环夹具，没有真实账户或发布动作。#70证明中11个前端/门禁文件哈希与正式主线完全一致，保留暂停完整入口、日常入口、`index-OopWuqxH.js`、中性活动桥和r5，没有重复无交集浏览器或PG全量。本轮macOS未重新运行完整workspace测试、Linux原生musl构建或真实systemd/生产节点；最终HEAD的双架构制品与正式主线CI继续单独核对，旧作者native CI不能代替此次最终整合证据。没有登记新插件、签正式Release或发布制品。

## Debian 12 启动修复与固定制品验收（独立后续 PR）

#69 已合并；其 main 整合版本没有包含作者在 Bookworm 上发现的 musl 启动修复。旧 musl-gcc linker 产生的静态 PIE 在 --version 就以 SIGSEGV 退出，构建器正确拒绝该产物。相同最小 hello 在该 wrapper 下返回 -11，使用 native cc 与 -Clink-self-contained=yes 返回0。后续修复保留静态 PIE，使用 Rust 自带 musl/CRT；不改 Agent 的既有构建配方。增加独立 Debian12 真构建/执行/签名 CI，Ubuntu 原生 amd64/arm64 验收继续保留。CLI help 只保留稳定功能边界，移除“尚未接入”临时说明。

已公开且实际验证的永久工具源为 [5e843f0fd9532abe9b7b9a052ef77b45abcfa675](https://github.com/theLucius7/sinan/commit/5e843f0fd9532abe9b7b9a052ef77b45abcfa675)，外部版本为 0.3.0-5e843f0fd9532abe9b7b9a052ef77b45abcfa675-r1。制品 binary SHA256 为 dd4b804f290e920637c3164fed0bd6f1b078ca90d26e34d33d2d8eb849beeeba。--version 精确返回 sinan-tcp-probe 0.3.0；--build-info 明确返回同一 source commit、theLucius7/sinan 和 0.3.0。源码 archive、recipe、Cargo.lock、第三方原文及编译输入一致，后续仅合并其他模块或更新验收文档不重定该固定源。

2026-10-01 独占受限 Debian12 构建容器（1536MiB memory/swap、2CPU、pids512、OOMScoreAdjust500）实际记录：

- 源 500a13d990bf21c07a4e111d751693a0803473c1：fmt/core门禁、全targets Clippy（-D warnings）、locked workspace Rust **347通过/0失败/9既有条件忽略**；Python来源/签名12项、旧Release32项、模拟发布22项全部通过；真实 amd64 musl 构建与完整五辅助文件 TEST_ONLY 签名 bundle通过。
- 最终源 5e843f0fd9532abe9b7b9a052ef77b45abcfa675 只进一步改 help 文案，再次 fmt、全targets Clippy、TCP **13库+4真实CLI**、真实 musl 构建/CLI、完整签名 bundle **1项**通过，容器 exit0、OOMKilled=false。
- 永久制品的五辅助文件为 build-info.json、LICENSE、source.tar.gz、Cargo.lock、THIRD_PARTY_NOTICES.txt；完整签名覆盖二进制和全部辅助文件。35个锁定依赖的许可原文及 Rust/musl 通知保留，原文库存约2MiB。
- 最终证据、制品、build-info、日志、SHA256SUMS 与源码标记保存于受控构建机的 evidence/tcp-artifacts-final-5e843f0；原生 binary 与来自明确源500的 core测试 binary 单独保存于 binaries/tcp-artifacts-head。

上述 workspace 全量来自明确的500源码，不替代后续 main 新增功能的验收；本后续 PR 的最新 GitHub 两架构及 Debian12 任务仍须按实际状态核对。没有正式签名/发布，没有执行外部探测或真实 systemd 诊断。所有签名夹具为 TEST_ONLY。

首次后续CI（464f0ff）Ubuntu原生amd64/arm64制品均通过，新增Bookworm任务在构建前Git目录所有权检查返回128。checkout只设置临时HOME的safe.directory，后续容器步骤需对实际GITHUB_WORKSPACE设置精确例外；受限容器复现了dubious ownership，精确设置/workspace后同一固定对象解析通过。工作流只允许本任务检出的目录，不设置全局通配符、不跳过源码对象检查；补修后最新CI另行核对，永久工具源5e未变化。

整合复核保留作者 `b536476`：该源码的[三项原生制品 CI](https://github.com/theLucius7/sinan/actions/runs/36792542789)全部通过，含真实 Bookworm 启动、来源校验和 TEST_ONLY 完整签名。macOS 专项首次暴露旧回环夹具的读取竞态：非阻塞 listener 接受的 stream 未显式切回阻塞，读 EOF 返回 WouldBlock 导致 server 提前结束；夹具现在显式设置 blocking，仍保留一秒读取限制和零应用数据断言。并行库测试另遇一秒初始发布预算超时；未放宽引擎或测试预算，按单线程重验后 **13库+4真实CLI 全部通过/0忽略**。TCP 全targets Clippy、workspace fmt、core门禁、actionlint通过，Python来源12项、模拟发布22项通过，旧Release32项中28通过/4既有root条件跳过。没有在本机重跑 Linux musl 构建或完整 workspace Rust；最终 HEAD 的 GitHub CI 继续单独核对。

已拉取永久 `5e843f0` Git 对象并逐字核对：生产 TCP、构建配方与来源/原文验证器相同；当前 main 的 Cargo.lock 另含 #71 适配器包，不把两份整体锁文件称为相同。永久制品仍从该对象自己的完整归档、配方和 Cargo.lock 构建，测试夹具修复不替换既有固定制品。

## Issue #75：实际 bundled musl 版权库存（独立 PR）

#72 已合并，不修改已关闭分支。当前实际 Rust工具链使用bundled musl1.2.5，旧库存将系统1.2.3标为linker库并不能证明真实来源。补齐官方1.2.5完整COPYRIGHT、rustc完整commit对应的musl配方原文（仅不可执行证明文本）和固定来源摘要；系统musl通知明确为构建工具。没有引入上游TcpQuality代码/targets/rootfs或下载执行链。

构建必须在Cargo前拒绝未知rustc commit、缺失/篡改库存原文；制品验证必须检查原文与source归档一致、标准库与binary rustc一致，重签缺失或伪造bundled通知仍拒绝。新增真实minisign重签夹具与原文/工具链行为测试；原生重新构建、最新source pin和最终CI分别记录。五辅助文件和CLI/报告ABI保持不变。源5e既有启动/签名证据不代表此通知补齐完成。

官方musl1.2.5归档SHA256 a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4，COPYRIGHT原文SHA256 f9bc4423732350eb0b3f7ed7e91d530298476f8fec0c6c427a1c04ade22655af，固定Rust recipe SHA256 2f218a2dc7b7e73509212bfd4319ebddc2ddac7c651fca142c2b29bd7ea0aa38。许可证构建阶段零联网，原文随source与第五辅助文件共同签名。

最终公开并验证的修复后工具源为 [b562effcd90f8ae319665fb4ead1807b770ed4d5](https://github.com/theLucius7/sinan/commit/b562effcd90f8ae319665fb4ead1807b770ed4d5)，外部版本0.3.0-b562effcd90f8ae319665fb4ead1807b770ed4d5-r1。2026-10-01受限Bookworm容器（1536MiB/2CPU/pids512/OOM500）实际通过fmt/core、来源与真实重签15项、旧Release32项、模拟发布22项、固定源码archive内配方locked musl构建、实际ELF/version/build-info及完整五辅助文件TEST_ONLY签名bundle1项；exit0/OOMKilled=false。35个锁定Cargo依赖之外，库存有Rust标准库原文、systemmusl build tooling1.2.3、Rust bundled musl libc1.2.5三组，后二者来源不混用。

修复后原生binary SHA256 e493d09511929f4479a3a18496f4a034e01faac5628f3db204b2d1c6f89cd85f，build-info的source commit与公开对象精确一致。日志/制品/原文/SOURCE/SHA256SUMS保存在evidence/tcp-musl-notices-b562eff，binary单独保存binaries/tcp-musl-notices-head。此项仅变Python配方/库存与测试，没有Rust实现变化，未重复无交集完整workspace；新PR的两架构、Bookworm和标准CI继续按最终HEAD单独核对。无正式签名/发布/外部探测。源5e已完成的证据保留，但分发候选改为b562。

制品workflow_dispatch支持source_commit指定已存在的完整固定对象，保持Git archive/缺对象拒绝与归档配方契约，可在新主线验证历史固定工具源的amd64/arm64及Bookworm，而不把当前head误作永久工具pin。只产生TEST_ONLY验收产物，不发布Release。新PR发布后将以b562固定源触发双架构原生验收，最终结果单独记录。

## bundled musl 独立复核与历史制品兼容

本轮直接读取官方 musl1.2.5 发布归档，确认归档 SHA256 `a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4`；其中 193 行 COPYRIGHT 与仓库保存文本逐字一致，SHA256 `f9bc4423732350eb0b3f7ed7e91d530298476f8fec0c6c427a1c04ade22655af`。官方 Rust commit `48a229ceaefd4985c50990b14116b6d856af0985` 的 `src/version` 为 1.98.1；97 行 musl 配方逐字匹配，SHA256 `2f218a2dc7b7e73509212bfd4319ebddc2ddac7c651fca142c2b29bd7ea0aa38`。除库存简述中的两项 2025 iconv 补丁，完整配方还应用 CVE-2026-6042 与 CVE-2026-40200；官方两份补丁未改版权原文。来源链接见 [ADR 0032](../adr/0032-native-tcp-artifacts.md)。不将系统 musl1.2.3 工具通知称作实际 self-contained libc。

[原生 CI 36794789931](https://github.com/theLucius7/sinan/actions/runs/36794789931) 的工作流提交为 `08c9de2`，显式源码 pin 为 `b562effcd90f8ae319665fb4ead1807b770ed4d5`。Ubuntu amd64、arm64 与 Debian12 三个 job 全部成功，各自完成真实本机启动、静态 ELF、来源/签名 15 项和原生 bundle 1 项。该结果证明固定 b562 配方及官方编译器产物，不将旧配方记作包含本轮新的重复字段检查。

独立复核捕获旧验证器接受同时包含已知/未知或重复已知 `commit-hash` 的重签包：先更新 notices、build-info、外部全部摘要并以公开 TEST_ONLY key 真实重签，三个歧义输入仍被接受。当前验证器改为只接受一个精确字段；来源/签名 17 项全部通过，覆盖重新签名后仍拒绝及 Cargo 调用前拒绝。另下载上述 CI 的两个真实原包，只读验证其静态 ELF、五个辅助文件和内嵌固定源；构建配方、旧验证器、收集器及三份库存与 Git b562 原字节一致。两个原包均通过当前严格验证器，并可重新组装公开 TEST_ONLY 签名 bundle 后通过现有 release 验证；未执行下载二进制、未在本机重建 Linux 或签正式 release。b562 原库存、版权文本、版本与制品路径均保持不变。

本轮旧 release 32 项中 28 通过、4 既有 root 条件跳过，模拟发布 22 项及 core 分层 6 项通过；合计 73 通过、0 失败、4 跳过。Python 语法、core 门禁、actionlint、文档链接及差异检查通过。Rust 源码无变化，未运行 Cargo/native 构建，也不把已完成固定源 CI 当作新验证器提交的 CI；最终整合提交仍需单独核对。
