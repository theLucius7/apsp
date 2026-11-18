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
  // Line quality (ADR 0130).
  '服务器线路': 'Server lines', '中国可达性': 'China reachability', '未分配': 'Not assigned', '可达性变化': 'Reachability changes', '检测目标': 'Check targets',
  '按延迟': 'By latency', '按丢包': 'By loss', '异常在前': 'Problems first', '筛选服务器': 'Filter servers', '需要关注': 'Needs attention', '端点可达性': 'Endpoint reachability',
  '延迟任务': 'Latency tasks', '立即检测': 'Check now', '服务器详情': 'Server details', '添加延迟任务': 'Add latency task', '尚未配置延迟任务': 'No latency tasks yet', '尚无服务器': 'No servers yet',
  '统计说明': 'About these figures', '判定说明': 'How blocking is judged', '任务说明': 'About tasks', '变化说明': 'About changes',
  // Costs and renewals (ADR 0118).
  '即将续费': 'Upcoming', '支出': 'Spending', '未来 30 天': 'Next 30 days', '待处理': 'To do', '添加项目': 'Add item', '确认续期': 'Confirm renewal',
  '费用记录': 'Cost records', '导出日历': 'Export calendar', '每月费用构成': 'Monthly cost by category', '实际付款': 'Payments', '费用与续费设置': 'Costs and renewals settings', '登记费用': 'Record a cost',
  // Subscription board (ADR 0117): the administrator's own paid subscriptions.
  '订阅看板': 'Subscription dashboard', '订阅总览': 'Subscription summary', '使用中': 'In use', '每月费用': 'Monthly cost', '30 天内续费': 'Renewing within 30 days',
  '需关注': 'Needs attention', '订阅状态筛选': 'Subscription status filter', '搜索订阅': 'Search subscriptions',
  '管理代理用户': 'Manage proxy users', '下次续费': 'Next renewal', '续费方式': 'Renewal', '没有匹配的订阅': 'No matching subscriptions', '原系统': 'Original system',
  '在费用与续费中管理': 'Manage in costs and renewals',
  // Sign-in page and account card (2026-11-06).
  '隐藏密码': 'Hide password', '显示密码': 'Show password',
  // Server monitoring panel, server pages and overview (2026-11-05 redesign).
  '实时状态': 'Live status', '实时网络': 'Live network', '上行': 'Upload', '下行': 'Download', '本期流量': 'Traffic this cycle', '额度已用尽': 'Quota used up', '观测不完整': 'Incomplete observation',
  '等待采样': 'Waiting for samples', '不限': 'Unlimited', '操作系统': 'Operating system', '常规': 'General', '设置已保存': 'Settings saved', '此服务器的节点': 'Nodes on this server',
  '目标版本': 'Target revision', '已应用版本': 'Applied revision', '管理节点': 'Manage nodes', '配置拨测': 'Configure probes',
  '续期': 'Renew', '今天': 'Today', '部署历史': 'Deployment history', '各服务器的 sing-box': 'sing-box on each server', '各服务器的运行时': 'Runtimes on each server', '网络配置（实验）': 'Network configuration (experimental)', '自动化与维护（实验）': 'Automation and maintenance (experimental)',
  '配置同步受阻': 'Configuration sync blocked', '待升级 Agent': 'Agent upgrade required', '当前配置健康': 'Current configuration healthy', '当前健康待确认': 'Current health pending confirmation',
  '未配置服务': 'Service not configured', '等待设备接入': 'Waiting for device enrollment', '设备离线': 'Device offline', '等待应用配置': 'Waiting to apply configuration', '运行中': 'Running', '应用失败': 'Application failed', '服务与运行状态': 'Service and runtime status', '查看部署': 'View deployment', '配置服务': 'Configure service', '只读保留主机': 'Read-only retained host',
  '原生订阅接管': 'Native subscription adoption', '转由 apsp 生成订阅': 'Switch subscription generation to apsp', '比较并接管订阅': 'Compare and adopt subscription', '比较并接管原生订阅': 'Compare and adopt native subscription', '重新预览': 'Preview again', '确认接管此用户': 'Confirm user adoption', '正在接管…': 'Adopting…',
  '保留原订阅地址': 'Keep original subscription address', '显示原订阅地址': 'Show original subscription address', '复制原订阅地址': 'Copy original subscription address', '逐项比较客户端格式': 'Compare client formats', '隐藏候选内容': 'Hide candidate content', '已人工比较': 'Compared', '尚未比较': 'Not compared yet', '当前不可生成': 'Unavailable',
  '负载（1 / 5 / 15 分钟）': 'Load (1 / 5 / 15 min)', '看板已隐藏': 'Hidden from dashboard',
  // Connection state, account menu and the sign-in page (ADR 0102).
  '专属地址': 'Own address', '用户专属地址': 'Personal addresses', '登录地址': 'Sign-in address', '订阅链接': 'Subscription link', '订阅详情': 'Subscription details', '复制订阅链接': 'Copy subscription link', '权限与套餐': 'Permissions and package', '单独授权': 'Direct grants', '流量明细': 'Traffic details', '诊断与安全': 'Diagnostics and security', '添加 Cloudflare 账号': 'Add Cloudflare account', '验证并保存': 'Verify and save', '其他提供方': 'Other providers', 'DNS 账号': 'DNS accounts', '添加解析': 'Add record', '添加两条解析': 'Add both records', '解析记录': 'DNS records', '解析到': 'Points to', '添加账号': 'Add account', '添加 DNS 账号': 'Add DNS account', '连接 DNS 账号': 'Connect a DNS account', '先连接 DNS 账号': 'Connect a DNS account first', '还没有解析': 'No records yet', '管理记录': 'Manage records', '更多设置': 'More settings', '批量修改': 'Bulk edit', '立即同步': 'Sync now', '恢复同步': 'Resume sync', '暂停': 'Pause', '核对与历史': 'Check and history', '检查连接': 'Check connection', '解析操作': 'Record actions', '账号操作': 'Account actions', '这个页面显示时出错了': 'This page failed to display',
  '监控与设置': 'Monitoring and settings', '告警': 'Alerts', '线路质量': 'Line quality', '设置': 'Settings', '域名解析': 'DNS', '本节页面': 'Pages in this section', '显示实验与冻结功能': 'Show experimental and frozen features',
  '暂时连不上面板': 'Cannot reach the panel right now', '正在自动重试，恢复后会直接进入。': 'Retrying automatically; you will go straight in once it is back.', '立即重试': 'Retry now',
  '已连接面板': 'Connected to panel', '网络已断开': 'Network disconnected', '正在重新连接': 'Reconnecting', '已恢复连接': 'Connection restored', '与面板的连接中断，正在重试': 'Connection to the panel was lost, retrying',
  '显示的是': 'Showing data from', '的数据': '', '账号选项': 'Account options', '当前项': 'Current', '或': 'or',
  '正在同步最新信息，完成后可以提交；当前草稿已保留。': 'Syncing the latest information; you can submit when it finishes. Your draft is kept.',
  'apsp': 'apsp', 'apsp首页': 'apsp home', '菜单': 'Menu', '主导航': 'Main navigation', '当前位置': 'Breadcrumb',
  '概览': 'Overview', '服务器': 'Servers', '代理服务': 'Proxy service', '系统': 'System',
  '统计仪表盘': 'Statistics dashboard', '服务器看板': 'Server dashboard', '接入与日常运维': 'Enrollment and daily operations',
  '网络与验机': 'Network and diagnostics', '网络与证书': 'Network and certificates', '批量运维与恢复': 'Bulk operations and recovery',
  '质量概况': 'Quality overview', '检测设置': 'Check settings', '代理节点': 'Proxy nodes', '代理用户': 'Proxy users', '策略与套餐': 'Policies and packages',
  '动态域名解析': 'Dynamic DNS', '阿里云 CDT': 'Alibaba Cloud CDT',
  '看板与通知': 'Dashboard and notifications', '告警通知': 'Alert notifications', '管理与安全': 'Management and security',
  '系统管理员': 'System administrator', '管理员': 'Administrator', '控制面板': 'Control panel', '管理员会话已登录': 'Administrator session active',
  // Navigation groups, entries and page headers follow docs/product-map.md.
  '总览': 'Overview', '监控告警': 'Monitoring and alerts', '网络': 'Network', '扩展': 'Extensions',
  '统计报表': 'Statistics', '趋势与报表': 'Trends and reports', '地址与域名': 'Addresses and domains', '云资源': 'Cloud resources', '用户': 'Users', '付费服务': 'Paid services', '监测与诊断': 'Monitoring and diagnostics', '服务器列表': 'Server list', '运维工具': 'Operations tools', '自动化与恢复': 'Automation and recovery',
  '告警事件': 'Alert events', '告警设置': 'Alert settings', '网络检测': 'Network tests', '网络配置': 'Network configuration',
  '服务概览': 'Service overview', 'Agent 版本': 'Agent versions', '网络服务': 'Network services', '运行时': 'Runtimes', '外部来源': 'External sources', '面板设置': 'Panel settings', '账号安全': 'Account security',
  '实验功能': 'Experimental', '实验': 'Experimental', '冻结': 'Frozen', '备份与恢复': 'Backup and recovery',
  '自动化与维护': 'Automation and maintenance', '线路诊断': 'Line diagnostics', '批量接入': 'Batch enrollment',
  '实验功能：保留可用，只修缺陷和安全问题，不再扩展；不属于代理网络主线。': 'Experimental: kept usable and fixed for bugs and security only; not part of the proxy network core.',
  '冻结功能：不再投入，只修安全问题；默认隐藏，可在「面板设置」中显示。': 'Frozen: security fixes only; hidden by default and can be shown in Panel settings.',
  '需要处理': 'Needs attention', '开始使用': 'Get started', '常用入口': 'Shortcuts', '功能显示': 'Feature visibility', '显示冻结功能': 'Show frozen features',
  '概况': 'Overview', '资产与流量': 'Assets and traffic', 'Agent 与任务': 'Agent and tasks', 'IP 信息': 'IP information',
  '节点验机': 'Node check', 'TCP 诊断': 'TCP diagnostics', '服务器导航': 'Server navigation', '带此服务器打开': 'Open with this server',
  '返回服务器列表': 'Back to server list', '看板访问': 'Dashboard access',
  '告警开关与阈值': 'Alert switches and thresholds', '查看告警事件': 'View alert events', '添加节点': 'Add node',
  '告警开关、阈值和 Telegram 通知只有所有者可以查看和修改。': 'Only the owner can view and change alert switches, thresholds and Telegram notifications.',
  '各服务器的 sing-box 状态': 'sing-box status by server', '前往节点管理': 'Go to node management', '下一步：按需配置代理服务': 'Next: configure proxy services if needed',
  '这个页面不存在': 'This page does not exist', '链接可能已失效。从左侧导航选择一个页面，或回到服务器列表。': 'The link may be out of date. Pick a page from the navigation or go back to the server list.',
  '这台服务器的运行指标、系统信息和网络接口。': 'Runtime metrics, system information and network interfaces of this server.',
  '地区、成本、到期与网卡流量额度；流量提醒按这里的账单周期计算。': 'Region, cost, expiry and network traffic quota. Traffic alerts use the billing cycle set here.',
  'Agent 的采集与上报设置、这台服务器发起的周期拨测，以及远程命令。': 'Agent collection and reporting settings, periodic probes run from this server, and remote commands.',
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
  '为代理用户分配策略组（能用哪些节点）和套餐（流量与有效期），管理订阅链接并查看实际代理流量。': 'Assign proxy users a policy group (which nodes they can use) and a package (traffic and validity), manage subscription links and see actual proxy traffic.',
  '策略组决定用户能用哪些节点与链路，套餐决定流量额度和有效期。两者在「代理用户」中分别分配。': 'Policy groups decide which nodes and chains a user can use; packages set the traffic quota and validity. Both are assigned separately under Proxy users.',
  '面板本身的全局设置：看板是否公开、金额按哪种币种显示、监控历史保存多久。': 'Panel-wide settings: whether the dashboard is public, the display currency, and how long monitoring history is kept.',
  '管理员与权限、凭据、API 令牌、会话和操作审计，以及面板自检和工具依赖。': 'Administrators and access, credentials, API tokens, sessions and audit, plus panel self-checks and tool dependencies.',
  '管理当前管理员的 Passkey 和二步验证。其他管理员、会话与 API 令牌在「管理与安全」中管理。': 'Manage passkeys and two-factor authentication for the signed-in administrator. Other administrators, sessions and API tokens live under Management and security.',
  '服务器在线情况、网卡流量与代理流量的趋势和排行，用来回顾一段时间内的整体运行。': 'Trends and rankings for server availability, network traffic and proxy traffic over a period.',
  '供应商与订单、采购配置、续费与年度成本账本，以及维护状态在「运维工具 → 资产与生命周期」中记录；带有效期的续费记录会更新这里的到期日期。': 'Provider and order details, purchased specs, the renewal and annual cost ledger, and maintenance state are recorded under Operations tools → Assets and lifecycle. Renewals with a validity date update the expiry date here.',
  '打开运维工具': 'Open operations tools',
  '费用与续费': 'Costs and renewals', '续费项目': 'Renewal items',
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
  '语言': 'Language',
  '偏好按管理员保存。搜索快捷键为 Ctrl / ⌘ + K，收藏与最近访问可在全局搜索中直接进入。': 'Preferences are saved per administrator. Use Ctrl / ⌘ + K to search, or open favorites and recent pages from global search.',
  '登录已过期，请重新登录。': 'Your session expired. Please sign in again.',
  '登录': 'Sign in', '登录名': 'Login name', '管理员登录名': 'Administrator login name', '管理员密码': 'Administrator password', '清晰掌握，自在连接。': 'Clear oversight, effortless connections.',
  '使用管理员账号登录。': 'Sign in with an administrator account.', '其他用户请使用管理员发给你的登录地址。': 'Other users: use the sign-in address your administrator sent you.', '我的账户': 'My account', '欢迎回来': 'Welcome back', '输入管理员密码；已启用二步验证时，还需验证器中的验证码。': 'Enter the administrator password. If two-factor authentication is enabled, enter the authenticator code too.',
  '密码或验证码不正确、已过期或已使用，请重新输入。': 'The password or verification code is incorrect, expired, or already used. Enter it again.',
  '二步验证码': 'Two-factor code', '未启用时留空': 'Leave blank when disabled', '请输入密码': 'Enter password',
  '启用二步验证后，请输入验证器当前的六位数字。': 'Enter the current six-digit code from your authenticator when two-factor authentication is enabled.',
  '使用 Passkey 登录': 'Sign in with a passkey', '按上面的登录名验证该管理员的 Passkey，无需密码。': 'Uses the passkeys of the administrator named above; no password needed.', '仅限管理员访问，使用部署时设置的密码。': 'Administrator access only. Use the password set during deployment.',
  'Passkey 已绑定，请收藏此入口供下次登录。': 'Passkey bound. Bookmark this entry for your next sign-in.', '新 Passkey 已绑定。': 'New passkey bound.', 'Passkey 已删除，其他用户会话已退出。': 'Passkey removed. Other user sessions were signed out.',
  '一处管理，始终有序': 'One place to manage everything', '掌握每一台': 'Know every', '服务器。': 'server.',
  '从设备接入到节点授权，': 'From device enrollment to node authorization,', '让网络的每一步都清晰可见。': 'keep every network step clear.',
  '设备主动连接': 'Devices connect out', '配置自动对账': 'Configurations reconcile automatically', '流量按用户统计': 'Traffic is measured per user',
  '网络检测与验机': 'Network checks and diagnostics', '测试方案': 'Test plans', '授权目标': 'Authorized targets',
  '工具与许可': 'Tools and licenses', 'IP资料来源': 'IP data sources', '任务与报告': 'Jobs and reports',
  'DNS、证书与网络配置': 'DNS, certificates and network configuration', '域名台账': 'Domain inventory', '证书版本': 'Certificate versions',
  '端点台账': 'Endpoint inventory', '端口转发': 'Port forwarding', '系统网络参数': 'System network parameters',
  '反向隧道': 'Reverse tunnels', '私有组网': 'Private networking', '受管防火墙': 'Managed firewall',
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
  '全局搜索': 'Global search', '搜索服务器、域名、节点、任务…': 'Search servers, domains, nodes, tasks…', '搜索结果': 'Search results', '收藏与最近访问': 'Favorites and recent pages', '关闭搜索': 'Close search', '没有匹配的授权对象': 'No matching authorized objects', '输入搜索词或访问对象后添加收藏。': 'Enter a search term or visit an object to add it to favorites.', 'Ctrl / ⌘ + K 打开，Esc 关闭': 'Press Ctrl / ⌘ + K to open, Esc to close', '收藏': 'Favorite', '服务器地址': 'Server address', 'IP来源观测': 'IP source observation', 'IP质量资料': 'IP quality data', '节点': 'Node', '域名': 'Domain', '证书': 'Certificate', 'DNS规则': 'DNS rule', '网络规则': 'Network rule', '诊断任务': 'Diagnostic task', '测试运行': 'Test run', '远程命令': 'Remote command', '设备操作': 'Device operation', '运维任务': 'Operations task', '任务计划': 'Operations schedule', '自动处置规则': 'Remediation rule', '维护窗口': 'Maintenance window', '故障事件': 'Incident', '探测规则': 'Probe rule', '告警规则': 'Alert rule', '任务': 'Task', '规则': 'Rule',
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
  '全部服务器': 'All servers', '筛选结果': 'Filtered servers', '筛选名称、标签或地区': 'Filter by name, tag or region', '搜索名称、标签或地区': 'Search names, tags or regions',
  '状态': 'Status', '全部状态': 'All statuses', '离线或未接入': 'Offline or not enrolled', '更多筛选': 'More filters', '分组': 'Group', '全部分组': 'All groups',
  '视图': 'View', '视图设置': 'View settings', '列表分组方式': 'Group the list by', '不分组': 'No grouping', '按服务器分组': 'By server group', '按地区分组': 'By region',
  '接入顺序': 'Enrollment order', 'CPU 使用率': 'CPU usage', '到期时间': 'Expiry', '表格列': 'Table columns', '卡片显示': 'Card fields', '成本与到期': 'Cost and expiry',
  '设备版本': 'Agent version', '列表密度': 'List density', '完成': 'Done', '已启用的筛选条件': 'Active filters', '清除全部筛选': 'Clear all filters',
  '没有符合条件的服务器': 'No servers match', '无法读取服务器列表': 'Could not read the server list', '从第一台服务器开始': 'Start with your first server',
  '批量操作': 'Bulk actions', '正在提交…': 'Submitting…', '上移': 'Move up', '下移': 'Move down', '上传': 'Upload', '下载': 'Download',
  // Self-contained Agent releases, built-in components and config-driven runtime states.
  '运行状态暂时无法识别，请刷新确认。': 'Runtime status is not recognized right now. Refresh to confirm.',
  '先添加并接入服务器，再创建节点或配置网络服务。': 'Add and enroll a server first, then create nodes or configure network services.',
  '检查运行条件': 'Check runtime readiness',
  'Agent 接入、设备在线与内置运行时能力': 'Agent enrollment, device online status and built-in runtime capability',
  '当前 Agent 尚未声明内置运行时能力，请升级 Agent': 'The connected Agent does not declare built-in runtime capability yet; upgrade the Agent',
  '当前服务器尚未配置代理节点或链路，纯监控模式不运行代理进程。添加节点、选入转发链路或保存下方服务端配置后，在线一体化 Agent 会自动启动并应用服务。': 'No proxy nodes or chains are configured on this server yet; monitor-only mode runs no proxy process. Add a node, include this server in a chain, or save the server-side configuration below and the online self-contained Agent will start and apply the service automatically.',
  '当前服务器处于只读保留状态，未在此面板配置新的代理服务。': 'This server is retained as read-only and has no new proxy services configured in this panel.',
  '正在等待设备应用最新配置。设备离线时会在重连后自动同步。': 'Waiting for the device to apply the latest configuration. Offline devices sync automatically once reconnected.',
  '还没有发布记录。保存节点、链路或服务端配置后会自动生成部署并下发到在线 Agent。': 'No deployment records yet. Saving nodes, chains, or server-side configuration automatically creates a deployment for the online Agent.',
  '用同一份服务配置生成新版本并重新下发到这台服务器。日常宿主机重启或进程退出由一体化 Agent 自动受控恢复，仅在需要强制重发当前配置时使用。需要再次验证身份。': 'Create a new revision from the same service configuration and send it to this server again. Routine host reboots or process exits are recovered automatically by the self-contained Agent; use this only to force-resend the current configuration. Re-authentication is required.',
  'Agent 与内置组件': 'Agent and built-in components',
  '一体化 Agent': 'Self-contained Agent',
  'sing-box 运行时': 'sing-box runtime',
  'TCP 连接诊断': 'TCP connection diagnostics',
  'IP 质量检测': 'IP quality check',
  '当前设备尚未上报内置组件版本；升级到一体化 Agent 并连接后将在此显示内置运行时与诊断能力版本。': 'This device has not reported built-in component versions yet. Upgrade to the self-contained Agent and connect to view built-in runtime and diagnostic versions here.',
  'Agent 发布与升级方式': 'How Agent releases and upgrades work',
  '前往服务器列表': 'Go to server list',
  '一体化 Agent 已内置 sing-box 运行时与线路诊断能力，无需单独选择或部署组件版本。设备从外部可信发布源或已配置的下载加速地址拉取发布包并独立验签，升级时保留设备身份。': 'Each self-contained Agent bundles the sing-box runtime and line diagnostic capabilities, with no separate component version selection or installation. Devices fetch releases from trusted upstream sources or a configured mirror, verify signatures independently, and keep their device identity across upgrades.',
  '发布与升级策略': 'Release and upgrade policy',
  '默认版本策略': 'Default version policy',
  '自动匹配最新兼容版本': 'Match latest compatible version automatically',
  '版本匹配规则': 'Selection rule',
  '按目标平台选择最高已签名且协议兼容的稳定版本': 'Highest signed, protocol-compatible stable release for the target platform',
  '最低允许接入版本': 'Minimum allowed enrollment version',
  '新服务器接入或手工更新请在服务器详情点击「接入 / 升级」生成命令；已接入服务器可在各自的「Agent 与任务」中开启自动更新。具体服务器的内置组件版本在服务器详情中直接读取。': 'To enroll a new server or upgrade manually, open the server details and click Enroll / Upgrade to generate a command. Enrolled servers can enable automatic updates under Agent and tasks. Built-in component versions of each server are shown directly in its server details.',
  '已收录签名版本': 'Recorded signed versions',
  '暂无已收录的签名版本': 'No signed versions recorded yet',
  '维护者准备并收录官方签名 Agent 发布后，接入与自动升级可用的版本会在此列出。': 'Once the maintainer records signed official Agent releases, versions available for enrollment and automatic upgrades appear here.',
  '查看服务器': 'View servers',
  '版本号': 'Version',
  '发布标签': 'Release tag',
  '支持平台与架构': 'Supported platforms and architectures',
  '协议范围': 'Protocol range',
  'Linux AMD64（静态构建）': 'Linux AMD64 (static build)',
  'Linux ARM64（静态构建）': 'Linux ARM64 (static build)',
  '查看 Agent 版本': 'View Agent versions',
  '监控与诊断已接入，可以在看板、告警和运维工具中使用。需要代理时直接创建节点或加入转发链路，在线一体化 Agent 会自动启用内置 sing-box。': 'Monitoring and diagnostics are connected and ready across the dashboard, alerts, and operations tools. When proxy service is needed, create a node or add this server to a chain and the online self-contained Agent will enable built-in sing-box automatically.',
  '从外部可信发布源或已配置的下载加速地址拉取面板选定的签名一体化 Agent（内置 sing-box 与诊断能力），独立验证签名并保留设备身份与本地账本；旧版 Agent 需先通过「接入 / 升级」手工迁移一次。下载加速在编辑服务器中设置。': 'Downloads the panel-selected signed self-contained Agent (bundling sing-box and diagnostics) from trusted release sources or the configured mirror, verifying signatures independently while keeping device identity and local ledgers. Legacy Agents require a one-time manual upgrade via Enroll / Upgrade. Configure mirrors under Edit server.',
  'Agent 发布与版本观察': 'Agent releases and version observation',
  'Agent 发布观察状态未知': 'Agent release observation status unknown',
  '查看 Agent 版本与升级策略': 'View Agent versions and upgrade policy',
  '发布签名核对': 'Release signature verification',
  '发布签名及内容摘要已核对，不表示已在目标服务器部署': 'Release signature and digest verified; does not indicate deployment on target servers',
  '尚无已核对的签名发布记录': 'No verified signed release records yet',
  '工作区锁文件与随发布构建的内置诊断依赖保留精确版本，包含开发依赖，不表示每个包都进入此面板进程。工具许可门禁、发布签名与运行兼容性分别核对，不能凭此记录声称组件已部署或已通过实机验收。': 'Workspace lockfiles and built-in diagnostic dependencies bundled at release retain exact versions, including development dependencies; not every package runs in the panel process. Tool license gates, release signatures, and runtime compatibility are verified separately, and these records do not imply deployment or live-host acceptance.',
  '完整恢复点包含数据库、面板配置和恢复材料清单。独立恢复工具可在面板不能启动时使用，并以固定镜像和数据库迁移身份执行隔离演练。': 'A full recovery point includes the database, panel configuration, and recovery manifest. The standalone recovery tool works even when the panel cannot start and runs isolated drills against pinned images and migration identities.',
  '由面板本机执行数据库一致性导出和恢复材料归档，然后用独立保管方的 age 公钥加密；只有工具实际完成才登记备份结果。': 'Runs a consistent database export and recovery material archive on the panel host, encrypted with an independent custodian age public key; backup results are recorded only after the tool actually completes.',
  '节点自查尚未就绪，请确认设备在线并已升级至自带诊断能力的当前 Agent 版本。': 'Node self-check is not ready yet. Confirm the device is online and upgraded to a current Agent release with built-in diagnostics.',
  '执行前校对受控执行工具摘要、固定版本、许可和平台。登记记录不代表工具已就绪或实机验收已通过。': 'Verify controlled tool digests, pinned versions, licenses, and platforms before execution. Recording an entry does not mean the tool is ready or live-host acceptance has passed.',
  '使用面板受控固定版本的 lego，向 Cloudflare 与 Let\'s Encrypt 发起真实请求。账户材料与私钥以加密凭据保存；不会自动部署到目标服务。默认使用预发布签发环境。': 'Uses a panel-controlled pinned version of lego to send real requests to Cloudflare and Let\'s Encrypt. Account material and private keys are stored as encrypted credentials, and certificates are never auto-deployed to target services. Staging is selected by default.',
  '固定 lego 版本': 'Pinned lego version',
  '填写受控固定版本号，不能使用 latest': 'Enter the controlled pinned version, never latest',
  '执行组件摘要：': 'Component digest: ',
  '诊断按执行工具版本、组件摘要、参数和环境摘要分组。仅核对同条件原始报告，不生成统一分数、供应商排名或超售结论。': 'Diagnostics are grouped by tool version, component digest, parameter digest, and environment digest. Only raw reports under identical conditions are compared, without unified scores, provider rankings, or overselling claims.',
  '当前筛选中没有可配置的服务器，请明确选择后再创建。': 'No configurable servers match the current filter. Select a server explicitly before creating.',
  '已选节点所属服务器不存在或为只读接管服务器；当前草稿已保留。': 'The server for the selected node does not exist or is a read-only adopted host; your draft is kept.',
  '已选服务器不存在或为只读接管服务器，请明确重新选择；当前草稿已保留。': 'The selected server does not exist or is a read-only adopted host. Please reselect explicitly; your draft is kept.',
  '保存后自动合并生成目标配置并下发，在线 Agent 会自动准备运行时、应用配置并回报健康状态；离线设备会在重连后自动同步。': 'Saved changes are merged into the target configuration and dispatched automatically. Online Agents prepare the built-in runtime, apply the configuration, and report health; offline devices sync automatically upon reconnecting.',
  '当前尚无可用于配置代理服务的普通服务器，请先添加服务器。': 'There are no ordinary servers available to configure proxy services yet. Add a server first.',
  '当前筛选的服务器不存在或为只读接管服务器，暂时不能在此服务器下创建节点或链路。': 'The filtered server does not exist or is a read-only adopted host, so nodes or chains cannot be created under it.',
  '前往服务器': 'Go to servers',
  '查看全部服务器': 'View all servers',
  '查看部署状态': 'View deployment status',
  '目标配置已保存并自动加入发布队列，等待目标服务器 Agent 自动应用并回报健康状态。': 'Target configuration saved and queued for automatic publication, waiting for the target server Agent to apply it and report health.',
  '目标配置已保存；该节点尚未授权给任何代理用户，暂不会监听端口，实际应用等待设备回执。': 'Target configuration saved. Because this node is not authorized to any proxy user yet, it will not listen on a port; actual application awaits device acknowledgement.',
  '已选服务器不存在或为只读接管服务器（原选择保留）': 'Selected server does not exist or is a read-only adopted host (selection kept)',
  '指定服务器不存在': 'Specified server does not exist',
  '目标服务器不存在或为只读接管服务器，请明确重新选择；当前草稿已保留。': 'The target server does not exist or is a read-only adopted host. Please reselect explicitly; your draft is kept.',
  '已选节点已不可用，或所属服务器不存在、为只读接管服务器，请重新选择；名称和其他草稿已保留。': 'The selected node is unavailable, or its server does not exist or is a read-only adopted host. Please reselect; the name and other draft fields are kept.',
  '已选入口服务器不存在或为只读接管服务器，请重新选择；草稿已保留。': 'The selected entry server does not exist or is a read-only adopted host. Please reselect; your draft is kept.',
  '已选入口服务器不存在或为只读接管服务器；草稿已保留。': 'The selected entry server does not exist or is a read-only adopted host; your draft is kept.',
  '当前筛选的入口服务器不存在或为只读接管服务器，原草稿已保留。': 'The filtered entry server does not exist or is a read-only adopted host; your draft is kept.',
  '先添加可配置代理服务的普通服务器，再创建代理节点与链路。': 'Add an ordinary server that can configure proxy services first, then create proxy nodes and chains.',
  '面板策略尚未允许这台服务器做只读环境预检；预检只读取本机目录与监听状态，不修改服务器。': 'Panel policy does not allow read-only environment preflight on this server yet. Preflight only inspects local directories and listener state without modifying the server.',
  '只读部署环境预检': 'Read-only deployment environment preflight',
  '的目标配置正在等待合并发布与设备应用。': 'target configuration is waiting for merged publication and device application.',
  '按需核对入口域名的解析地址，并向 Agent 只读查询运行时目录、服务权限与监听情况。普通节点保存后自动发布并由 Agent 应用，此预检不阻塞发布。': 'Checks DNS resolution of entry domains on demand and queries the Agent read-only for runtime directories, service permissions, and listeners. Ordinary nodes publish and apply automatically after saving; this preflight never blocks publication.',
  '最近一次环境预检通过': 'Latest environment preflight passed',
  '证据已过期，可重新采集': 'Evidence expired; collect again if needed',
  '尚未采集环境预检': 'Environment preflight not collected yet',
  '采集部署环境预检': 'Collect deployment environment preflight',
  '已登记只读预检请求，等待 Agent 实际采样；排队不表示已通过。': 'Read-only preflight request recorded, waiting for actual Agent sampling; queued does not mean passed.',
  '请在部署排查时留意此项。': 'Check this item when troubleshooting deployments.',
  '需要面板和 Agent 都启用 runtime_inspection 才能采集本机目录与监听详情。': 'Both the panel and the Agent must enable runtime_inspection to collect local directory and listener details.',
  '配置等待合并发布': 'Configuration waiting for merged publication',
  '内置运行时版本：': 'Built-in runtime version: ',
  '已请求目标 Agent 核对实际配置文件、内置运行时和受管进程；等待回执。': 'Requested the target Agent to verify the actual configuration file, built-in runtime, and managed process; waiting for acknowledgement.',
  '配置发布历史': 'Configuration publication history',
  '节点迁移与显式备选链路': 'Node migration and explicit failover chains',
  '的代理服务应用或运行失败': 'proxy service failed to apply or run',
  '安装自带能力的 Agent 并确认服务器在线。监控、资产和日常运维都在「服务器」中管理。': 'Install the self-contained Agent and confirm the server is online. Monitoring, assets, and daily operations live under Servers.',
  '节点是某台服务器上的代理入口；链路把多台服务器或订阅中的节点串成一条线路。保存后在线 Agent 会自动准备运行时并应用配置。': 'A node is a proxy entry on a server; a chain connects nodes across servers or subscriptions into one route. Once saved, the online Agent prepares the runtime and applies configuration automatically.',
  '服务器在线、已配置代理服务运行正常、告警均已恢复。': 'Servers are online, configured proxy services are running normally, and all alerts are resolved.',
  '服务器在线、已配置代理服务运行正常、告警均已恢复，30 天内也没有待续费的项目。': 'Servers are online, configured proxy services are running normally, all alerts are resolved, and nothing is due for renewal within 30 days.',
  '圆点大小表示本期流量，虚线红圈表示 Agent 离线；点击服务器查看它的网络服务状态。': 'Dot size shows traffic this cycle and a dashed red ring marks an offline Agent. Click a server to inspect its network services.',
  '仅监控': 'monitor-only',
  '先添加服务器': 'Add a server first',
  '添加服务器并接入 Agent；需要提供代理时直接创建节点，在线 Agent 会自动启动并应用服务。': 'Add a server and enroll its Agent; create nodes directly whenever proxy service is needed and the online Agent will start and apply the service automatically.',
  '重新应用已有失败部署': 'Reapply existing failed deployment',
  '由一体化 Agent 使用内置运行时重新应用设备已回报失败的当前固定部署，不生成新业务配置。需要在线 Agent、运行时运维和实际配置核对能力。': 'Reapplies the pinned deployment that the device reported as failed using the self-contained Agent built-in runtime, without generating new service configuration. Requires an online Agent with runtime operations and configuration checkpoint capabilities.',
  '配置修订': 'Configuration revision',
  '失败修订': 'Failed revision',
  '不更改业务配置': 'Does not change service configuration',
  '设备上报 Agent 版本：': 'Device-reported Agent version: ',
  '设备上报运行时版本：': 'Device-reported runtime version: ',
  '未上报': 'Not reported',
  '配置包摘要：': 'Bundle digest: ',
  '绑定摘要：': 'Binding digest: ',
  '保留原请求和实际结果。先重新读取受管运行时实例与精确配置，再由管理员核对；原部署不会重新提交，人工核对后停止后续步骤。': 'Keeps the original request and actual outcome. Re-read the managed runtime instance and exact configuration first for administrator review; the original deployment is not resubmitted and subsequent steps stop after manual reconciliation.',
  '服务端仍须确认本次回执与原固定部署及当前受管实例匹配。失败、过期或不同部署的回执不能解锁。': 'The panel still verifies that this receipt matches the original pinned deployment and current managed instance. Failed, expired, or mismatched receipts cannot unlock reconciliation.',
  '固定失败部署步骤的每个目标都须在线并声明运行时运维与精确配置核对能力。': 'Every target of a pinned failed-deployment step must be online and declare runtime operations and exact configuration checkpoint capabilities.',
  '缺少运行时运维或精确核对能力': 'Missing runtime operations or exact checkpoint capability',
  '仅保存操作类型、受管服务名、备份计划引用与固定目标，不包含凭据或命令。跨窗口保存会核对草稿版本；失败保留当前输入。': 'Saves only operation types, managed service names, backup schedule references, and pinned targets, never credentials or commands. Cross-tab saves check draft revisions and preserve current input on failure.',
  '先选择在线且支持运行时运维与实际配置核对的目标，才可选择固定失败部署步骤。': 'Select online targets supporting runtime operations and configuration checkpoints before choosing a pinned failed-deployment step.',
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
      if (!translated) { sources?.delete(attribute); continue }
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
    // One pass restores the Chinese source text after switching back; the source
    // language needs no observer, which would otherwise walk the whole document
    // after every render.
    translateStaticDom(locale)
    if (locale !== 'en-US') return
    const observer = new MutationObserver(() => {
      translateStaticDom(locale)
      // Drop the records caused by this pass's own writes.
      observer.takeRecords()
    })
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

export function LanguageSelect({ className = '', onChoose }: { className?: string; onChoose?: (locale: Locale) => void }) {
  const { locale, setLocale, t } = useI18n()
  return <label className={cn('relative inline-flex min-w-0 items-center', className)}>
    <span className="sr-only">{t('界面语言')}</span>
    <LanguagesIcon className="text-muted-foreground pointer-events-none absolute left-2.5 z-10 size-4" aria-hidden="true" />
    <NativeSelect aria-label={t('界面语言')} value={locale} onChange={event => { const next = event.target.value as Locale; setLocale(next); onChoose?.(next) }} className="min-w-32 *:data-[slot=native-select]:pl-8">
      <option value="zh-CN">简体中文</option>
      <option value="en-US">English</option>
    </NativeSelect>
  </label>
}
