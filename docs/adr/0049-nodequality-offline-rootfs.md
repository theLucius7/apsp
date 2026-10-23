# ADR 0049：NodeQuality 的离线 Debian 12 rootfs 准备链

状态：实现准备接口，尚未完成真实制品与双架构验收。完整验机门禁保持关闭。
日期：2026-10-01。

## 问题与决定

原固定 NodeQuality 链保留完整上游源码和许可材料，但原 BenchOS 是缺少完整构建来源的独立归档。其现有两架构归档都超过 Apsp 外层制品的 256 MiB 总预算，不能直接换一个下载地址后称为受控离线环境。当前尚未提供适用于这条工具链的 Geekbench 授权证明，其他第三方工具的固定来源、再分发和上传行为仍有未闭合项。

新增 `tools/nodequality-rootfs-build.py`，只接受显式输入锁和已经取得的本地普通文件。工具不下载、执行在线脚本、自动批准 builder 镜像或启用完整验机。分别执行准备、原生构建和导出；制品打包重新核验准备目录，并将最终归档与清单绑定。主线 r19 默认入口和已有完整源码不因此被替换，r18 历史制品继续可读。r20 是显式选择的准备制品，其包装器仍拒绝完整验机。

这次交付的是可审阅的来源准备、原生构建配方与安全导出代码。不存在已解决的真实 snapshot/package lock、已审批 builder 镜像、已经产出的 rootfs 或成功复建记录。测试代码中的假签名状态、输入锁和普通文件都是自有夹具，不能当成 Debian 签名或真实构建证据。

## Debian 信任根和历史来源

