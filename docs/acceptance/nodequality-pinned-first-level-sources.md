# NodeQuality 首层脚本固定来源独立验收（关联 #28）

本项只固定完整执行链的首层五个脚本。r6 runner 的宿主 curl shim 真实供给这些原字节，不再在运行时取移动分支或 Check.Place 重定向；这段代码位于现有入口的实际执行路径。完整验机门禁仍关闭，#28 不关闭，不能据此签收完整 NodeQuality 或 P0 联合实机总验。

## 来源与打包契约

版本为 `a92fca6c0067df29ddd03fdc2fee6f3000f64545-r6`。入口、首层五脚本和四仓完整 LICENSE 的身份、大小和 SHA256 全在 [source-lock.json](../../crates/observability/upstream/nodequality/source-lock.json)（验收当时位于 `plugins/nodequality/source-lock.json`），其摘要为 `3d20398eeda72654c59b3271fd03b35ca8c0b4e92ee92a054a4a8c432a62723a`。

| 文件 | 固定来源提交 | SHA256 |
| --- | --- | --- |
| NodeQuality.sh | LloydAsp/NodeQuality `a92fca6c0067df29ddd03fdc2fee6f3000f64545` | `4e1b25894cadf908ef61fb0d9ce874a75524c6dafc2ea26f0477107288e0c018` |
| part/header.sh | 同上 | `d6b1990f815bcdb42ac978941b9edf841556c4861e453c23e9bef41b66e4d03f` |
| part/swap.sh | 同上 | `5406da3ab0ff47105f0c06dcbb9fbb34bbb9c5e78c801095c60597b8c6a9d43e` |
| hardware.sh | xykt/HardwareQuality `06f99880d516bb744afa2948261b9697c79789e2` | `73e032ef5409e014cca411a71c677a76db19a94ef0a96a73827b41b2059cd86c` |
| ip.sh | xykt/IPQuality `87397e2c3196ec796f5477c83343c2354df601ea` | `b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf` |
| net.sh | xykt/NetQuality `d5b99484d51286374d24b892c1b54235dc282148` | `6c40fe1ae40d969255cb63075c94882733b82ba43831341eb1aadeea7b1fbfcd` |

四份 LICENSE 各保留 34,523 原字节，均为 AGPL-3.0 全文，SHA256 均为 `8486a10c4393cee1c25392769ddd3b2d6c242d6ec7928e1414efff7dfb2f07ef`。保留每仓来源身份及所有脚本原版权、归属说明，不把这一许可证结论推广到 rootfs 或后续二进制。

构建器只访问清单产生的完整提交 raw URL，保留连接/总时长及 HTTPS-only 限制；每个响应先经 helper 从 stdin 最多读 2 MiB＋1 字节。空响应或超限在创建文件前拒绝，合法响应才以 `O_EXCL|O_NOFOLLOW` 创建新普通文件并固定权限 0600，全部十文件验证成功才嵌入 canonical JSON。curl 的第一项 `--disable` 禁用隐式配置，输出只经管道进入 helper；`--max-filesize` 是额外保护，不承担独立字节上限。入口提交必须与版本一致；不会执行下载源码。五脚本、四许可证、清单和校验 helper 全在同一个 `nodequality` 可执行文件中，由现有制品签名和二进制摘要覆盖，无新增未签名辅助文件。amd64/arm64 是相同架构无关 runner；版本目录、各架构及旧 checksum 的不可变契约保留。

