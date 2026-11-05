import { BellRingIcon, BlocksIcon, CalendarClockIcon, ChartColumnIcon, CloudCogIcon, DatabaseBackupIcon, GaugeIcon, GlobeIcon, HouseIcon, KeyRoundIcon, LayersIcon, LayoutDashboardIcon, NetworkIcon, ServerIcon, Settings2Icon, ShieldCheckIcon, SlidersHorizontalIcon, TimerIcon, UsersIcon, WaypointsIcon, WorkflowIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { AppRoute } from './routes'
import type { Tier } from './tiers'

export type NavigationItem = {
  page: AppRoute['page']
  path: string
  /** The page's own name: its title and its tab. */
  label: string
  icon: LucideIcon
  group: string
  /** The sidebar entry this page is a tab of; pages without one are sidebar entries. */
  parent?: string
  /** The sidebar's name for an entry whose tabs go by more specific names. */
  navLabel?: string
  /** Set for downgraded features, which stay usable but are labelled. */
  tier?: Tier
}

type NavigationGroup = { label: string; tier?: Tier; items: readonly Omit<NavigationItem, 'group' | 'tier'>[] }

// Twelve entries in four groups, in the order of the main journey: see the whole
// network, run the proxy service, manage the servers it runs on, watch alerts and
// lines and set up the panel. Pages about the same thing are tabs of one entry
// instead of separate entries. Downgraded workspaces stay hidden until the panel
// settings show them. Addresses never change. See docs/product-map.md and ADR 0102.
const settingsPath = '/system/settings'
const groups: readonly NavigationGroup[] = [
  { label: '总览', items: [
    { page: 'singbox-overview', path: '/plugins/sing-box', label: '概览', icon: HouseIcon },
    { page: 'statistics', path: '/statistics', label: '统计报表', icon: ChartColumnIcon, parent: '/plugins/sing-box' },
    // ADR 0087: costs and renewals sit with the overview; the published path is kept.
    { page: 'expenses', path: '/plugins/expenses', label: '费用与续费', icon: CalendarClockIcon },
  ] },
  { label: '代理服务', items: [
    { page: 'nodes', path: '/plugins/sing-box/nodes', label: '代理节点', icon: WaypointsIcon },
    { page: 'proxy-users', path: '/plugins/sing-box/users', label: '代理用户', icon: UsersIcon },
    { page: 'groups', path: '/plugins/sing-box/groups', label: '策略与套餐', icon: LayersIcon },
  ] },
  { label: '服务器', items: [
    { page: 'servers', path: '/servers', label: '服务器列表', navLabel: '服务器', icon: ServerIcon },
    { page: 'dashboard', path: '/dashboard', label: '服务器看板', icon: LayoutDashboardIcon, parent: '/servers' },
    { page: 'ddns', path: '/plugins/ddns', label: '域名解析', icon: GlobeIcon },
    { page: 'alicloud', path: '/plugins/alicloud', label: '阿里云 CDT', icon: CloudCogIcon },
  ] },
  { label: '监控与设置', items: [
    { page: 'notifications', path: '/system/notifications', label: '告警事件', navLabel: '告警', icon: BellRingIcon },
    { page: 'alert-settings', path: '/monitoring/settings', label: '告警设置', icon: SlidersHorizontalIcon, parent: '/system/notifications' },
    { page: 'network-workbench', path: '/network-workbench', label: '网络检测', navLabel: '线路检测', icon: GaugeIcon },
    { page: 'latency', path: '/latency', label: '延迟检测', icon: TimerIcon, parent: '/network-workbench' },
    { page: 'settings', path: settingsPath, label: '面板设置', navLabel: '设置', icon: Settings2Icon },
    { page: 'catalog', path: '/plugins/catalog', label: '插件中心', icon: BlocksIcon, parent: settingsPath },
    { page: 'backup', path: '/system/backup', label: '备份与恢复', icon: DatabaseBackupIcon, parent: settingsPath },
    { page: 'control-center', path: '/system/control-center', label: '管理与安全', icon: ShieldCheckIcon, parent: settingsPath },
    { page: 'security', path: '/system/administrator', label: '账号安全', icon: KeyRoundIcon, parent: settingsPath },
  ] },
  { label: '实验功能', tier: 'experimental', items: [
    { page: 'operations', path: '/operations', label: '自动化与维护', icon: WorkflowIcon },
    { page: 'network-configuration', path: '/network-configuration', label: '网络配置', icon: NetworkIcon },
  ] },
]

export const navigation: readonly NavigationItem[] = groups.flatMap(group => group.items.map(item => ({ ...item, group: group.label, ...(group.tier ? { tier: group.tier } : {}) })))

const familyPath = (item: NavigationItem) => item.parent ?? item.path

/** The sidebar entries among the pages this administrator may open, each going to the first page it may open. */
export function sidebarEntries(pages: readonly NavigationItem[]) {
  const entries: { root: NavigationItem; href: string; path: string }[] = []
  for (const page of pages) {
    const path = familyPath(page)
    if (entries.some(entry => entry.path === path)) continue
    entries.push({ root: navigation.find(item => item.path === path) ?? page, href: page.path, path })
  }
  return entries
}

/** The tabs of the entry a page belongs to; a page on its own has none. */
export function sectionTabs(current: NavigationItem | undefined, pages: readonly NavigationItem[]) {
  if (!current) return []
  const tabs = pages.filter(page => familyPath(page) === familyPath(current))
  return tabs.length > 1 ? tabs : []
}

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
      // China reachability is the second view of the recurring line checks.
      : route.page === 'latency-reachability' ? 'latency'
      : serverPages.has(route.page) ? 'servers'
        : route.page
  return navigation.find(item => item.page === page)
}
