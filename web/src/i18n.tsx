import { useCallback, useEffect } from 'react'
import type { ReactNode } from 'react'
import { LanguagesIcon } from 'lucide-react'
import { NativeSelect } from '@/components/ui/native-select'
import { cn } from '@/lib/utils'
import { useLocaleStore } from './stores/locale'
import type { Locale } from './stores/locale'

export type { Locale }
const translatedTextSources = new WeakMap<Node, string>()
const translatedAttributeSources = new WeakMap<Element, Map<string, string>>()

// Static interface phrases live here. Runtime data, user input and API error
// messages intentionally fall back to their source text when no translation is
// available.
const english: Record<string, string> = {
  'apsp': 'apsp', 'apsp首页': 'apsp home', '服务器与节点': 'Servers and nodes', '菜单': 'Menu', '主导航': 'Main navigation', '当前位置': 'Breadcrumb',
  '概览': 'Overview', '服务器': 'Servers', '代理服务': 'Proxy service', '扩展插件': 'Extensions', '系统': 'System',
  '统计仪表盘': 'Statistics dashboard', '服务器看板': 'Server dashboard', '接入与日常运维': 'Enrollment and daily operations',
  '网络与验机': 'Network and diagnostics', '网络与证书': 'Network and certificates', '批量运维与恢复': 'Bulk operations and recovery',
  '延迟检测': 'Latency checks', '代理节点': 'Proxy nodes', '代理用户': 'Proxy users', '策略与套餐': 'Policies and packages',
  '动态域名解析': 'Dynamic DNS', '阿里云 CDT': 'Alibaba Cloud CDT', '服务器插件': 'Server plugins', '插件目录': 'Plugin catalog',
  '看板与通知': 'Dashboard and notifications', '告警通知': 'Alert notifications', '管理与安全': 'Management and security',
  '系统管理员': 'System administrator', '管理员': 'Administrator', '控制面板': 'Control panel', '管理员会话已登录': 'Administrator session active',
  // Navigation groups, entries and page headers follow docs/product-map.md.
  '总览': 'Overview', '监控告警': 'Monitoring and alerts', '网络': 'Network', '扩展': 'Extensions',
  '统计报表': 'Statistics', '服务器列表': 'Server list', '运维工具': 'Operations tools', '自动化与恢复': 'Automation and recovery',
  '告警事件': 'Alert events', '告警设置': 'Alert settings', '网络检测': 'Network tests', '网络配置': 'Network configuration',
  '服务概览': 'Service overview', '插件中心': 'Plugin center', '面板设置': 'Panel settings', '账号安全': 'Account security',
  '概况': 'Overview', '资产与流量': 'Assets and traffic', 'Agent 与任务': 'Agent and tasks', '插件': 'Plugins', 'IP 信息': 'IP information',
  '节点验机': 'Node check', 'TCP 诊断': 'TCP diagnostics', '服务器导航': 'Server navigation', '带此服务器打开': 'Open with this server',
  '返回服务器列表': 'Back to server list', '服务器启用': 'Server enablement', '看板访问': 'Dashboard access',
  '告警开关与阈值': 'Alert switches and thresholds', '查看告警事件': 'View alert events', '启用插件': 'Enable plugins',
  '告警开关、阈值和 Telegram 通知只有所有者可以查看和修改。': 'Only the owner can view and change alert switches, thresholds and Telegram notifications.',
  '各服务器的 sing-box 状态': 'sing-box status by server', '前往代理服务': 'Go to proxy service', '下一步：按需启用插件': 'Next: enable plugins as needed',
  '这个页面不存在': 'This page does not exist', '链接可能已失效。从左侧导航选择一个页面，或回到服务器列表。': 'The link may be out of date. Pick a page from the navigation or go back to the server list.',
  '这台服务器的运行指标、系统信息和网络接口。': 'Runtime metrics, system information and network interfaces of this server.',
  '地区、成本、到期与网卡流量额度；流量提醒按这里的账单周期计算。': 'Region, cost, expiry and network traffic quota. Traffic alerts use the billing cycle set here.',
  'Agent 的采集与上报设置、这台服务器发起的周期拨测，以及远程命令。': 'Agent collection and reporting settings, periodic probes run from this server, and remote commands.',
  '在这台服务器上启用插件，并查看已启用插件的部署与运行状态。': 'Enable plugins on this server and check the deployment and runtime state of enabled plugins.',
  '这台服务器出口 IP 的查询结果、逐源状态和历史记录。': 'Lookup results, per-source status and history for the outbound IPs of this server.',
  '从这台服务器运行节点验机，查看分段报告和历史结果。': 'Run node checks from this server and review sectioned reports and history.',
  '从这台服务器向已授权目标发起有限的 TCP 连接诊断，保留参数和报告。': 'Run bounded TCP connection diagnostics from this server to authorized targets, keeping parameters and reports.',
  '添加并接入服务器，按状态、地区和分组查看运行情况；点开一台服务器查看详情、资产和诊断。': 'Add and enroll servers, and review them by status, region and group. Open a server for its details, assets and diagnostics.',
  '选择一台服务器，使用终端、服务日志与文件、资产与生命周期等日常工具；也可以用模板批量登记新服务器。': 'Pick a server to use daily tools such as the terminal, service logs and files, and assets and lifecycle. Templates register new servers in bulk.',
  '跨服务器的批量任务与定时计划、故障事件、面板备份与恢复，以及云资源与费用账本。批量操作固定目标并保留每一步的结果。': 'Bulk jobs and schedules across servers, incidents, panel backup and recovery, and the cloud resource and cost ledger. Bulk operations fix their targets and keep the result of every step.',
  '离线、资源、到期和流量告警的触发与恢复记录。显示最近 200 条，已关闭事件保留 90 天；采样缺失时保留原告警，等有效数据确认后再恢复。': 'Firing and recovery records for offline, resource, expiry and traffic alerts. The latest 200 are shown and closed events are kept for 90 days. Missing samples keep an alert open until valid data confirms recovery.',
  '让服务器周期性地用 TCP / ICMP 测量到电信、联通、移动等已授权目标的延迟与丢包，结果显示在看板中。': 'Servers periodically measure latency and loss over TCP / ICMP to authorized carrier targets. Results appear on the dashboard.',
  '决定哪些情况会产生告警，以及告警通过 Telegram 或 Webhook 发给谁。': 'Choose what raises an alert and who receives it over Telegram or Webhook.',
  '从面板或指定服务器测试连通性、路由、吞吐、硬件与 IP 质量。每个结果都记录从哪里测到哪里、工具版本和参数；完整 NodeQuality 验机仍受安全门禁约束。': 'Test connectivity, routes, throughput, hardware and IP quality from the panel or chosen servers. Every result records where it was measured from and to, the tool version and parameters. Full NodeQuality checks remain behind the safety gate.',
  '维护域名、证书、端点、端口转发、系统网络参数、隧道、组网与防火墙。保存配置不会改动服务器，执行变更需要单独确认。': 'Maintain domains, certificates, endpoints, port forwarding, system network parameters, tunnels, private networks and firewalls. Saving does not change servers; applying a change needs its own confirmation.',
  '让域名跟随服务器公网 IP 自动更新，支持 Cloudflare、腾讯云、阿里云和华为云。': 'Keep domains pointed at server public IPs with Cloudflare, Tencent Cloud, Alibaba Cloud and Huawei Cloud.',
  '由 sing-box 插件在你的服务器上提供代理节点，并管理代理用户、授权与订阅。首次使用按下面的顺序完成配置。': 'The sing-box plugin runs proxy nodes on your servers and manages proxy users, access and subscriptions. For a first setup, follow the steps below.',
  '管理你服务器上的节点、由多段组成的链路，以及从订阅来源导入的外部节点。': 'Manage nodes on your servers, multi-hop chains, and external nodes imported from subscription sources.',
  '为代理用户分配策略组（能用哪些节点）和套餐（流量与有效期），管理订阅链接并查看实际代理流量。': 'Assign proxy users a policy group (which nodes they can use) and a package (traffic and validity), manage subscription links and see actual proxy traffic.',
  '策略组决定用户能用哪些节点与链路，套餐决定流量额度和有效期。两者在「代理用户」中分别分配。': 'Policy groups decide which nodes and chains a user can use; packages set the traffic quota and validity. Both are assigned separately under Proxy users.',
  '先了解每个插件做什么、在哪里执行，再进入云账号或服务器管理。同一插件的各版本与架构集中展示。': 'See what each plugin does and where it runs, then go to its cloud account or server management. Versions and architectures of a plugin are grouped together.',
  '按服务器查看和管理插件的启用状态。单台服务器的插件也可在服务器详情的“插件”中管理。': 'Review and manage which servers have each plugin enabled. A single server\'s plugins can also be managed under Plugins in its details.',
  '面板本身的全局设置：看板是否公开、金额按哪种币种显示、监控历史保存多久。': 'Panel-wide settings: whether the dashboard is public, the display currency, and how long monitoring history is kept.',
  '管理员与权限、凭据、API 令牌、会话和操作审计，以及面板自检和工具依赖。': 'Administrators and access, credentials, API tokens, sessions and audit, plus panel self-checks and tool dependencies.',
  '管理当前管理员的 Passkey 和二步验证。其他管理员、会话与 API 令牌在「管理与安全」中管理。': 'Manage passkeys and two-factor authentication for the signed-in administrator. Other administrators, sessions and API tokens live under Management and security.',
  '服务器在线情况、网卡流量与代理流量的趋势和排行，用来回顾一段时间内的整体运行。': 'Trends and rankings for server availability, network traffic and proxy traffic over a period.',
  '供应商与订单、采购配置、续费与年度成本账本，以及维护状态在「运维工具 → 资产与生命周期」中记录；带有效期的续费记录会更新这里的到期日期。': 'Provider and order details, purchased specs, the renewal and annual cost ledger, and maintenance state are recorded under Operations tools → Assets and lifecycle. Renewals with a validity date update the expiry date here.',
  '打开运维工具': 'Open operations tools',
  '退出登录': 'Sign out', '只读 · ': 'Read-only · ', '运维 · ': 'Operations · ', '页面不存在': 'Page not found',
  '界面语言': 'Interface language', '简体中文': 'Simplified Chinese', 'English': 'English',
  '刷新': 'Refresh', '保存': 'Save', '取消': 'Cancel', '关闭': 'Close', '确认': 'Confirm', '删除': 'Delete',
  '编辑': 'Edit', '详情': 'Details', '添加': 'Add', '重试': 'Retry', '复制': 'Copy', '已复制': 'Copied',
  '正在加载…': 'Loading…', '正在保存…': 'Saving…', '正在登录…': 'Signing in…', '登录面板': 'Sign in to panel',
  '验证并继续': 'Verify and continue', '返回修改': 'Back to edit', '关闭对话框': 'Close dialog', '确认删除': 'Confirm deletion',
  '正在删除…': 'Deleting…', '暂无数据': 'No data', '尚未上报': 'Not reported yet', '暂无记录': 'No records',
  '最新信息正在同步，本次操作未执行；请稍候再试。': 'The latest data is syncing. This action was not executed; try again shortly.',
  '浏览器禁止自动复制，请选中文字手动复制。': 'The browser blocked automatic copying. Select and copy the text manually.',
  '复制失败，请手动选择并复制。': 'Copy failed. Select and copy the text manually.',
  '主题与密度': 'Theme and density', '主题': 'Theme', '跟随系统': 'System', '浅色': 'Light', '深色': 'Dark',
  '密度': 'Density', '舒适': 'Comfortable', '紧凑': 'Compact', '语言': 'Language',
  '偏好按管理员保存。搜索快捷键为 Ctrl / ⌘ + K，收藏与最近访问可在全局搜索中直接进入。': 'Preferences are saved per administrator. Use Ctrl / ⌘ + K to search, or open favorites and recent pages from global search.',
  '登录已过期，请重新登录。': 'Your session expired. Please sign in again.',
  '登录': 'Sign in', '登录名': 'Login name', '管理员登录名': 'Administrator login name', '管理员密码': 'Administrator password', '清晰掌握，自在连接。': 'Clear oversight, effortless connections.',
  '欢迎回来': 'Welcome back', '输入管理员密码；已启用二步验证时，还需验证器中的验证码。': 'Enter the administrator password. If two-factor authentication is enabled, enter the authenticator code too.',
  '密码或验证码不正确、已过期或已使用，请重新输入。': 'The password or verification code is incorrect, expired, or already used. Enter it again.',
  '二步验证码': 'Two-factor code', '未启用时留空': 'Leave blank when disabled', '请输入密码': 'Enter password',
  '启用二步验证后，请输入验证器当前的六位数字。': 'Enter the current six-digit code from your authenticator when two-factor authentication is enabled.',
  '使用初始所有者 Passkey 登录': 'Sign in with the initial owner passkey', '仅限管理员访问，使用部署时设置的密码。': 'Administrator access only. Use the password set during deployment.',
  'Passkey 已绑定，请收藏此入口供下次登录。': 'Passkey bound. Bookmark this entry for your next sign-in.', '新 Passkey 已绑定。': 'New passkey bound.', 'Passkey 已删除，其他用户会话已退出。': 'Passkey removed. Other user sessions were signed out.',
  '一处管理，始终有序': 'One place to manage everything', '掌握每一台': 'Know every', '服务器。': 'server.',
  '从设备接入到节点授权，': 'From device enrollment to node authorization,', '让网络的每一步都清晰可见。': 'keep every network step clear.',
  '设备主动连接': 'Devices connect out', '配置自动对账': 'Configurations reconcile automatically', '流量按用户统计': 'Traffic is measured per user',
  'apsp · 自托管服务器与节点面板': 'apsp · Self-hosted server and node panel', '你的服务器，你的控制权。': 'Your servers, your control.',
  '网络检测与验机': 'Network checks and diagnostics', '测试方案': 'Test plans', '授权目标': 'Authorized targets',
  '工具与许可': 'Tools and licenses', 'IP资料来源': 'IP data sources', '任务与报告': 'Jobs and reports',
  'DNS、证书与网络配置': 'DNS, certificates and network configuration', '域名台账': 'Domain inventory', '证书版本': 'Certificate versions',
  '端点台账': 'Endpoint inventory', '端口转发': 'Port forwarding', '系统网络参数': 'System network parameters',
  '反向隧道': 'Reverse tunnels', '私有组网': 'Private networking', '受管防火墙': 'Managed firewall', 'DNS 与 DDNS 插件': 'DNS and DDNS plugin',
  '服务器运维功能': 'Server operations', '资源与能力': 'Resources and capabilities', '资产与生命周期': 'Assets and lifecycle',
  '服务日志与文件': 'Service logs and files', '交互式终端': 'Interactive terminal', '历史与比较': 'History and comparison',
  '模板与批量接入': 'Templates and bulk enrollment', '面板健康': 'Panel health', '节点视图': 'Node views', '管理内容': 'Management content',
  '新增': 'New', '保存表单草稿': 'Save form draft', '恢复表单草稿': 'Restore form draft', '读取最新草稿版本': 'Read latest draft',
  '读取最新草稿': 'Read latest draft', '恢复草稿': 'Restore draft', '保存基础信息草稿': 'Save basic information draft',
  '选择当前筛选': 'Select current filter', '清空选择': 'Clear selection', '进入维护': 'Enter maintenance',
  '停止接收新任务': 'Stop accepting new jobs', '恢复接收任务': 'Resume accepting jobs', '统计时间范围': 'Statistics range',
  '近 7 天': 'Last 7 days', '近 30 天': 'Last 30 days', '配置分区': 'Configuration sections', '名称': 'Name', '地区标签': 'Region and tags',
  '成本到期': 'Cost and expiry', '流量额度': 'Traffic quota', '告警下载': 'Alerts and downloads', '监控': 'Monitoring', '拨测': 'Probes',
  '刷新状态': 'Refresh status', '刷新任务': 'Refresh jobs', '查看告警通知': 'View alerts',
  '系统设置': 'System settings', '管理员设置': 'Administrator settings',
  '服务器信息': 'Server information', '延迟与丢包': 'Latency and packet loss',
  '所有者': 'Owner', '运维': 'Operations', '只读': 'Read-only', '我的工作区': 'My workspace', '会话': 'Sessions', '管理员授权': 'Administrator access', '凭据': 'Credentials', 'API 令牌': 'API tokens', '审计': 'Audit', '系统自检': 'System health', '工具与依赖': 'Tools and dependencies',
  '在线': 'Online', '离线': 'Offline', '待接入': 'Pending enrollment', '全部': 'All', '未知': 'Unknown', '启用': 'Enable', '停用': 'Disable',
  '是': 'Yes', '否': 'No', '不限量': 'Unlimited', '不限期': 'No expiry', '未设置': 'Not set', '不适用': 'N/A',
  '公开服务器看板': 'Public server dashboard', '启用通知与告警': 'Enable notifications and alerts', '启用离线告警': 'Enable offline alerts',
  '离线告警阈值（分钟）': 'Offline alert threshold (minutes)', '到期提前提醒（天）': 'Expiry reminder (days in advance)',
  '流量提醒起始阈值（%）': 'Traffic alert threshold (%)', 'Telegram 通知': 'Telegram notifications', '保存设置': 'Save settings',
  '发送测试通知': 'Send test notification', '正在发送…': 'Sending…', '设置已保存。': 'Settings saved.', '查看模板预览': 'Preview template',
  '服务器网络': 'Server networking', '基础设施': 'Infrastructure', '运行概览': 'Operations overview',
  '当前服务器': 'Current server', '选择服务器': 'Select a server', '选择受管服务器': 'Select a managed server', '独立云资源': 'Standalone cloud resource',
  '选择有权限的处理人': 'Select an authorized assignee', '选择受管云资源': 'Select a managed cloud resource',
  '排序': 'Sort', '筛选': 'Filter', '搜索': 'Search', '按名称': 'By name', '按 CPU': 'By CPU', '按到期时间': 'By expiry',
  '保存当前视图': 'Save current view', '应用已保存视图': 'Apply saved view', '恢复默认视图': 'Restore default view', '紧凑列表': 'Compact list',
  '选择套餐': 'Select package', '选择节点': 'Select node', '选择链路': 'Select chain', '选择目标': 'Select target', '选择工具': 'Select tool',
  '跟随所选节点更新': 'Follow selected node updates', '固定所选版本': 'Pin selected version', '其他客户端（当前未支持）': 'Other client (not supported)',
  '配置可生成': 'Configuration can be generated', '当前不可用': 'Currently unavailable', '检查协议与字段兼容': 'Check protocol and field compatibility',
  '全局搜索': 'Global search', '搜索服务器、域名、节点、任务…': 'Search servers, domains, nodes, tasks…', '搜索结果': 'Search results', '收藏与最近访问': 'Favorites and recent pages', '关闭搜索': 'Close search', '没有匹配的授权对象': 'No matching authorized objects', '输入搜索词或访问对象后添加收藏。': 'Enter a search term or visit an object to add it to favorites.', 'Ctrl / ⌘ + K 打开，Esc 关闭': 'Press Ctrl / ⌘ + K to open, Esc to close', '收藏': 'Favorite', '服务器地址': 'Server address', 'IP来源观测': 'IP source observation', 'IP质量资料': 'IP quality data', '节点': 'Node', '域名': 'Domain', '证书': 'Certificate', 'DNS规则': 'DNS rule', '网络规则': 'Network rule', '诊断任务': 'Diagnostic task', '测试运行': 'Test run', '远程命令': 'Remote command', '设备操作': 'Device operation', '运维任务': 'Operations task', '任务计划': 'Operations schedule', '自动处置规则': 'Remediation rule', '维护窗口': 'Maintenance window', '故障事件': 'Incident', '探测规则': 'Probe rule', '告警规则': 'Alert rule', '延迟监测': 'Latency monitor', '任务': 'Task', '规则': 'Rule',
  '搜索或跳转…': 'Search or jump to…', '搜索或跳转': 'Search or jump to', '输入页面名称或服务器、域名、节点等对象，用方向键选择，回车打开。': 'Type a page name or a server, domain, node or other object; use the arrow keys to choose and Enter to open.', '搜索页面或对象': 'Search pages or objects', '搜索页面、服务器、域名、节点、任务…': 'Search pages, servers, domains, nodes, tasks…', '正在搜索…': 'Searching…', '没有匹配的页面或对象': 'No matching pages or objects', '输入搜索词，或从页面开始。': 'Type a search term, or start from a page.', '选择': 'Select', '打开': 'Open', '取消收藏': 'Remove favorite', '收藏当前项': 'Favorite this item', '已收藏': 'Favorited', '最近访问': 'Recent', '页面': 'Pages',
  '当前账号': 'Signed in as', '更多操作': 'More actions',
  '跳到服务器信息': 'Skip to server information', 'apsp服务器看板': 'apsp server dashboard', '切换浅色主题': 'Switch to light theme', '切换深色主题': 'Switch to dark theme',
  '浅色主题': 'Light theme', '深色主题': 'Dark theme', '进入后台': 'Open administration',
  '服务器总览': 'Server overview', '服务器状态筛选': 'Server status filter', '搜索服务器': 'Search servers', '搜索节点': 'Search nodes',
  '刷新服务器': 'Refresh servers', '刷新服务器详情': 'Refresh server details', '查看详情': 'View details', '看板视图': 'Dashboard view', '卡片': 'Cards',
  '全部地区': 'All regions', '服务器资源与网络状态。数据不可用时保留历史值，实时速率显示为空缺。': 'Server resources and network status. Historical values remain when data is unavailable; live rates are left blank.',
  '在线节点': 'Online nodes', '流量': 'Traffic',
  '延迟 / 丢包': 'Latency / packet loss', '实时速率': 'Live rate', '实时上行': 'Live upload', '实时下行': 'Live download', '上行速率': 'Upload rate', '下行速率': 'Download rate',
  '处理器': 'CPU', '内存': 'Memory', '磁盘': 'Disk', '交换内存': 'Swap', '内核版本': 'Kernel version', '主机名': 'Hostname', '架构未知': 'Architecture unknown',
  '地区': 'Region', '地区未配置': 'Region not configured', '硬件信息': 'Hardware information', '磁盘读写': 'Disk I/O', '拨测目标': 'Probe target',
  '历史数据': 'Historical data', '历史曲线': 'Historical curves', '历史采样': 'Historical samples', '最近采样': 'Latest sample', '最后采样': 'Last sample', '最后在线': 'Last online',
  '最近设备消息': 'Latest device message', '暂无实时流量': 'No live traffic', '暂无有效速率数据': 'No valid rate data', '无采样时间': 'No sample time', '指标待更新': 'Metrics pending update',
  '指标已过期': 'Metrics expired', '状态待确认': 'Status pending confirmation', '查看': 'View', '清除筛选': 'Clear filters', '没有匹配的节点': 'No matching nodes',
  '正在读取服务器': 'Reading servers', '正在读取历史采样': 'Reading historical samples', '正在读取拨测结果': 'Reading probe results', '正在读取拨测配置': 'Reading probe configuration',
  '暂时无法读取最新设备状态。': 'The latest device status is temporarily unavailable.', '尚未添加节点': 'No nodes added yet', '尚未配置拨测': 'No probes configured',
  '前往后台配置拨测': 'Configure probes in administration', '前往服务器管理': 'Open server management', '参考汇率': 'Reference exchange rate', '汇率': 'Exchange rate', '汇率缺失': 'Exchange rate missing',
  '加载中': 'Loading', '更多': 'More', '折叠侧栏': 'Collapse sidebar', '展开侧栏': 'Expand sidebar',
  '调整侧栏宽度': 'Resize sidebar', '调整列表宽度': 'Resize list', '调整代理用户列表宽度': 'Resize proxy user list', '调整台账列表宽度': 'Resize inventory list',
  '按空格键拿起此项，用上下方向键移动位置，再按空格键放下；按 Esc 取消。': 'Press Space to pick up this item, move it with the up and down arrow keys, then press Space to drop it; press Escape to cancel.', '筛选与视图': 'Filters and view', '返回服务器看板': 'Back to server dashboard', '返回看板': 'Back to dashboard',
  '表格': 'Table', '资源趋势': 'Resource trends', '网络接口': 'Network interfaces', '图形处理器': 'GPU',
  '设备状态': 'Device status', '采样信息': 'Sampling details', '系统信息': 'System information', '资产与流量额度': 'Assets and traffic quota',
  '连接中断，等待更新': 'Connection interrupted, waiting for updates',
}

