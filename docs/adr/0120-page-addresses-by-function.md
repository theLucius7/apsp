# ADR 0120：页面地址按功能命名

状态：部分被取代：地址按功能命名继续有效；“旧地址必须改写、不得删除”先由 [ADR 0122](0122-unreleased-schema-baseline.md) 清理，再由 [ADR 0126](0126-workspaces-language-and-addresses.md) 的地址策略取代。

## 背景

用户要求“重新整理命名路径，应该不叫这种路径的：/#/plugins/sing-box，按照新的运行逻辑，修复，优化，你来决定”。控制台的地址沿用了实现结构：代理业务在 `/#/plugins/sing-box/…`，费用、域名解析和阿里云在 `/#/plugins/…`，设置与告警在 `/#/system/…`、`/#/monitoring/…`。管理员看到的是“概览、代理节点、代理用户”，地址却是插件名。ADR 0086 与 ADR 0098 规定“已发布的地址只改显示名称或位置，不改地址”，用户的要求覆盖这一条。

## 决定

- 地址按页面的用途命名，与侧栏一致：

  | 页面 | 新地址 | 原地址 |
  |---|---|---|
  | 概览 | `/#/home` | `/#/plugins/sing-box` |
  | 代理节点 | `/#/nodes`（含 `?view=`、`/direct/{id}`、`/chain/{id}`） | `/#/plugins/sing-box/nodes`；`/#/plugins/sing-box/chains` → `/#/nodes?kind=chains` |
  | 代理用户 | `/#/users`、`/#/users/{id}` | `/#/plugins/sing-box/users`、`/#/plugins/sing-box/commerce` |
  | 策略与套餐 | `/#/groups` | `/#/plugins/sing-box/groups` |
  | 费用与续费 | `/#/expenses`、`/#/expenses/items/{id}`、`/#/expenses/servers/{id}` | `/#/plugins/expenses/…` |
  | 域名解析 | `/#/dns` | `/#/plugins/ddns` |
  | 阿里云 CDT | `/#/alicloud` | `/#/plugins/alicloud` |
  | 告警事件 / 告警设置 | `/#/alerts`、`/#/alerts/settings` | `/#/system/notifications`、`/#/monitoring/settings` |
  | 面板设置及其分页 | `/#/settings`、`/#/settings/plugins`、`/#/settings/plugins/servers`、`/#/settings/backup`、`/#/settings/admin`、`/#/settings/account` | `/#/system/settings`、`/#/plugins/catalog`、`/#/system/plugins`、`/#/system/backup`、`/#/system/control-center`、`/#/system/administrator` |
  | 用户门户、邮箱登录、邀请加入 | `/#/account/{id}`、`/#/member`、`/#/join`、`/#/join/{码}` | `/#/plugins/sing-box/account/…`、`/#/plugins/sing-box/member`、`/#/plugins/sing-box/join…` |

  统计报表、服务器看板、订阅看板、服务器、线路检测等原本不含插件名的地址不变。`/#/overview` 仍是服务器看板的旧书签别名，所以概览用 `/#/home`。
- **旧地址继续可用**：`web/src/app/paths.ts` 的对照表在打开页面时把旧地址就地改写为新地址（保留 `#` 之前的部分，专属地址的登录页在 `/{path}/login`），书签、已发出的登录地址、开通链接和邀请链接都照常打开；之后新生成的链接一律用新地址。后端生成的门户地址、开通链接、邀请链接与成员登录后的跳转改为新格式；全局搜索的节点、代理用户和告警规则结果，以及续费日历里订阅来源的链接也是新地址，搜索到的代理用户直接打开该用户（`/#/users/{id}`）。
- 对照表只替换开头的旧前缀，其余部分原样保留；`/#/plugins/sing-box/chains` 改写成带查询的 `/#/nodes?kind=chains`，后面再跟的参数用 `&` 接上，子路径不改写（本来也没有这一页）。插件目录的旧别名 `/#/artifacts` 也移进对照表，路由里不再单独判断。
- 全局搜索里保存的“最近访问”和“收藏”按当前地址读取并去重，同一页面的旧记录与新记录只显示一条，再次保存时写成新地址。
- 接口地址（`/api/plugins/…`）不变，它们是插件的接口边界，不是给人看的地址。
- 以后改名同样在对照表里加一行，不删旧行。

## 后果

- 旧地址不会失效，但收藏夹里看到的会逐渐变成新地址。
- 文档中描述当前用法的地址已更新；ADR 与历史记录保留当时的地址。
