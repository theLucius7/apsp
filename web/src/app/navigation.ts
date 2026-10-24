import { BellRingIcon, BlocksIcon, BoxesIcon, CalendarClockIcon, ChartColumnIcon, CloudCogIcon, GaugeIcon, GlobeIcon, KeyRoundIcon, LayersIcon, LayoutDashboardIcon, NetworkIcon, ServerIcon, Settings2Icon, ShieldCheckIcon, SlidersHorizontalIcon, SquareTerminalIcon, TimerIcon, UsersIcon, WaypointsIcon, WorkflowIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { AppRoute } from './routes'

export type NavigationItem = {
  page: AppRoute['page']
  path: string
  label: string
  icon: LucideIcon
  group: string
}

type NavigationGroup = { label: string; items: readonly Omit<NavigationItem, 'group'>[] }

// Groups follow what the administrator is doing, not how a feature is built:
// a plugin's pages sit in the group of the task they serve, and "扩展" only holds
// plugin management and plugins without a better home. See docs/product-map.md.
// The proxy network comes first, in the order its tasks build on each other
// (overview, nodes and chains, users, policies and packages); servers,
// monitoring, network tools and costs support it. Addresses never change with
// the order, and the page shown after sign-in is a separate decision (D6).
const groups: readonly NavigationGroup[] = [
  { label: '代理服务', items: [
    { page: 'singbox-overview', path: '/plugins/sing-box', label: '服务概览', icon: BoxesIcon },
    { page: 'nodes', path: '/plugins/sing-box/nodes', label: '代理节点', icon: WaypointsIcon },
    { page: 'proxy-users', path: '/plugins/sing-box/users', label: '代理用户', icon: UsersIcon },
    { page: 'groups', path: '/plugins/sing-box/groups', label: '策略与套餐', icon: LayersIcon },
    { page: 'commerce', path: '/plugins/sing-box/commerce', label: '成员运营', icon: UsersIcon },
  ] },
  { label: '总览', items: [
    { page: 'dashboard', path: '/dashboard', label: '服务器看板', icon: LayoutDashboardIcon },
    { page: 'statistics', path: '/statistics', label: '统计报表', icon: ChartColumnIcon },
    // ADR 0087: costs and renewals sit with the overview; the published path is kept.
    { page: 'expenses', path: '/plugins/expenses', label: '费用与续费', icon: CalendarClockIcon },
  ] },
  { label: '服务器', items: [
    { page: 'servers', path: '/servers', label: '服务器列表', icon: ServerIcon },
    { page: 'fleet', path: '/fleet', label: '运维工具', icon: SquareTerminalIcon },
    { page: 'operations', path: '/operations', label: '自动化与恢复', icon: WorkflowIcon },
  ] },
  { label: '监控告警', items: [
    { page: 'notifications', path: '/system/notifications', label: '告警事件', icon: BellRingIcon },
    { page: 'latency', path: '/latency', label: '延迟检测', icon: TimerIcon },
    { page: 'alert-settings', path: '/monitoring/settings', label: '告警设置', icon: SlidersHorizontalIcon },
  ] },
  { label: '网络', items: [
    { page: 'network-workbench', path: '/network-workbench', label: '网络检测', icon: GaugeIcon },
    { page: 'network-configuration', path: '/network-configuration', label: '网络配置', icon: NetworkIcon },
    { page: 'ddns', path: '/plugins/ddns', label: '动态域名解析', icon: GlobeIcon },
  ] },
  { label: '扩展', items: [
    { page: 'catalog', path: '/plugins/catalog', label: '插件中心', icon: BlocksIcon },
    { page: 'alicloud', path: '/plugins/alicloud', label: '阿里云 CDT', icon: CloudCogIcon },
  ] },
  { label: '系统', items: [
    { page: 'settings', path: '/system/settings', label: '面板设置', icon: Settings2Icon },
    { page: 'control-center', path: '/system/control-center', label: '管理与安全', icon: ShieldCheckIcon },
    { page: 'security', path: '/system/administrator', label: '账号安全', icon: KeyRoundIcon },
  ] },
]

export const navigation: readonly NavigationItem[] = groups.flatMap(group => group.items.map(item => ({ ...item, group: group.label })))

// Workspaces opened from a server keep their own sidebar entry; every other
// server section belongs to the server list.
const serverWorkspaces = new Set<string>(['ddns', 'fleet', 'network-workbench', 'network-configuration', 'operations'])

export function currentNavigation(route: AppRoute): NavigationItem | undefined {
  const page = route.page === 'server' ? (serverWorkspaces.has(route.section ?? '') ? route.section : 'servers')
    // Plugin enablement is the second view of the plugin centre.
    : route.page === 'plugins' ? 'catalog'
      : route.page
  return navigation.find(item => item.page === page)
}