export function translate(value: string, locale: Locale): string {
  return locale === 'en-US' ? english[value] ?? value : value
}


/** English text for a phrase in the static catalog, keeping its surrounding whitespace. */
function englishText(source: string) {
  const value = source.trim()
  const translated = english[value]
  return translated === undefined ? undefined : `${source.match(/^\s*/)?.[0] ?? ''}${translated}${source.match(/\s*$/)?.[0] ?? ''}`
}

// The remembered Chinese source only stands while the node still shows its English
// text. React writes new values into the same text nodes and attributes (a server
// going offline); anything else on screen is that new value, which replaces the
// source instead of being overwritten with the old translation.
function currentSource(known: string | undefined, shown: string) {
  return known !== undefined && shown === englishText(known) ? known : shown
}

function translateStaticDom(locale: Locale) {
  if (typeof document === 'undefined' || !document.body) return
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT)
  let node: Node | null
  while ((node = walker.nextNode())) {
    if (node.parentElement?.closest('code, pre, textarea, input, [data-i18n-ignore]')) continue
    const source = currentSource(translatedTextSources.get(node), node.nodeValue ?? '')
    const inEnglish = englishText(source)
    if (inEnglish === undefined) { translatedTextSources.delete(node); continue }
    const translated = locale === 'en-US' ? inEnglish : source
    if (node.nodeValue !== translated) node.nodeValue = translated
    if (locale === 'en-US') translatedTextSources.set(node, source)
    else translatedTextSources.delete(node)
  }
  document.querySelectorAll<HTMLElement>('[aria-label], [title], [placeholder]').forEach(element => {
    if (element.closest('[data-i18n-ignore]')) return
    let sources = translatedAttributeSources.get(element)
    for (const attribute of ['aria-label', 'title', 'placeholder']) {
      const current = element.getAttribute(attribute)
      if (!current) continue
      const known = sources?.get(attribute)
      const source = known !== undefined && current === english[known] ? known : current
      if (source !== known) sources?.delete(attribute)
      const translated = english[source]
      if (!translated) continue
      const next = locale === 'en-US' ? translated : source
      if (current !== next) element.setAttribute(attribute, next)
      if (locale === 'en-US') {
        if (!sources) { sources = new Map(); translatedAttributeSources.set(element, sources) }
        sources.set(attribute, source)
      } else {
        sources?.delete(attribute)
      }
    }
  })
}