输入应先从官方 [Debian Snapshot](https://snapshot.debian.org/) 的实际导入列表选定时间，分别锁定 `debian` 与 `debian-security`。请求一个不存在的时间可能返回更早的快照，因此时间格式通过检查并不证明该导入存在；维护者必须保存实际导入记录，并审核锁中的固定时间和原始响应。Snapshot 网站的文件索引身份也不能代替 Debian 签名。

Bookworm 的主自动签名主指纹为 `B8B80B5B623EAB6AD8775C45B7C5D7D6350947F8`，稳定发布主指纹为 `4D64FEC119C2029067D6E791F8D2585B8783D481`，安全仓库主指纹为 `05AB90340C0C5E797F44A8C8254CF3B5AEC0A8F0`。这些来自 [Debian FTP Master 的官方说明](https://ftp-master.debian.org/keys.html)；该页面本身要求另行验证密钥。维护者需通过独立可信渠道核对指纹与 keyring 的取得来源，不能用待验证仓库的任意密钥自行建立信任。

准备阶段按 `InRelease 签名 → Release 中的索引 SHA256 → Packages/Sources 中的包和对应源码 SHA256 → 实际本地文件` 验证。`apt-secure` 的认证对象是仓库链，并不等于每个 `.deb` 都有独立的包签名。[apt-secure(8)](https://manpages.debian.org/bookworm/apt/apt-secure.8.en.html)

调用固定字节的 `/usr/bin/gpgv`，使用明确的 keyring、私有 homedir、机器状态输出和真实退出码。只接收上述主指纹、SHA256 或更强摘要；拒绝错误、过期、撤销及快照时间之后的签名。`gpgv` 将指定 keyring 中的密钥视作可信，不能把它的成功退出误写为已独立完成密钥信任和撤销审查。[gpgv(1)](https://manpages.debian.org/bookworm/gpgv/gpgv.1.en.html)

历史快照只在每条明确的本地源上使用 `check-valid-until=no`。继续保留 `Signed-By` 的完整指纹限定、签名验证与时间检查；不允许 `trusted=yes`、全局关闭 `Check-Date`、不安全仓库降级或绕过过期签名。`Signed-By` 允许明确 keyring 路径与主指纹限定。[sources.list(5)](https://manpages.debian.org/bookworm/apt/sources.list.5.en.html)

## 固定输入契约

输入锁是拒绝重复键的 JSON，schema 1，最多 8 MiB，不提供含虚构摘要的可直接使用示例。一个锁只描述 `amd64` 或 `arm64`；两架构必须分别解决完整依赖闭包。

| 字段 | 精确材料 |
| --- | --- |
| `schema, arch, source_epoch` | schema 1、原生架构、固定正整数构建时间 |
| `builder` | `image_sha256, arch, tools`；tools 恰为 gpgv、mmdebstrap、unshare，每项含 name/path/version/sha256/size |
| `keyring` | 本地 `blob, sha256, size`，必须与独立批准材料匹配 |
| `repositories` | `id, archive, timestamp, suite, inrelease, indices`；主仓库和安全仓库必需，updates 可选，同一 archive 使用一个固定时间 |
| `inrelease` | 原始 `blob, sha256, size`，不重新格式化签名内容 |
| `indices` | 固定 main 的 native Packages 与 Sources gzip/xz；每项 `kind, path, blob, sha256, size` |
| `packages` | 全部已解决二进制包：`repository, name, version, architecture, filename, blob, sha256, size, source_name, source_version` |
| `sources` | 每个二进制包对应源码的 `repository, name, version, directory, files`；files 恰包含 Sources 中所有 SHA256 条目，每项 `name, blob, sha256, size`，不得遗漏 `.dsc`、原始源码或 Debian 补丁材料 |

只接受 Debian 12 `main` 的 native/all 二进制包和完整对应源码。索引压缩文件及其展开最多各 256 MiB，单 `.deb` 最多 256 MiB，单源码文件最多 1 GiB；所有输入数量也有上限。源缓存可能比最终 rootfs 大，必须在独立 builder 上准备磁盘空间。流程不接受缓存路径穿越、符号链接父目录、FIFO、设备或冲突的 blob 身份。

`--approved-builder-image-sha256` 必须由调用者独立提供，并与锁完全一致。模块检查三个关键工具的实际字节，但不从内部证明正在运行的虚拟机等于该镜像摘要。Perl/Python/APT、动态库、mmdebstrap file-mirror hook、内核及构建环境属于外部镜像审批范围。外层环境管理器必须给出实际镜像身份、构建配置与全部构建工具来源/许可；CLI 中重复一个摘要字符串不构成镜像证明。当前没有这些真实审批材料。

## 接口与目录

`prepare --lock --cache --output --approved-builder-image-sha256` 对已取得输入执行认证，保存原始锁与全部源材料，构造只有已锁包的普通本地镜像。输出为：

- `inputs-lock.json`：原始输入锁。
- `input-cache/`：已认证 keyring、InRelease、索引、二进制包和完整对应源码。
- `mirrors/<repository>/`：原始 InRelease、索引、已锁包与固定 keyring；不生成自签替代索引。
- `source-inventory.json`：架构、全部包、对应源码文件的版本/摘要/大小/固定 URL、工具映射与未完成能力。
- `prepared.json`：输入锁、源库存、批准镜像摘要、实际签名主指纹，且 `full_ready=false`、`reproducibility_verified=false`。

`verify_prepared(prepared_dir, approved_builder_image_sha256)` 重新执行完整来源认证，并逐文件比对派生 mirror，拒绝额外包、缺失文件和修改的 keyring。返回 dict 字段固定为 `schema, arch, lock, directory, inputs_lock_sha256, source_inventory_sha256, source_inventory, build_tool_sha256, approved_builder_image_sha256`。`build_tool_sha256` 是当前构建模块实际字节摘要；不能拿旧 helper 摘要认领新的导出。

`build --prepared --output --approved-builder-image-sha256` 只在原生架构、root、Debian 12 的独立 Linux builder 执行。使用新的 mount/network/PID/UTS/IPC namespace，网络隔离，只给 mmdebstrap 本地 `file://` 源和完整固定版本清单；不使用 chrootless 模式、QEMU 替代原生性能环境或第三方 hooks。file-mirror hook 将镜像中的固定 keyring 置于内外相同可达路径，构建结束清理映射。该 hook 的行为、镜像审批和权限仍须实际验收。[mmdebstrap(1)](https://manpages.debian.org/bookworm/mmdebstrap/mmdebstrap.1.en.html)

构建保留版权、共同许可证和包状态，移除手册与无关 locale。设置固定 `SOURCE_DATE_EPOCH`，禁用推荐包和重试；安装后的包身份必须恰等于已认证闭包。构建子进程有整体一小时 deadline 与 8 MiB 输出限额，异常终止并回收所启动的进程组。准备及导出各有十分钟整体 deadline，单次 GPG 验证最多三十秒及 64 KiB 状态输出。不能把时间上限解释为已实测所有异常内核或存储场景均可即时终止。

构建后先读取当前 Linux mount inventory，拒绝树内残留挂载，再清理新树的 proc/sys/dev、日志、APT 缓存和主机身份。清理只作用于本次新建的私有目录，不能把宿主设备节点带入制品。最后写 `build-plan.json`、`build.log`、`build-receipt.json` 与 `tree/`。该配方尚未实际运行，包可达性、权限、hook、设备目录清理与两架构大小均是后续验收项。

CLI 的 TERM/HUP handler 将中断转换为 `SystemExit(128+signal)`，让资源清理执行。进程启动及输出目录创建的短窗口暂缓 INT/TERM/HUP，先登记句柄，再交付待处理信号。selector 创建/注册、读取和结束均在已启动子进程的同一个 finally 范围内；清理逐项关闭、杀进程组并 wait/reap，清理异常不能覆盖原始异常。子进程会继承启动时的信号 mask，因此该收集器的终止保证采用显式 SIGKILL，不能宣称对子进程进行了 TERM/HUP 协作退出。

失败构建目录在删除前也重新读取挂载清单：检测到残留挂载或清单不能读取时保留目录、报告清理失败，并给原始错误附加 note，不能在拒绝清理后继续递归删除挂载内容。prepare/export 不执行挂载命令，只清理自己登记的私有输出目录。工厂输入缓存和构建目录的容量需求属于独立 builder 的容量规划；设备制品的 256 MiB 总预算不代表工厂磁盘消耗也被限制到该值。

## 开源工具库存与完整能力

两架构的基础命令映射相同，完整二进制版本及依赖由各自签名索引决定：

| 用途 | 命令/包 |
| --- | --- |
| 脚本与格式 | bash；base64/head/wc/date/timeout/numfmt → coreutils；grep、sed、awk → gawk、findutils、tar、gzip、xz-utils、zip、jq、bc |
| 查询与密码学 | curl、wget、openssl、nc → netcat-openbsd、dig → bind9-dnsutils、update-ca-certificates → ca-certificates |
| 系统与硬件 | dmidecode、sensors → lm-sensors、lspci → pciutils、lscpu → util-linux、smartctl → smartmontools、fio、sysbench、free → procps、clinfo |
| 网络与图表 | mtr → mtr-tiny、iperf3、stun → stun-client、convert → imagemagick |

Bookworm 的 `stun-client` 在 amd64 与 arm64 提供客户端 `/usr/bin/stun`；不能把同名虚拟包/服务器包误当成客户端。[amd64 文件清单](https://packages.debian.org/bookworm/amd64/stun-client/filelist)、[arm64 文件清单](https://packages.debian.org/bookworm/arm64/stun-client/filelist)

以上是已规定的库存范围，不是已取得、安装或运行成功的证明。导出必须找到对应可执行普通文件及每个安装包的版权文件；命令探测、动态库闭包与实际功能仍需独立验收。`clinfo` 也不表示 rootfs 已包含 GPU 驱动。

以下能力继续明确待完成，不能以有限自写 benchmark 宣称原完整目标已满足：

- nexttrace：固定 GPL 源码、Go 工具链及模块闭包、两架构离线构建、许可库存。
- Geekbench：适用的授权证明尚未提供，离线、不上传模式及再分发权未获确认；不采购、自动接受条款或包入未授权二进制。
- Ookla speedtest：固定来源、再分发/条款和真实上传行为仍未审批。
- curl-impersonate：原能力保留在 canonical 记录，实际 provider 访问需遵循原生身份边界。
- nvidia-smi：可选厂商驱动及对应授权未提供。

## 导出、外层预算与维护者签名

`export --tree --prepared --output --approved-builder-image-sha256 --outer-reserve-bytes` 重新验证准备目录，要求树与本模块的 build receipt 匹配。普通文件在归档读取期间再次核验摘要和字节数；拒绝已有来源元数据、特殊文件、危险路径及不符合格式的成员。

导出将 Debian 合法链接链展开为指向清单内普通文件/目录的相对链接；硬链接作为各自普通文件保存，不保留宿主 owner、setuid/setgid、capabilities 或其他 xattr。目录 mode 仅 0700/0755，文件仅 0600/0644/0755，符号链接 header mode 0777、size 0。归档是纯 USTAR 与 gzip，固定排序、时间、uid/gid 0、空 uname/gname、gzip mtime 0；不以 PAX/GNU 扩展绕过长路径限制。

归档不含 BenchOs 外层目录；输出 `rootfs.tar.gz` 与 `rootfs-manifest.json`。manifest schema 1 严格字段为 `schema, arch, archive:{sha256,size}, expanded_size, stream_size, entries_sha256, entries`。entries 按 path 排序，摘要来自 ASCII、排序键、无额外空白的 canonical JSON。展开最多 2 GiB、完整 tar stream 最多 2 GiB+64 MiB、最多十万个成员；清单最多 8 MiB。四份元数据每份最多 1 MiB，作为普通文件嵌入 `usr/share/apsp-rootfs/`：

- `inputs-lock.json`。
- `source-inventory.json`。
- `license-inventory.json`：保留版权/许可证实际文件摘要、安装包身份及基础命令实际路径/摘要，`reviewed=false`。
- `provenance.json`：kind 为 `apsp-nodequality-debian12-preparation`，绑定锁/源库存/许可库存/构建模块摘要，`source_authenticated=true` 只表示固定 Debian 输入链认证，`full_ready=false`、`reproducibility_verified=false`。

相同四份 sidecar 与 `export-receipt.json` 保存到导出目录。`verify_export(rootfs_directory, prepared, arch)` 返回 receipt dict；`archive`、`manifest` 都是嵌套 `{sha256,size}`，另有锁/库存/provenance/build tool 摘要、outer reserve 和两个 false 标记。它认证 receipt、sidecar 与 manifest 的绑定；packer 随后必须调用独立 `rootfs.py` 的严格 USTAR/gzip 全归档验证与 `read_metadata`，核验实际四份嵌入字节。不能只看四个 JSON 的声明就签收任意归档。

外层总预算仍是 256 MiB，包括 runner、两份 aux、许可、tar header 与 padding。导出要求明确预留 outer reserve 并核算 compressed rootfs+manifest+reserve；packer 再按真实 runner 和整包核算，任一阶段超限就失败，不拆分归档或放宽限额。

发布签名表示维护者对准备材料及摘要的确认。standalone release 校验可以验证签名、manifest 和嵌入库存绑定，但没有本地 prepared/cache 时不能重演原 Debian 来源链；也不能从公开 receipt 推断输出文件必然来自某次构建。该签名不是许可证证书、实际 builder 镜像证书或双构建可复现性证明。

## 代码验收与待实机材料

遵照本轮指令，本步骤编写期间不运行测试、构建、语法检查或 hash/验证命令。新增测试只提供自有小输入的 schema、签名状态接口、索引链、mirror 篡改、审批边界、链接/特殊文件、预算、deadline、回收和导出绑定场景；其中 mock 明确不能证明真实 Debian 签名。

清理专项测试代码还包括真实小 Python child-group 的 TERM/HUP、实际子进程启动后 selector 创建/注册失败、清理异常保留原错误、目录创建后待处理终止信号，以及残留挂载拒绝删除。Linux harness 仅收养并回收自己的小 grandchild，避免依赖测试容器的 PID 1 回收；它不运行 builder、GPG 或诊断工具。这些专项同样尚未执行。

当前主线组合的验收记录见[本步骤记录](../acceptance/nodequality-offline-rootfs.md)。原分支的测试结果只作历史参考，不认证当前新增版本或主线。

实机材料还需覆盖真实官方签名/keyring、两个原生架构完整依赖的可安装性和真实命令、真实构建设备/挂载清理、两次独立构建的 tree/归档摘要及实际外层预算。不完整对应源码和错误/额外包已由小型夹具检查拒绝，仍须在真实缓存上验收。必须保留失败收据，不能用 synthetic 全通过代替这些项。

真实 snapshot、完整锁与 cache、独立 builder 审批、构建工具来源及许可、真实 rootfs、第三方完整工具授权和复建记录仍缺。只有这些材料和既有服务器保护/取消/心跳验收一起完成，才可另行考虑开放完整验机；本准备层不会打开该门禁。


## 2026-10-02 主线整合边界

本实现来源为尚未合入的 `de54084` 准备代码，窄整合至当前主线；原分支 r18 与主线公共凭证保护 r18 身份不同，本整合的离线准备制品使用新 r20，从主线完整 r19 包装器严格派生，保留主线原生 curl、公共访问保护、日常正式 IP helper 与完整执行准入清单；不改写任何 r19 字节。历史制品不改写，不把原分支 synthetic/Linux 小夹具结果认作本次新主线验收。
