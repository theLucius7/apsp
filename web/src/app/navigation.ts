import { ActivityIcon, BellRingIcon, BlocksIcon, BoxesIcon, ChartColumnIcon, CloudCogIcon, GaugeIcon, GlobeIcon, LayoutDashboardIcon, NetworkIcon, PackageSearchIcon, ServerCogIcon, ServerIcon, ShieldCheckIcon, SlidersHorizontalIcon, TimerIcon, UserCogIcon, UsersIcon, WaypointsIcon, WrenchIcon } from 'lucide-react'
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

// Groups follow the administrator's tasks: observe, manage servers, run the proxy
// business, extend servers with plugins, then configure the panel itself.
const groups: readonly NavigationGroup[] = [
  { label: '概览', items: [
    { page: 'statistics', path: '/statistics', label: '统计仪表盘', icon: ChartColumnIcon },
    { page: 'dashboard', path: '/dashboard', label: '服务器看板', icon: LayoutDashboardIcon },
  ] },
  { label: '服务器', items: [
    { page: 'servers', path: '/servers', label: '服务器', icon: ServerIcon },
    { page: 'fleet', path: '/fleet', label: '接入与日常运维', icon: ServerCogIcon },
    { page: 'network-workbench', path: '/network-workbench', label: '网络与验机', icon: GaugeIcon },
    { page: 'network-configuration', path: '/network-configuration', label: '网络与证书', icon: NetworkIcon },
    { page: 'operations', path: '/operations', label: '批量运维与恢复', icon: WrenchIcon },
    { page: 'latency', path: '/latency', label: '延迟检测', icon: TimerIcon },
  ] },
  { label: '代理服务', items: [
    { page: 'singbox-overview', path: '/plugins/sing-box', label: '代理服务', icon: BoxesIcon },
    { page: 'nodes', path: '/plugins/sing-box/nodes', label: '代理节点', icon: WaypointsIcon },
    { page: 'proxy-users', path: '/plugins/sing-box/users', label: '代理用户', icon: UsersIcon },
    { page: 'groups', path: '/plugins/sing-box/groups', label: '策略与套餐', icon: SlidersHorizontalIcon },
  ] },
  { label: '扩展插件', items: [
    { page: 'ddns', path: '/plugins/ddns', label: '动态域名解析', icon: GlobeIcon },
    { page: 'alicloud', path: '/plugins/alicloud', label: '阿里云 CDT', icon: CloudCogIcon },
    { page: 'plugins', path: '/system/plugins', label: '服务器插件', icon: BlocksIcon },
    { page: 'catalog', path: '/plugins/catalog', label: '插件目录', icon: PackageSearchIcon },
  ] },
  { label: '系统', items: [
    { page: 'settings', path: '/system/settings', label: '看板与通知', icon: ActivityIcon },
    { page: 'notifications', path: '/system/notifications', label: '告警通知', icon: BellRingIcon },
    { page: 'control-center', path: '/system/control-center', label: '管理与安全', icon: ShieldCheckIcon },
    { page: 'security', path: '/system/administrator', label: '系统管理员', icon: UserCogIcon },
  ] },
]

export const navigation: readonly NavigationItem[] = groups.flatMap(group => group.items.map(item => ({ ...item, group: group.label })))

export function currentNavigation(route: AppRoute): NavigationItem | undefined {
  const page = route.page === 'server' ? (['ddns', 'fleet', 'network-workbench', 'network-configuration', 'operations'].includes(route.section ?? '') ? route.section : 'servers') : route.page
  return navigation.find(item => item.page === page)
}