发布前发现旧构建路径依赖 `--max-filesize` 的边界缺口：[curl 官方文档](https://curl.se/docs/manpage.html#--max-filesize) 明确 8.4.0 之前无法用该选项限制预先未知长度的响应。已推送的首个源码点 `7d4e940f1f65af937758cecafc5d7f2de2b947fd` 保留为原验收快照；本项在同一 PR 中补上上述接收器与管道，不改写原证据或发布旧制品。

helper 可以独立输出固定下载清单或校验已有私有源码目录（现行路径在 `crates/observability/upstream/nodequality/`，下述验收命令保留当时路径），以下命令不运行源码：

```sh
python3 plugins/nodequality/source-helper.py downloads plugins/nodequality/source-lock.json
python3 plugins/nodequality/source-helper.py pack plugins/nodequality/source-lock.json "$PRIVATE_SOURCE_DIRECTORY" > "$PRIVATE_BUNDLE"
```

运行时只接受入口真实调用的 `-sL/-Ls` 和五个精确 URL。原入口第 208 行固定 BenchOS v0.0.2 下载和第 435 行既有报告上传分支单独保留；不存在其它宿主 IP 发现 GET 被误拦。未知 URL、参数形状或校验错误拒绝，不能回退在线 main。普通文件读取采用 `O_NOFOLLOW|O_NONBLOCK`，FIFO 在读之前拒绝，避免等写端。

## 本地行为验收

以下测试只用私有合成脚本、回环或完全替身的网络/挂载/运行时；没有运行任何上游脚本、rootfs、硬件测试或公网探测。

```sh
python3 tools/test-nodequality-sources.py
python3 tools/test-nodequality.py
python3 tools/test-diagnostic-modes.py
python3 -m unittest discover -s tests -p test_release.py
```

原快照 macOS 首层来源 12 项通过、0 失败/跳过；旧 wrapper 34 项通过；日常 helper 7 项通过；发布契约 32 项运行，其中 28 通过/4 既有环境条件跳过。补齐下载边界后来源专项扩为 15 项，全部通过、0 失败/跳过（8.916 秒），其中包括再次运行合法构建与 TEST_ONLY 签名/篡改验证。没有重复旧 wrapper、Rust 或日常 helper 场景。

| 场景 | 已证明行为 |
| --- | --- |
| 实际 shim 与 runner PATH | 精确五请求返回合成来源的原字节；合成脚本若执行会留下标记，实际没有标记或 real curl 调用；runner 清理 `.runner` 和锁 |
| 单字节脚本变化 | 运行时返回失败和空输出；构建下一架构失败，已有归档与 checksum 保持，不能上线回退 |
| 未知 URL/选项 | main、额外路径、查询、UA、POST 和非白名单 rootfs 请求均失败；不调用 fake real curl |
| 缺源、缺/变许可证、漂移身份 | 固定清单不完整、重复键、非完整提交、错误许可证角色、不同提交或入口提交与版本不符拒绝；校验失败不产生可执行归档或部分展开目录 |
| FIFO/符号链接 | 自有临时 FIFO 在 3 秒 subprocess 截止内立即拒绝；符号链接拒绝 |
| 两架构不可变构建 | 用真实构建器和私有 curl 替身生成两架构相同 bytes；另一目录重新构建相同 bytes；重复构建拒绝且旧 checksum 不变 |
| 下载字节上限 | 真实接收 CLI 在 umask 777 下仍写入 0600 合法文件；空响应和 2 MiB＋1 响应不创建目标，旧文件拒绝覆盖；超限来源使下一架构构建失败，旧归档/checksum 和锁清理保持 |
| 实际 HTTP 流 | 私有回环 HTTP/1.1 分块无 Content-Length 与声明超限各输出最多 2 MiB＋1；真实 curl stdout 经接收器均拒绝且没有目标文件，server 关闭/线程退出、curl 子进程回收 |
| TEST_ONLY 完整签名 | 真实 minisign 接受合法 r6 合成 runner 包；改内嵌来源/许可证集合后，旧签名包拒绝；重写 checksum 但不重签仍拒绝 |

在独立临时插件副本恢复 `61a65651` 原 curl shim，三个相关用例出现 16 个预期失败断言、0 异常：五源被转发、已篡改来源未拒绝、未知请求可回退。生产源码没有为负对照修改。负对照不累计为修复后的通过数量。

下载流负对照的 macOS curl 为 8.7：分块控制路径 exit 63、最多写 2,097,152 字节，声明超限控制路径 exit 63、0 字节。接收器测试特意不使用 curl 的大小选项，证明自身限制；旧 curl 的控制行为如已 backport 也允许拒绝，不要求复现漏洞才能通过。每个 curl 连接/总时长限制为 1/3 秒，subprocess 截止为 4 秒，所有目标只为测试创建的 `127.0.0.1`。这验证流读取边界，不认证生产 HTTPS、证书或 URL 策略。

Rust 冻结源码的 NodeQuality adapter 诊断专项 15、panel chain_gate PostgreSQL 专项 3、日常 HTTP/PG 入口追加 1 项均通过，0 失败/忽略；r2–r5 历史报告回收、r4–r6 日常参数和预算、所有支持版本的 full prepare 拒绝保留。日常 API 测试直接断言插件版本常量，避免未来硬编码漂移。workspace fmt、adapter 全 targets Clippy（warnings 为错误）、core 分层及差异检查通过；自有 PostgreSQL 55439 已停止。未重跑完整 workspace 或所有 panel Clippy/数据库场景。

冻结 Rust 验收收据摘要：主专项 `0a7891f9b6d3721866e88d6fa05fa8a8a92e957ddec7f98caef6f9e54cffa96d`；追加日常 `2453fd31c6fa500d2592f3864004eb5026732ef39ea8dd1c1ef18fbc766fa60c`。

最初在基线 `61a65651` 保存独立源码快照 `aca2f62bdcdbebc88a95b9830e64d02c917d7c06`。随后重定位到已合入其它协议/监控工作的主线 `be6bf8103a18eb6144a010e3d436ade1c540e9f8`，形成受验输入 `71a778b03d6fa1c6722c9030e3bcb136c72b45b0`：本项五个 Rust 文件和 guest 的 18 输入文件 SHA 全部不变，runner/helper/builder/release/签名契约没有上游变更。SDK 只增加常驻 Adapter 的默认 health_timeout，诊断接口没有改变；Cargo.lock 仅增加已有库的依赖边和 panel 的测试依赖。

新基线上重新执行同一组 19 项 Rust 专项，仍全部通过/0 失败/忽略，adapter Clippy/fmt/core/差异检查通过，专属 PostgreSQL 已停止；收据 SHA256 `51a6a0366bb596336226001fd0dda7ef724aef8aadd850a652032e462ff3c43f`。这两轮是相同 19 场景的不同基线验证，不累计为 38 个场景。当时 guest 证据对应同一冻结 18 文件字节，因此没有重复运行；其它新协议/监控能力及当前整条 main 不由本项测试认证。

发布前最后固定整合基线为 `92800dd9bb83ff92cf87d26fec17fdf73db97e42`，受验输入 `d3ca6756f3708f34653988fc4478efb487754481`；仅 PROGRESS 的并行追加需要合并，完整保留他项记录。本项五 Rust/18 guest 输入、Cargo.lock 和 TEST_ONLY 信任根逐项当时仍相同；为覆盖 panel/probes 的新编译输入，再执行同一组 19 专项及 adapter Clippy/fmt/core/差异检查，全部通过/0 失败/忽略，自有 PG55439 停止并确认 PID 不存在/端口关闭。最终收据 SHA256 `7021f9db82a133f782fb182cd85b59adaffe304052680666d4139ab7e2f56d1c`。重复基线验证不增加场景数量，不认证新增 probes 功能；随后文档回填形成首个提交 `7d4e940`。下载边界补修仅改 Python helper、builder 与来源测试，五 Rust/Cargo/trust 字节仍不变，19 专项未再次运行。

## 专用 Debian 12 ARM64 guest 条件复核

使用下载边界补修前的冻结快照，在已隔离的专用 guest、Linux/root/Bash 条件下只运行上述 wrapper 与来源两个脚本一次。旧 wrapper 34 通过/0 失败/跳过（2.411 秒）；来源 12 项运行、11 通过/0 失败/1 因 guest 未安装 minisign 跳过（1.425 秒）。全部 18 个输入文件前后 SHA256 相同，未下载或执行上游脚本；实际夹具负载 3.929184 秒。`RemainAfterExit` 单元到显式 stop 保留约 3 分 32 秒，这不是测试耗时；没有因清理核验追加或重跑测试。此原结果保留，不用它认证新版接收 helper。

单次 `systemd-run` 的配置参数是 `MemoryMax=256M`、`MemorySwapMax=0`、`TasksMax=64`、CPU/IOWeight 10、OOMScoreAdjust 500、PrivateNetwork/NoNewPrivileges、KillMode control-group 和 RuntimeMaxSec 120s。该次运行的限额 readback 没有在单元 GC 前持久化，因此这里只记录调用配置，不能宣称已独立读回运行时实际限额。

清理核验从原 journal 单元启动 monotonic `5085045994` 到停止 `5297945245` 取证，避免把单元 GC 后零启动时间当作起点：OOM 事件为空，单元 inactive/not-found、MainPID/ControlPID 0、cgroup 不存在、夹具进程/挂载为空、swap 条目 0；SSH PID 406/重启 0/active，仍为同一 boot SHA256 `5a2730c8d35f994be6e7931791fa3e7637cf12043d87875e4bfcc571a3850217`。本段证明真实 Linux/root 包装器夹具与清理，仍没有真实上游 benchmark、Agent/代理持续流量或完整 NodeQuality 总验。

私有收据 `nodequality-pin-r6-guest-20261001` 的结果 SHA256 为 `98858e83afa68019d4471963ce7d6aee8077deb091995595c0557986168495a6`、清理核验为 `7fba20422483b86073f0e630b485a92f5c1d63169fce4fc2560ed518711670b8`、索引为 `3c825c7e91d716ed3e9ba94ff4c788a1e7df33ca9291d873ffc9addddbd2fe32`。关键冻结源码摘要：helper `8fbb3d9840b1eb80404b7e5ca0faa0c615d57e864df7774f2c9e9beae571da9e`，runner `82321b5daca3e49b46dc22c0a3eaad8e4e82d20af1bd1ac76a94d65a1bca465c`，shim `307b56270d67a71d3af667f6e17351bd115676bd3eecb6686b7be26931d0cfce`。

下载边界补修后只在同一专用 guest 追加一次新版来源专项，没有重复旧 wrapper34 或 Rust19：15 项运行，14 通过/0 失败/1 缺 minisign 条件跳过（2.513 秒，driver 2.58366 秒）。实际 curl 7.88 的分块控制路径 exit 0 并写入 2,097,153 字节，声明超限控制路径 exit 63/0 字节；两种响应经新接收器均拒绝且目标不存在。真实 Linux runner 接线路径也在这组来源测试内，仍只执行私有合成来源，不访问公网或执行上游源码。

本次在单元内部实际持久化属性 readback：MemoryMax 268,435,456、MemorySwapMax 0、TasksMax 64、CPUWeight/IOWeight 10、OOMScoreAdjust 500、PrivateNetwork/NoNewPrivileges yes、KillMode control-group。cgroup 的 memory.peak 47,603,712 字节、pids.peak 7，memory.events 的 max/oom/oom_kill 均 0。原 journal 实际启动之后内核 OOM 为空；stop 后单元 inactive/not-found、MainPID/ControlPID 0、cgroup 不存在、夹具进程/挂载为空，SSH PID 406/重启 0/active、同一 boot、swap 0。18 个输入 SHA 前后相同，其中仅 helper/builder/source-tests 三文件相对原快照改变；不会用原快照摘要认证新版接收边界。

本次私有收据 `nodequality-r6-bounded-download-20261001` 的结果 SHA256 `1c5a9a5190b2a7496d3c36f700338071402bedef5a2bcd0ea48babe39a66b3b7`、清理核验 `f6ac776fb3644f46940b00105bb909ab900ff363e6eb16f85af9f7aa113025db`、索引 `3e74cebefcea196396c205486366ae27990e298a5b42e5df5f95124ed7f3a21c` 已独立核对。新版冻结 helper SHA256 `0590d7f9804fd843c2a70313ebc40bbbf3c21c30405491f83d342a07c26ae3c7`、builder `ffffbc2eb515b113d6cae746ec7cb2b6c1ea4d905f6936353e542f3cad2f3824`、来源测试 `d958a67d79f5a727f37a368592b181d021b5075de0de86af410aa5a91dc971a2`。原收据和原结果保留，这次结果只证明有界来源构建夹具。

## 尚未证明的范围

BenchOS 两架构 rootfs、二级 tools/targets/data、下载二进制许可、所有外部上传，以及完整诊断资源与网络行为仍需独立修复和验收。没有添加 `-p`、删掉 Geekbench/GPU 或改原参数来回避风险。原 header/swap/HW/IP/net 原字节未改；现有章节、报告收集、超时/取消回收及 r2–r5 历史兼容保留。

本项没有正式签名、发布或部署 r6；所有签名夹具只使用已公开 TEST_ONLY key。四个工作流按用户要求保持暂停，没有触发或重跑 CI。完整验机只可在前置保护及整条来源/许可/上传链独立通过后恢复，本项不解除面板或 Agent 门禁。
