# apsp 项目名称

当前仓库、产品界面、Rust crate、可执行程序、脚本、服务、环境变量、容器及后续发布统一使用 `apsp`：

- 仓库：`https://github.com/theLucius7/apsp`。
- 面板与 Agent：`apsp-panel`、`apsp-agent`；环境变量前缀：`APSP_`。
- Linux/FreeBSD 默认目录：`/etc/apsp`、`/var/lib/apsp`、`/opt/apsp`、`/var/log/apsp`。
- 服务：`apsp-agent`、`apsp-singbox`；Windows 数据根目录为 `%ProgramData%\Apsp`。
- 新发布的签名元数据使用 `theLucius7/apsp`，Agent 二进制名使用 `apsp-agent`（Windows 带 `.exe`）。

## 既有安装与证据

这次改名不是线上升级或生产数据搬迁。既有安装升级前必须停止旧服务、备份数据，核对显式路径、环境变量、数据库连接、服务账户与目录权限，再迁移到新命名；不能直接启动默认新目录并将空目录当成既有数据。Compose 项目名改变会产生不同的默认卷名，升级必须显式复用或迁移旧卷。Cookie 和浏览器偏好键也改为新命名，需要重新登录并设置偏好。

数据库迁移 0001–0064 保持原始字节，避免破坏 SQLx 已执行迁移的校验和。0065 转换流量周期函数名称、默认订阅请求标识及当前受管转发记录，并重建唯一索引。管理员自定义请求标识和历史审计快照保持原样；升级前结束旧版本在途任务并重新生成预览。

`docs/acceptance/`、本次改名前的 `PROGRESS.md`、历史提交与历史签名插件源码是历史证据，保留当时的名称、路径和摘要。`plugins/nodequality/historical-r19/` 与 `historical-native-r1/` 参与历史制品校验，不能替换内容再伪称原签名有效。历史适配器版本号、解析历史脚本的边界标记及相应回归测试也保留原值；当前 helper 的固定摘要依据改名后的源码重新计算。历史发布资产不重写、不重新签名；新身份安装需另行构建并签署 apsp Release，现有旧身份 Release 不能作为新身份制品使用。

本次只修改仓库及本地项目关联，不执行真实服务器升级、外部云资源操作或通知发送。远端 CI 继续遵循仓库已有暂停安排。
