# ADR 0032：原生 TCP 工具的固定源码与完整签名制品

状态：已接受；制品入口独立于插件登记、执行参数和界面。

## 约束

用户要求固定提交、锁定依赖、校验与签名，不执行在线 main。上游 TcpQuality 尚无分发授权，采用本仓库 AGPL-3.0-only 的原生连接工具，不复制或运行上游脚本、rootfs、targets。工具只建立 TCP 连接；此项不增加探测、上传或宿主修改能力。

## 决策

工具内版本为 `apsp-tcp-probe 0.3.0`。制品角色为 `tcpquality`，二进制名为 `apsp-tcp-probe`；外部版本为 `0.3.0-<40 位小写 Git SHA>-r1`，每个源码变化产生不同不可变路径。源码提交必须是仓库中存在的 commit 对象，不接受分支、标签、unknown 或空值。它可为当前发布树的祖先，避免后续插件常量登记造成循环。

构建器先验证干净仓库，再 `git archive` 指定对象到私有临时目录，执行该归档内的构建配方，以 `Cargo.lock --locked` 构建本机架构 musl 目标。不使用当前工作树替换固定源码。构建前后均验证归档、源码文件和仓库，编译环境嵌入固定 SHA；提供非法编译期 SHA 会编译失败，未提供 SHA 的开发构建可运行但不能打包。

制品包含二进制和五个辅助文件：`build-info.json`、`LICENSE`、`source.tar.gz`、`Cargo.lock`、`THIRD_PARTY_NOTICES.txt`。build-info 记录版本、完整 SHA、目标、rustc、locked 标志和各文件 SHA256。源归档保留 Git 的完整 commit 注释，并必须包含工具及构建配方；独立锁文件、许可证必须与归档逐字一致。编译器验证原生架构的静态 ELF、无动态解释器或依赖，并实际执行 `--version`、`--help` 和无网络的 `--build-info`。不在错误架构上执行。

沿用 release.json、SHA256SUMS 与 minisign 契约，辅助文件全部记录大小与 SHA256。SDK 新增默认空的 `DiagnosticAdapter::auxiliary_files()`；core 的签名检查和下载描述使用同一次读取的集合，保持精确集合及缓存再校验，旧适配器行为不变。

默认旧三模块 assemble 及发布流程保持原行为。新制品仅显式 `--tcp-probe-version` 加入。CI 在 amd64/arm64 原生 runner 上构建，仅使用公开 TEST_ONLY 签名夹具，拒绝将其作为正式信任根。本项不持有正式私钥、不创建或发布 release，生产发布接入仍须由后续登记项选择已验收的源码版本。

## 验收

见 [独立验收](../acceptance/native-tcp-artifacts.md)。制品协议同时拒绝签名后篡改，以及重新签名但来源字段缺失、unknown、未锁依赖、锁文件与源码不一致的包；签名不能替代来源完整性。

## 静态依赖的原文通知

Apsp 根 AGPL 许可证不能替代第三方通知。构建器以 --locked metadata 加上只针对原生工具的 normal/build cargo tree 选择实际依赖；每个 registry package 的原始 .crate 必须匹配 Cargo.lock checksum，安装源文件也须逐字匹配该归档。收集所有 LICENSE/NOTICE/COPYRIGHT/COPYING 原文，保留 SPDX 声明、版本和 source/checksum，Unicode 组合许可另要求 Unicode 原文。缺原始 crate 或许可文本拒绝打包，不从在线 main 补齐。

Rust 的 COPYRIGHT-library.html 与 license 原文库存，以及本机 musl 包版权文件随第五个辅助文件保留，记录实际工具链版本；这同时覆盖静态标准库及原生库。THIRD_PARTY_NOTICES.txt 使用可读 JSON 文本，manifest 与 build-info 分别覆盖其大小/哈希；验证器检查依赖身份/checksum与锁文件、原文集合及本机目标。

## Debian 12 本机构建兼容

实际 Bookworm 验收发现 musl-gcc wrapper 与静态 PIE 启动不兼容，构建后 --version 即 SIGSEGV，构建器拒绝产物。相同最小 Rust hello 在 wrapper 下 -11，使用 native cc + -Clink-self-contained=yes 返回0；选择 Rust 自带 musl/CRT，保留静态 PIE，而非混用系统启动对象。依据 [Rust issue 95926](https://github.com/rust-lang/rust/issues/95926) 和 [rustc 官方 self-contained 文档](https://doc.rust-lang.org/rustc/codegen-options/index.html#link-self-contained)；独立 Debian12 CI 与 Ubuntu amd64/arm64 CI 各执行真实启动与签名校验。未改 Agent 既有构建脚本，该模块配方需独立评估。

## Rust 实际 bundled musl 库存

Self-contained 链接必须记录 Rust 自带的 libc，而不是以系统 musl 版本替代。该工具当前支持已核实 rustc commit 48a229ceaefd4985c50990b14116b6d856af0985。其[官方固定配方](https://github.com/rust-lang/rust/blob/48a229ceaefd4985c50990b14116b6d856af0985/src/ci/docker/scripts/musl.sh)使用 musl1.2.5 及两项 CVE-2025-26519 iconv补丁。官方 release 归档 SHA256 为 a9a118bbe84d8764da0ea0d28b3ab3fae8477fc7e4085d90102b8596fc7c75e4；完整 COPYRIGHT 原文、不可执行的 Rust 配方文本、来源与摘要库存纳入固定 Git 源码。构建不下载许可文件，也不执行该证明配方。

第五辅助文件分别保存 Rust标准库原文、实际 Rust bundled musl1.2.5 原文及其精确来源证明、系统musl构建工具通知。系统1.2.3通知不再标作实际静态链接 libc。构建在 Cargo metadata/build 前核对 rustc commit 与本地原文摘要；未审计的新工具链明确拒绝，须以新固定源完成库存更新。签名验证重新对比库存、source.tar.gz 中原文与recipe、binary build-info 的实际 rustc，缺失/篡改/重签不一致均拒绝。五辅助文件集合和三字段CLI身份契约不变。

此修复对应 Issue #75，新工具源须公开完整 Git 对象并经过原生Bookworm实际构建/运行与TEST_ONLY完整签名验收；旧5e只保留其既有测试记录，不作为修复后分发候选。未签正式Release。

完整 97 行官方配方还应用 [CVE-2026-6042](https://github.com/rust-lang/rust/blob/48a229ceaefd4985c50990b14116b6d856af0985/src/ci/docker/scripts/musl-cve-2026-6042.diff) 和 [CVE-2026-40200](https://github.com/rust-lang/rust/blob/48a229ceaefd4985c50990b14116b6d856af0985/src/ci/docker/scripts/musl-cve-2026-40200.diff) 两份外部补丁；两项 2025 iconv 补丁与这两份 2026 补丁均不修改 COPYRIGHT。库存 `patches` 字段只描述 2025 iconv 子集，保留其已验证原字节，不能以简述替代完整配方。

已验收的永久工具源 `b562effcd90f8ae319665fb4ead1807b770ed4d5` 保留自己的归档配方。该固定对象内的 rustc 检查采用旧的匹配表达式；本轮当前源码中的原文收集器和 release 验证器另外要求恰好一个 `commit-hash` 字段，并精确等于已核实的官方 commit，拒绝未知或重复字段，即使其余摘要和公开 TEST_ONLY 签名均被重新生成。当前验证器接受 b562 的合法历史产物；不修改该固定对象、版本路径或已存在制品。
