# ADR 0056：多云 DDNS 插件

状态：已接受。

日期：2026-10-02。用户授权腾讯云 DNSPod、阿里云云解析 DNS、华为云公网 DNS。

## 决策

- 保留 Cloudflare 规则、Token、记录 ID、租约及退役/写入前复核；旧 JSON 缺省提供方仍为 Cloudflare。迁移只追加凭据字段及提供方/线路维度的唯一索引。
- 实现留在 `plugins/ddns/panel/`，共用有限 HTTP 和签名位于 `plugins/cloud_api/panel/`。复用已有依赖，不新增 SDK，不向 Agent 下发凭据或命令。
- 腾讯、阿里填写托管根域名和解析线路，华为填写公网 Zone ID，记录仍仅 A/AAAA。仅 Cloudflare 提供代理和自动 TTL；提供方、Zone、域名、类型及线路创建后固定。
- 完整列表后精确匹配，不选模糊结果首条。同线路重复、CNAME、子域委派和华为多值记录集不覆盖；顶点默认 NS 不误判为委派。已有记录需明确接管；停用/删除不删除远端 DNS。
- 腾讯/阿里添加接口没有原子归属备注，丢失创建回执后未知记录须人工核对接管；地址相同不能证明所有权。华为创建用描述标记，更新不改原描述/标签。写入前复用既有租约及生命周期门禁。
- 签名分别为 TC3-HMAC-SHA256、阿里 RPC HMAC-SHA1、SDK-HMAC-SHA256；固定 HTTPS、禁止代理/重定向、响应/预算有界。凭据只写不读，错误只返回本地类别。

## 参考与验证

参考 [ddns-go](https://github.com/jeessy2/ddns-go) `7aad574de4ba1e4646f07648a38235d34bab6648`（MIT）的提供方和线路设计，独立实现，不沿用首记录选择行为。

官方依据：[腾讯列表](https://cloud.tencent.com/document/api/1427/56166)、[腾讯修改](https://cloud.tencent.com/document/api/1427/56157)、[阿里添加](https://help.aliyun.com/zh/dns/api-alidns-2015-01-09-adddomainrecord)、[阿里修改](https://help.aliyun.com/zh/dns/api-alidns-2015-01-09-updatedomainrecord)、[华为列表](https://support.huaweicloud.com/intl/en-us/api-dns/ListRecordSetsByZone.html)、[华为签名](https://support.huaweicloud.com/intl/en-us/devg-apisign/api-sign-algorithm-002.html)。

本地签名向量、回环 API、PostgreSQL 和浏览器验证不代替真实 DNS 验收；CI 持续暂停。
