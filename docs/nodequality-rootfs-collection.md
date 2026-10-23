# 取得 NodeQuality 的 Debian 输入

`tools/nodequality-rootfs-collect.py` 为 [离线 rootfs 构建器](adr/0043-nodequality-offline-rootfs.md) 收集真正的 Debian 输入。它不安装包、不运行诊断、不创建 rootfs，也不批准 builder；收集取得的开源基础库存不能替代 Geekbench、Ookla 等工具的许可和完整能力。决定及审批边界见 [ADR 0063](adr/0063-nodequality-input-collection.md)。

## 环境与信任材料

在专用 Debian 12 的对应原生架构上以 root 运行，需要 Python 3、APT、gpgv、unshare 和系统 CA。root 仅用于新网络命名空间及私有 APT file 镜像求解；`APT::Sandbox::User` 明确为 root，以读取本次 0700 目录，而不更改宿主 APT 配置或目录权限。APT 的命令只更新本地索引及打印下载选择，不能安装包或执行维护脚本。

准备普通文件 keyring 和独立取得说明 JSON。说明字段包含 `schema=1`、`source`、`reviewer`、`obtained_at`、实际 `sha256` 和 `size`；不提供虚构摘要示例。记录 keyring 的可信取得方式及指纹核对材料，不能从待验证 Snapshot 自行取一把任意密钥。收集器只检查说明与实际文件相符，收据保留 `keyring_trust_independently_verified_by_collector=false`，不能将调用者声明升级为自动认证。

使用经过独立审阅的 Debian archive keyring。旧稳定版的 InRelease 可能同时包含后续发行版的已知签名；仅拼接三把 Bookworm 公钥会因其他签名缺键使 `gpgv` 失败。额外已知公钥用于完整验证多重签名，不能单独成为 Bookworm 授权：仍须有原范围内的 Bookworm 主指纹，且坏签名、过期或撤销状态继续拒绝。首次真实收集的缺键失败及同一文件的离线诊断分别保留在整步收据中。

输入请求的严格字段为 `schema=1`、`arch`、`source_epoch`、`repositories`；仓库项为 `id,archive,timestamp,suite`。主仓库、安全仓库必需，updates 可选，同一 archive 使用同一个实际导入时间。仓库提供了两个候选请求：

- `tools/nodequality-rootfs-request-amd64.json`。
- `tools/nodequality-rootfs-request-arm64.json`。

它们采用已从官方导入列表读到的 `debian/20261001T142720Z` 与 `debian-security/20261001T142623Z`。收集时仍重新保存并核对精确实际导入，不接受更早快照回退；这些请求文件本身不是已签名材料或 builder 证明。

## 无 builder 身份时收集

先创建由当前 root 拥有且其他账号不能写入的输出父目录。输出目录必须不存在；下面命令的路径对应操作者已经准备的真实材料：

```sh
python3 tools/nodequality-rootfs-collect.py collect \
  --request tools/nodequality-rootfs-request-amd64.json \
  --keyring /var/lib/apsp-factory/reviewed-bookworm.gpg \
  --keyring-provenance /var/lib/apsp-factory/keyring-provenance.json \
  --output /var/lib/apsp-factory/inputs-amd64-20261001 \
  --max-total-bytes 6442450944 \
  --reserve-free-bytes 2147483648 \
  --timeout-seconds 7200
```

这是收集接口示例，不是建议在任意节点运行的默认资源配置。实际执行前读回可用内存和磁盘，并用 systemd 单元限制内存、swap、TasksMax、CPU/IO 权重及运行期限。网络工作进程每个请求最多 180 秒，并受剩余整体 deadline 约束；没有自动重试或身份伪装。下载在写入前限制响应字节并检查磁盘保留量。APT 可能写派生索引，进入求解前预留其展开上界；目录总预算在阶段边界核对，文件系统硬配额和并发外部磁盘消耗仍属于运行环境范围，不能声称脚本预算等于内核配额。

成功输出包括：

| 路径 | 内容 |
| --- | --- |
| `request.json`、`keyring-provenance.json` | 原始请求和独立取得说明 |
| `input-cache/` | 原始实际导入响应、keyring、InRelease、压缩索引、全部已选 `.deb` 和对应源码文件 |
| `http-*.headers` | HTTP 解析后的响应头；没有声称原始 TLS/HTTP 报文 |
| `solver/` | 私有配置、两次独立网络命名空间证明、命令输出及选择清单；不留派生列表/重复镜像 |
| `capacity-plan.json` | 正文下载前的完整包/源码预计容量、既有元数据与求解开销、显式预算及磁盘预留；是计划，不是取得完成证明 |
| `materials.json` | 没有 builder 字段的实际输入材料，沿用原包和完整源码身份契约 |
| `unbound-inputs.json` | `builder=null,lock_ready=false`；不能作为原构建器的输入锁 |
| `collection.json` | 实际摘要、签名身份、工具字节及本地 dpkg 来源声明；`complete=true`，审批、完整能力与复建标志均 false |

