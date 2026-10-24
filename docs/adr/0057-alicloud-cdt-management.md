# ADR 0057：阿里云 CDT 与公网带宽管理

日期：2026-10-02。用户授权 CDT 管理，明确包含 ECS 固定公网 IP、独立 EIP 的带宽/计费方式调整及自动流量控制。

## 决策

- 独立面板插件 `plugins/alicloud/panel/`，凭据与 DDNS 分开；只管理明确登记的账号/地域/资源，不修改代理业务账本，不向 Agent 下发云凭据。
- 使用 BSS `QueryInstanceBill` 完整分页查询 CDT 账单，展示月份、更新时间、币种与用量单位。缺少账单不表示未开通，账单不等于实时计量/剩余免费额度。CDT 官方元数据返回产品未公开，开通状态显示无法通过公开接口确认，提供官方控制台核对/开通入口，不猜测开通/关闭 API。
- ECS 使用 `DescribeInstances`、`ModifyInstanceNetworkSpec`；独立 EIP 使用 `DescribeEipAddresses`、`ModifyEipAddressAttribute`。不申请/释放 IP、不停机、不改绑定、不操作共享带宽包。缺少公开参数的 EIP 计费转换提供官方控制台入口，不猜测商品参数。
- 手动变配先预览、再明确确认；持久化操作、按资源串行，重启/超时/未知回执先读回，不盲目重发计费写入。外部修改导致前置状态变化时停止旧操作。
- 自动控制默认关闭，阈值和资源范围须明确配置；只按当月完整已出账 CDT 流量降低指定资源的流量计费带宽，不自动换计费、增加带宽或按月恢复。失败/不完整/过期/跨期数据不触发；停用策略与执行互斥。EIP 最低 1 Mbps，降速仍产生流量，不承诺零费用或精准免费额度保护。
- 复用现有 HMAC/SHA/HTTP 依赖。云资源账单查询不是代理业务计费、付款或套餐管理。

## 依据与边界

[CDT 说明及不可关闭](https://help.aliyun.com/zh/cdt/product-overview/what-is-cdt/)、[CDT 1.0/2.0 账单](https://help.aliyun.com/zh/cdt/query-bills)、[BSS 查询](https://help.aliyun.com/zh/user-center/developer-reference/api-bssopenapi-2017-12-14-queryinstancebill)、[ECS 变配](https://help.aliyun.com/zh/ecs/developer-reference/api-ecs-2014-05-26-modifyinstancenetworkspec)、[EIP 公开参数](https://api.aliyun.com/meta/v1/products/Vpc/versions/2016-04-28/apis/ModifyEipAddressAttribute/api.json)。免费额度按账号/地域共享，不硬编码成余额；控制受账单/API 延迟影响。

开发仅用回环 API 替身及 PostgreSQL；不执行真实开通、计费或带宽操作。CI 暂停和已有实机门禁保持。

## 用户补充参考 CDT-Monitor

用户指定 [CDT-Monitor](https://github.com/wang4386/CDT-Monitor)，审阅 `148fd7823367a9618ec185e0f46559c491465470`（MIT）的 `internal/aliyun/client.go` 与任务持久化设计。独立实现 `ListCdtInternetTraffic` 兼容只读查询，按 `BusinessRegionId` 聚合中国内地/海外字节，拒绝缺项、非法值、重复地域和未取完分页；支持中国站/国际站 BSS 端点。该接口公开元数据仍不可用，且参考响应没有可信账期字段，所以兼容读数只展示，不能单独授权自动变配。BSS 查询错误不抹除旧展示数据，也不把旧数据视为有效控制依据。

未复制启停机、抢占式保活、定时恢复、余额或通知系统。手动变配通过明确确认授权相应的云端扣款；自动流程只降低原本按流量计费资源的上限。配置每月最多触发一次，结果待核实时禁止同资源新变配；人工结束跟踪会关闭该资源自动策略。

用户随后明确补齐 ECS 启停、阈值停机、每日计划、抢占式保活与费用缓存，上述暂不实施项由 [ADR 0058](0058-alicloud-power-and-billing-cache.md) 扩展。兼容 CDT 读数只展示及真实云操作不属于开发验证的约束保持。

## 2026-10-24：绑定 EIP 的 ECS 查询兼容

迁移的 CDT 实例绑定 EIP，`DescribeInstances` 的顶层 `InternetChargeType` 为空、`InternetMaxBandwidthOut` 为零；真实网络配置在 `EipAddress`，其中计费方式为 `PayByDominantTraffic`。按固定公网 IP 解析会误报查询异常。

- 保留 ECS 资源身份，读取同一响应的 `EipAddress.AllocationId/IpAddress/Bandwidth/InternetChargeType`，无需额外 VPC 请求或权限。校验实例/地域、唯一结果、EIP 标识、IPv4 和整数带宽；绑定信息缺失或同时存在固定公网地址时拒绝猜测。
- 快照增加可选 `attached_eip_id`，旧 JSON 默认没有绑定，不增加数据库迁移。绑定 EIP 的计费方式独立展示，允许保留未来非空类型，不把主流量计费降格成普通流量计费。
- 读取能力与写入范围分开：手动预览、执行入口、后台自动降速均拒绝将绑定 EIP 当作 ECS 固定公网网络写入；运行中的旧变配不能因 EIP 的地址和目标值恰好一致而误判成功。界面提供 EIP 控制台入口，并保留独立 EIP 原有写入边界。
- CDT 月度状态、150/20 GiB 配置与 ECS 启停逻辑不变。回归覆盖两个真实响应形状、异常字段、自动刷新事务、预览后绑定变化、未知结果读回及桌面/手机查询。

依据：[DescribeInstances 的 EipAddress 字段](https://www.alibabacloud.com/help/en/ecs/developer-reference/api-ecs-2014-05-26-describeinstances)、[阿里云对 PayByDominantTraffic 的定义](https://www.alibabacloud.com/help/en/ros/developer-reference/datasource-vpc-commonbandwidthpackages)。真实验证仅查询两个已迁移账号，不执行带宽、计费或绑定变更。
