# Nexus 与 Wallos 迁移

2026-11-05 起，Nexus 带来的售卖功能（钱包、销售套餐、订单、充值、成员运营页面）已删除，见 [ADR 0113](adr/0113-invitation-quota-without-commerce.md)。迁移用户仍用原邮箱和密码在 `/#/plugins/sing-box/member` 登录（旧网站跳转到这里），邀请改为名额与邀请码，原余额已换算为邀请名额。订阅费用位于「总览 → 费用与续费」（地址仍为 `/#/plugins/expenses`），见 [ADR 0096](adr/0096-costs-and-renewals-ledger.md)。

## 功能与资料

迁移用户在 apsp 中是普通的代理用户，另有一份登录资料（邮箱、密码、提醒偏好）。管理员在「代理用户」中管理他们；登录邮件、公告与指南、邀请规则在「代理用户 › 门户设置」中设置。原订单、钱包、邀请、支付记录保留在数据库中作为历史，不再显示或写入。

原成员编号、订阅令牌、UUID、有效期及已用量保持；原密码只导入哈希，成功登录后升级算法；原到期/流量提醒偏好保持。

订阅费用包含分类、付款方式、付款人、币种和预算、单项周期及频率、下次付款、取消日期、替代订阅、手动续期/复制、统计、日历/ICS、通知记录及导入导出。金额精确到万分之一货币单位，按自然日历推进周期；服务商自动续费设置只推进计划，不在 apsp 发起付款。提醒复用已有 Telegram/Webhook，全局默认关闭。统计保留原币金额，缺少汇率时不伪造汇总。

## 私密快照

成员导入已于 2026-10 完成，导出工具 `tools/export_nexus.py` 与迁移工作台随成员运营删除。Wallos 仍可导入：

```bash
python3 tools/export_wallos.py /private/wallos.db /private/wallos.json --source-id wallos-001 --user-id 1 --timezone Asia/Shanghai
```

Wallos 文件在订阅费用的导入导出页处理。生产快照、哈希、凭据和用户资料均不得提交。

迁移用户登录门户后，可在「线路与探针」查看原 Nexus「节点」页的内容，见 [ADR 0106](adr/0106-member-lines-and-probes.md)：

- **线路：** 入口与落地地区、直连/转发/分流/家宽类型、在线状态、倍率、套餐归属和三网延迟。
- **探针：** 每台主机的负载，以及测得的 ASN、网络组织、用途类型与风险标记。

线路来自已导入的节点目录。仍由旧节点服务的线路，其状态由兼容桥每 30 秒从旧面板导出；已接管线路的状态取 apsp Agent。尚未接入 Agent 的主机暂无 IP 质量，家宽标为运营方声明。成员视图不显示连接地址、SNI、密钥、城市与坐标。

## 实际切换（已完成，2026-10）

切换时的步骤：备份 apsp 与旧面板数据，导入最终快照并逐项核对，激活兼容通道（填写原 HTTPS 订阅基础路径；套餐缺少原权限组映射时拒绝），启动本机同步器，核对后让旧网页地址跳转到 `/#/plugins/sing-box/member`。保留旧订阅路径、节点接口和必要回调；浏览器跳转使用暂时重定向。成员导入与兼容通道激活的接口已随成员运营删除，兼容通道本身继续运行，结束方式见[路线图](roadmap.md)。

同步器配置示例（0600 文件，仅在受信本机使用）：

```json
{
  "legacy_database": "/private/nexus.sqlite",
  "legacy_container": "legacy-backend",
  "pg_container": "apsp-postgres-1",
  "pg_user": "apsp",
  "pg_database": "apsp",
  "import_id": "00000000-0000-4000-8000-000000000001",
  "interval": 10
}
```

```bash
python3 tools/nexus_compatibility.py --config /private/bridge.json --once
python3 tools/nexus_compatibility.py --config /private/bridge.json
```

同步器使用固定参数的本机 Docker CLI，不把密码放在命令行。PostgreSQL 产生带确认代次的成员资格批次，SQLite 事务原子更新允许字段，成功后确认心跳；失败重试不重复扣款。UUID/令牌不匹配时整批回滚。保留旧计量，旧后端初始化新成员缺失的计费日，但不重置已有流量。兼容期间拒绝重置原订阅令牌，防止原链接失效。成员资格变更后刷新旧 WebSocket 节点用户列表，每分钟补发一次完整列表，不改节点配置；失败时保留待刷新状态并重试。原节点仍由兼容后端提供协议服务，新增原生套餐须有旧套餐/权限组映射；原节点迁入 Agent 是后续独立操作。

超过 120 秒未确认同步时，已下发配置保持；排查时先恢复同步，不轮换订阅令牌。旧订阅及节点后端不能随着网页跳转停止。

## 回退与验收

服务器及代理运行时向 Agent 接管的条件、资产准备工具与当前差异见 [Nexus Agent 迁移](nexus-agent-migration.md)。服务器资产迁入不代表已接管代理进程。

开放新写入之前可恢复备份和旧网页路由；一旦 apsp 产生新账号或邀请，不能直接恢复旧数据库而丢弃它们，须冻结新写入、保留现库并对账。回退不得改变客户端原链接、UUID 或流量。

隔离验收使用 `cargo test -p apsp-panel --test user_accounts --test expense_subscriptions`、费用插件单元测试、`python3 -m unittest discover -s tools -p test_migration.py`、Bun 单元测试及 `web/tests/member-accounts.mjs`、`web/tests/expenses-ui.mjs`，只读取本机夹具，不发送通知。切换时的完整范围与结果记在 [验收记录](acceptance/nexus-migration.md)。

旧后端的财务触发器和钱包账本保持原样；兼容器不回写余额，新建的旧协议身份余额为零。原余额已换算为邀请名额（ADR 0113），apsp 不再处理充值、购买与退款。旧协议仅使用套餐、期限、配额及禁用状态；停用、待资助和原生归档在兼容身份上表现为停用，不伪造旧归档审计记录。原本已归档的旧身份保持归档，不在兼容阶段重新启用。