失败保留本次不完整材料及 `failure.json`，记录有限错误原因、目标和已收到对象，不能当作成功收集。不覆盖已有输出或旧缓存；无自动重试。部分材料仍可人工盘点，重新发起操作必须使用新的私有输出目录，不能修改失败收据伪装完成。

`failure.json.failed_download` 关联失败对象的 worker 分类、错误类型和消息、执行阶段、实际观察到的响应状态、已读/已写字节及预期/实际签名身份。收到响应后在状态、重定向和正文校验前记录上下文；后续 TLS/连接失败时，先前重定向响应的状态仍只表示“最近收到的响应”。HTTP 200 本身不能表示正文或签名认证成功。

在 worker 退出或父层正文校验失败后，尽力在 `failed-download-NNNNNN/` 保留有界 worker receipt 与选定响应头 JSON；各文件最多 64 KiB，仍受总预算、磁盘预留和期限限制。只保留 Content-Length/Type/Range/Encoding、Transfer-Encoding、Retry-After 和 Location，URL 去除凭据、查询和片段；失败正文不进入缓存或保留目录。保留失败写入 `failure_evidence.error`。如果 worker 被截止/信号终止且没有完整 receipt，响应状态、具体错误和字节数保持未知，不从父层通用错误或残余头片段推断；新记录也不能补写旧失败原因。

`failure_evidence.complete` 只表示证据保存动作完成。原始 worker receipt 的 `complete=true` 也只表示网络正文接收完成；若父层发现签名 Size/SHA256 不匹配，最终下载记录仍为 `complete=false` 和 `signed_identity_mismatch`，不能将上述字段读成材料认证成功。

在签名索引、隔离 APT 选择与对应源码闭包明确后，先记录 `capacity-plan.json`，再按原保守的所有文件预计总量检查正文容量。计划分别列出引用量和按内容摘要去重的量，不用去重数放宽 admission；现有缓存、求解派生空间、当前磁盘和保留量区分观察值与预计值。正文尚未取得，计划不能标记来源认证完成、批准 builder 或开放完整验机。后续 HTTP 头、日志与最终收据仍按阶段限额检查，计划不承诺并发外部磁盘消耗下容量必然足够。

认证在取得时和最终材料发布前分别执行。实际 package closure 由空 dpkg 状态下的 APT 选择，包含固定 main 的 Essential 集合、APT、原工具库存及 hard dependencies；虚拟包、版本比较和替代依赖交给 APT。APT 的 URI 行可有三个必需字段和一个可选的显示校验和；显示字段不参与认证，身份始终绑定签名 Packages 中的路径、架构、Size 和 SHA256。包的 Source 可能来自另一签名仓库，按精确名称/版本寻找完整 Sources SHA256 清单。各索引逐段扫描，只保存已选包和对应源码，适用于小内存收集；实际峰值需由执行收据确认。

## 绑定候选 builder 和继续构建

真实候选 builder JSON 恰为原锁的 `image_sha256,arch,tools`。三个工具必须分别记录 gpgv、mmdebstrap、unshare 的原规定路径、版本和实际 SHA256/size；不能用零摘要、ISO 或别架构的原始镜像代替当前 builder。该文件仍是候选材料，外部环境管理器还需证明完整镜像、配置、工具来源及审批。

已有真实候选文件时，可在 `collect` 增加 `--candidate-builder`，或在之后的匹配架构环境执行：

```sh
python3 tools/nodequality-rootfs-collect.py bind \
  --materials /var/lib/apsp-factory/inputs-amd64-20261001 \
  --candidate-builder /var/lib/apsp-factory/candidate-builder.json \
  --output /var/lib/apsp-factory/bound-amd64-20261001 \
  --timeout-seconds 600
```

`bind` 拒绝失败收集，重新认证精确导入时间对、全部缓存、签名链和当前候选工具字节，在新目录生成原 schema 1 的 `inputs-lock.json`。`binding.json` 给出原缓存路径，仍保留 `builder_approved=false`。之后才由已有 `prepare --lock --cache --output --approved-builder-image-sha256` 消费，独立 builder 审批不可省略。原生 build、export、双架构复建、256 MiB 外层预算、许可和完整负载条件都保持。

这一步的源冻结、夹具结果及实际取得材料见[整步验收记录](acceptance/debian-inputs-and-singbox-snapshots.md)。未执行的架构、未完成的锁绑定或构建均明确待验。

完整材料取得及容量记录的后续大步骤见[闭包记录](acceptance/complete-debian-materials.md)。前一步 900 MiB 拒绝属于其原始执行，后续盘点或较大明确预算不能改写旧失败结果。

该后续大步骤已完成实际 ARM64 的 237 个二进制包及 539 个对应源码文件，776 个正文独立 Size/SHA256 全部匹配，签名链与精确导入时间分别保留。输出仍是 `builder=null,lock_ready=false` 的未绑定材料；AMD64 收集、builder 工具/完整镜像来源认证、原生构建、双架构复建及完整验机分别待验。容量计划或材料收集完成均不开放完整入口。
