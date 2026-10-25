import { BellRingIcon, BlocksIcon, CalendarClockIcon, ChartColumnIcon, CloudCogIcon, DatabaseBackupIcon, GaugeIcon, GlobeIcon, HandshakeIcon, HouseIcon, KeyRoundIcon, LayersIcon, LayoutDashboardIcon, NetworkIcon, ServerIcon, Settings2Icon, ShieldCheckIcon, SlidersHorizontalIcon, TimerIcon, UsersIcon, WaypointsIcon, WorkflowIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { AppRoute } from './routes'
import type { Tier } from './tiers'

export type NavigationItem = {
  page: AppRoute['page']
  path: string
  label: string
  icon: LucideIcon
  group: string
  /** Set for downgraded features, which stay usable but are labelled. */
  tier?: Tier
}

type NavigationGroup = { label: string; tier?: Tier; items: readonly Omit<NavigationItem, 'group' | 'tier'>[] }

// Groups follow what the administrator is doing, in the order of the main
// journey: see the whole network, run the proxy service, manage the servers
// it runs on, watch alerts and line quality, then the panel itself. Plugin
// pages sit with the task they serve. Downgraded workspaces are collected in
// the last group and labelled; addresses never change with the order. See
// docs/product-map.md and ADR 0098.
const groups: readonly NavigationGroup[] = [
  { label: '总览', items: [
    { page: 'singbox-overview', path: '/plugins/sing-box', label: '概览', icon: HouseIcon },
    { page: 'dashboard', path: '/dashboard', label: '服务器看板', icon: LayoutDashboardIcon },
    { page: 'statistics', path: '/statistics', label: '统计报表', icon: ChartColumnIcon },
    // ADR 0087: costs and renewals sit with the overview; the published path is kept.
    { page: 'expenses', path: '/plugins/expenses', label: '费用与续费', icon: CalendarClockIcon },
  ] },
  { label: '代理服务', items: [
    { page: 'nodes', path: '/plugins/sing-box/nodes', label: '代理节点', icon: WaypointsIcon },
    { page: 'proxy-users', path: '/plugins/sing-box/users', label: '代理用户', icon: UsersIcon },
    { page: 'groups', path: '/plugins/sing-box/groups', label: '策略与套餐', icon: LayersIcon },
    { page: 'commerce', path: '/plugins/sing-box/commerce', label: '成员运营', icon: HandshakeIcon },
  ] },
  { label: '服务器', items: [
    { page: 'servers', path: '/servers', label: '服务器列表', icon: ServerIcon },
    { page: 'catalog', path: '/plugins/catalog', label: '插件中心', icon: BlocksIcon },
    { page: 'ddns', path: '/plugins/ddns', label: '动态域名解析', icon: GlobeIcon },
    { page: 'alicloud', path: '/plugins/alicloud', label: '阿里云 CDT', icon: CloudCogIcon },
  ] },
  { label: '监控告警', items: [
    { page: 'notifications', path: '/system/notifications', label: '告警事件', icon: BellRingIcon },
    { page: 'alert-settings', path: '/monitoring/settings', label: '告警设置', icon: SlidersHorizontalIcon },
    { page: 'latency', path: '/latency', label: '延迟检测', icon: TimerIcon },
    { page: 'network-workbench', path: '/network-workbench', label: '网络检测', icon: GaugeIcon },
  ] },
  { label: '系统', items: [
    { page: 'settings', path: '/system/settings', label: '面板设置', icon: Settings2Icon },
    { page: 'backup', path: '/system/backup', label: '备份与恢复', icon: DatabaseBackupIcon },
    { page: 'control-center', path: '/system/control-center', label: '管理与安全', icon: ShieldCheckIcon },
    { page: 'security', path: '/system/administrator', label: '账号安全', icon: KeyRoundIcon },
  ] },
  { label: '实验功能', tier: 'experimental', items: [
    { page: 'operations', path: '/operations', label: '自动化与维护', icon: WorkflowIcon },
    { page: 'network-configuration', path: '/network-configuration', label: '网络配置', icon: NetworkIcon },
  ] },
]

export const navigation: readonly NavigationItem[] = groups.flatMap(group => group.items.map(item => ({ ...item, group: group.label, ...(group.tier ? { tier: group.tier } : {}) })))

/** The page an administrator lands on after signing in. */
export const homePath = '/plugins/sing-box'

// Workspaces opened from a server keep their own sidebar entry; every other
// server section belongs to the server list. The operations tools and batch
// enrollment moved into the server pages, so their old addresses do too.
const serverWorkspaces = new Set<string>(['ddns', 'network-workbench', 'network-configuration', 'operations'])
const serverPages = new Set<string>(['fleet', 'batch-enrollment'])

export function currentNavigation(route: AppRoute): NavigationItem | undefined {
  const page = route.page === 'server' ? (serverWorkspaces.has(route.section ?? '') ? route.section : 'servers')
    // Plugin enablement is the second view of the plugin centre.
    : route.page === 'plugins' ? 'catalog'
      : serverPages.has(route.page) ? 'servers'
        : route.page
  return navigation.find(item => item.page === page)
}
