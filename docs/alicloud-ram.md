# 阿里云 CDT 的 RAM 最小权限

CDT 查询与 ECS 查询、启停只需下面的策略；余额查询为可选项。推荐每个 CDT 监控账号固定登记一个地域的一台 ECS，并使用独立 RAM 用户或凭据。APSP 的其他云管理功能需要分别授权，不要为解决查询错误直接授予管理员或 ECS 全权限。

## 查询与实例启停策略

在阿里云 RAM 控制台创建自定义策略，替换启停资源 ARN 的三段占位后，授予对应 RAM 用户：

```json
{
  "Version": "1",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": "cdt:ListCdtInternetTraffic",
      "Resource": "*"
    },
    {
      "Effect": "Allow",
      "Action": "bssapi:QueryAccountBalance",
      "Resource": "*"
    },
    {
      "Effect": "Allow",
      "Action": "ecs:DescribeInstances",
      "Resource": "*"
    },
    {
      "Effect": "Allow",
      "Action": ["ecs:StartInstance", "ecs:StopInstance"],
      "Resource": "acs:ecs:<region>:<主账号 UID>:instance/<实例 ID>"
    }
  ]
}
```

不需要余额时，删除 `bssapi:QueryAccountBalance` 整条语句，并保持账号设置中的「查询账户余额」关闭。只读监控不需要实例启停时，再删除最后一条语句，关闭自动启停策略。

## ARN 里三段占位怎么填

`acs:ecs:<region>:<主账号 UID>:instance/<实例 ID>` 的三段都必须换成实际值：

| 占位 | 含义 | 说明 | 示例 |
| --- | --- | --- | --- |
| `<region>` | 地域 ID | 目标实例所在地域；`*` 会匹配所有地域 | `cn-hongkong` |
| `<主账号 UID>` | 阿里云账号 ID | 资源所属的主账号 ID，不是 RAM 用户 ID 或 AccessKey ID。登录控制台后在右上角头像的账号信息中查看；`*` 表示此字段不限制账号 | `1234567890123456` |
| `<实例 ID>` | ECS 实例 ID | 要授权的具体实例；`instance` 后**必须带斜杠 `/`** | `i-bp1234567890abcdef` |

一账号、一地域、一实例时不使用通配符。通配符会扩大对应地域、账号或实例的匹配范围；不要把 ARN 写成 `instance*`、`instance:<实例 ID>` 或 `instance/*`。下面是格式示例，账号与实例 ID 均须替换：

```json
"Resource": "acs:ecs:cn-hongkong:1234567890123456:instance/i-bp1234567890abcdef"
```

确需授权多台实例时，用数组逐一列出完整 ARN：

```json
"Resource": [
  "acs:ecs:cn-hongkong:1234567890123456:instance/i-bp1111111111abcdef",
  "acs:ecs:cn-hangzhou:1234567890123456:instance/i-bp2222222222abcdef"
]
```

查询语句和启停语句分别授权；查询的 `Resource: "*"` 不会使启停获得全资源权限。RAM 策略的有效授权还会合并已有策略，请移除不再需要的宽泛授权。

## APSP 配置与兼容

- 新增账号默认关闭「查询账户余额」与「查询 CDT 与实例账单」。CDT 脚本守护、ECS 状态查询、每日计划、保活和手动启停不依赖 BSS。
- 余额开关只触发 `QueryAccountBalance`，不隐式开启账单查询。关闭时后台及手动刷新均不会发送该查询；余额失败独立显示，不阻塞 CDT 或 ECS。
- 「查询 CDT 与实例账单」是扩展能力，需要另授 `bssapi:QueryInstanceBill` 与 `bssapi:DescribeInstanceBill`（查询资源为 `*`）。上面的最小策略不包含这些权限。账单阈值通知、阈值停机、自动降速需要开启账单查询；关闭前先关闭依赖它的策略。
- 升级时仅为已有自动降速或已启用账单阈值策略的账号保留账单查询。其他账号停止 BSS 查询，旧缓存保留但不作为关闭查询后的自动控制依据。需要余额或账单展示时重新显式开启对应开关。
- CDT 脚本守护改用单实例 `StartInstance` / `StopInstance`；旧脚本的 `StartInstances` / `StopInstances` 权限不能替代它们。先增加精确实例的单实例授权，再升级，待旧控制器退出后移除批量权限。已发送且结果未知的任务继续只读核对，不重发。
- 最小权限下不调用 `ListSwitchedCdtProducts`。首次零用量不能单独证明已开通 CDT，因此保留历史、暂停自动启停并提示核对。同一凭据、地域和实例曾有有效非零 API 观测后，保存该证据，允许完整零读数按原有月界规则重置并自动开机；同月零读数不会降低历史最高值。空响应、缺少目标额度池、仅导入的历史或凭据身份变化均不能据此开机。此判断依据 [CDT 开通后不支持关闭](https://help.aliyun.com/zh/cdt/product-overview/) 的产品约束；不是对单个资源零费用的保证。
- ECS 绑定 EIP 的信息仍从 `DescribeInstances` 读取，无需 VPC 查询权限。独立 EIP、带宽变配、安全组管理超出本策略范围，只有使用相应功能时才另行授权。

账号 API 的 `billing_enabled` 与 `balance_enabled` 独立控制查询；创建时省略默认为 `false`，更新时省略保留原值。手动刷新不会绕过开关；关闭账单查询会清除当前错误和缓存授权，后台不会因旧缓存仍存在而继续执行账单策略。

## 核对依据

已同步参考 CDT-Monitor 上游 `ad152f0`，APSP 基线为 `f6e362ca`。上游当前状态查询仍使用 `DescribeInstanceStatus`，不能直接套用这份只含 `DescribeInstances` 的策略；本文对应已调整的 APSP 实现。

单实例 ARN 与操作权限参考阿里云官方 [StartInstance](https://help.aliyun.com/zh/ecs/developer-reference/api-ecs-2014-05-26-startinstance)、[StopInstance](https://help.aliyun.com/zh/ecs/developer-reference/api-ecs-2014-05-26-stopinstance)、[DescribeInstances](https://help.aliyun.com/zh/ecs/developer-reference/api-ecs-2014-05-26-describeinstances)。云控制器仍受计量和状态上报延迟影响，不能把阈值守护当作实时硬额度。

BSS 权限采用 `bssapi:<APIName>`，与控制台兼容权限点的映射见阿里云官方 [API 调用授权](https://help.aliyun.com/en/user-center/developer-reference/api-calling-authorization)。API 名称与授权页面显示的 `bss:DescribeAcccount`、`bss:DescribeBillList` 不同，不需要同时授予两套等价权限。
