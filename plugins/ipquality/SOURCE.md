# 独立节点出口 IPQuality 来源与执行约束

这是固定 IPQuality 上游 JSON 方法的节点适配，版本为
`87397e2c3196ec796f5477c83343c2354df601ea-node-r1`。它没有硬件跑分、吞吐压测或完整 NodeQuality 入口，不依赖 Geekbench、Ookla 或 NextTrace。完整 NodeQuality 的原许可和实机门禁继续生效。

`source-lock.json` 精确锁定 `xykt/IPQuality` 提交
`87397e2c3196ec796f5477c83343c2354df601ea` 中的四份文件：`ip.sh`、完整 `LICENSE`、`ref/iso3166.json`、`ref/dnsbl.list`。脚本原始 SHA256 为
`b30df5a3c2204276c54e99dcc5080b46f8a627667730aee7de63b109b8ecaecf`。
上游的 AGPL-3.0 许可证和原始版权适用；修改脚本保留明确修改标识。签名制品同时绑定完整上游来源、九份既有策略的精确副本、新适配源码、离线 Debian 工具的对应源码与许可证清单。发布必须向使用者提供可取得的对应源码，不以来源清单代替实际源码材料或发布审查。

构建阶段的 `source-helper.py` 提供以下接口，不执行上游脚本或查询服务：

- `decode_bundle(raw_bytes)` 校验有界来源包。
- `bundle_files(bundle)` 返回精确四个 basename 对应的原始字节。
- `transform_files(files)` 再核对完整来源、许可证和策略身份，返回 `patched-ip.sh`、`transport.py`、两份固定参考数据与许可证字节。
- `pack <source-lock.json> <ordinary-source-directory>` 从本地材料输出来源包。
- `materialize <source-bundle.json> <new-private-directory>` 写入新的独立来源目录。

最小 Debian 输入可以按 [ADR 0069](../../docs/adr/0069-ipquality-derived-debian-inputs.md) 从重新认证的完整固定缓存显式派生。派生只读借用原正文、以隔离离线 APT 选择精确子闭包，另存父收据、profile、缓存身份和资源证据；不是一次新的 HTTP 收集，也不批准 builder。对应源包必须携带 `ipquality-inputs.py` 和容量辅助程序的真实源码，后续 prepare 的独立复制仍须准入。

开发树可以按既有固定 SHA256 从 `plugins/nodequality/` 读取九份纯源码转换策略。发布的源码包在 `plugins/ipquality/policies/` 保存它们的独立精确副本。运行根文件系统只需要已派生脚本和传输守卫，不需要 NodeQuality 的其他代码、固定来源包或商业工具。

运行时固定路径为 `/usr/local/lib/apsp-ipquality/patched-ip.sh`、`transport.py`、`ip-iso3166.json`、`ip-dnsbl.list`。`/usr/local/bin/curl` 只调用这份固定 `transport.py`；真正 HTTP 客户端固定为 `/usr/bin/curl`，避免递归或宿主 PATH 注入。工具来自独立离线 Debian 12 闭包：Bash、Python 3、curl、jq、coreutils、grep、sed、gawk、bc、CA 证书及其依赖。不得在执行时安装依赖或降级到宿主工具。

脚本 CLI 精确接受一个参数 `4` 或 `6`。包装器设置 `APSP_IPQUALITY_FAMILY`、`APSP_IPQUALITY_ATTEMPTS=/work/attempts.jsonl`、`APSP_IPQUALITY_PARTIAL=/work/partial.json` 和任务 UUID；观测出口后设置 `APSP_IPQUALITY_TARGET_IP`。输出 stdout 只有一个该地址族的真实上游 JSON 对象，包装器保存 `/work/upstream.json`。每个数据源完成后按相同上游字段映射原子保存阶段 JSON，取消或失败可以保留此前已完成章节；未完成源不伪造完成结果。

只对所选地址族查询 HTTPS 出口发现源。每个固定发现源最多请求一次；第二个不同源仅在第一个失败后用于独立发现，失败记录保留。观测地址必须符合面板相同的公网地址判定，并规范化后绑定所有后续目标。数据库请求和公开流媒体页面依照固定上游方法顺序执行；一次失败不导致跳过其他独立来源。聚合入口始终标明为 `check-place-aggregator`，其中七个数据库只是 `dataset`，不能宣传为七个独立供应商。

每次逻辑请求最长十秒，连接最多三秒，响应读取保留的前缀最多 2 MiB，包含传输工具的确认尾部预算；触及边界即停止并记录未知，不能当成完整响应。数据库 JSON 最多 64 KiB。最多保存六十四条记录，有界读写并在网络 I/O 前检查次数。正常 HTTPS 重定向最多三次，限于同一个已登记服务及固定路径；403、429、TLS 或连接错误不重试。使用 curl 原生身份，不读取 curlrc，不借用令牌、Cookie、浏览器 UA 或身份头，不使用不安全 TLS、代理参数、在线 `main`、广告、计数器、公开报告上传或宿主网络修改。

`ipregistry`、DB-IP、Disney+、OpenAI 缺少授权查询方式，明确记录为未执行且未知。SMTP 和 DNSBL 未获此入口的批量探测授权，明确为未执行；邮件端口与黑名单计数全部为 `null`，不会把空 DNS 或拒绝访问算作“干净”。旧的随机域名 DNS/native 解锁分类没有可靠证据，标为未知。流媒体地区来自节点自身出口的固定公开页面判断；这不是付费账户播放证明。

逐请求记录采用下面的严格字段，`seq` 从一开始连续递增：

```json
{"seq":1,"provider":"smtp-disabled","dataset":"SMTP","target_ip":null,"url":null,"status":"not_attempted","attempted_at":null,"elapsed_ms":null,"http_status":null,"curl_exit":null,"response_bytes":null,"error_kind":"not_attempted","error_message":"本次未授权 SMTP 探测，信息未知"}
```

实际请求 `status` 为 `succeeded` 或 `failed`，包含实测时间、耗时、HTTP 状态与 curl 退出码。`response_bytes` 表示实际保留的字节数：完整确认的请求为正文长度，触限、超时或未取得尾部确认时仅为留存前缀长度，不能证明完整正文或已观测总大小。成功必须为 HTTP 200、curl 0、非空完整响应和该来源的字段或页面身份校验通过；目标回显若存在必须匹配节点出口。深层 JSON、溢出数字、非法 Unicode、错误国家代码或字段类型都会留下字段不匹配记录。字段值为零或 `false` 本身不能证明成功。`error_kind` 为 `dns`、`connect`、`tls`、`timeout`、`http_403`、`http_429`、`http_other`、`non_json`、`schema_mismatch`、`response_limit`、`request_error` 或 `not_attempted`。错误消息不包含响应正文、令牌或原始 stderr。

包装器以已验签任务制品的外层 SHA256、固定来源提交及源码 SHA256、真实任务 UUID 和地址族封装 `ipquality_result`。面板必须以认证设备和持久任务匹配后写缓存；不能把任意 JSON 文本或单个零分当成成功查询。缓存按节点、实际出口 IP、执行适配器和数据源保存，失败保留之前成功数据并标为历史结果。

源码整改、离线制品准备、签名发布和专用 Debian 实机验收分别记录。此说明及固定转换代码不证明实际镜像审批、可复现构建、取消清理、断连恢复、持续代理流量或实机验收已经通过。