/** Keeps document language attributes and static DOM text in the chosen locale. */
export function I18nProvider({ children }: { children: ReactNode }) {
  const locale = useLocaleStore(state => state.locale)
  useEffect(() => {
    document.documentElement.lang = locale
    document.documentElement.dataset.locale = locale
    translateStaticDom(locale)
    const observer = new MutationObserver(() => translateStaticDom(locale))
    observer.observe(document.body, { childList: true, subtree: true, characterData: true })
    return () => observer.disconnect()
  }, [locale])
  return children
}

export function useI18n() {
  const locale = useLocaleStore(state => state.locale)
  const setLocale = useLocaleStore(state => state.setLocale)
  const t = useCallback((text: string) => translate(text, locale), [locale])
  return { locale, setLocale, t }
}

export function LanguageSelect({ className = '' }: { className?: string }) {
  const { locale, setLocale, t } = useI18n()
  return <label className={cn('relative inline-flex min-w-0 items-center', className)}>
    <span className="sr-only">{t('界面语言')}</span>
    <LanguagesIcon className="text-muted-foreground pointer-events-none absolute left-2.5 z-10 size-4" aria-hidden="true" />
    <NativeSelect aria-label={t('界面语言')} value={locale} onChange={event => setLocale(event.target.value as Locale)} className="min-w-32 *:data-[slot=native-select]:pl-8">
      <option value="zh-CN">简体中文</option>
      <option value="en-US">English</option>
    </NativeSelect>
  </label>
}
