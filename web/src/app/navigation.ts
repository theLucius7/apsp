import { pages as access } from '../access/pages'
import { pages as costs } from '../costs/pages'
import { pages as governance } from '../governance/pages'
import { pages as naming } from '../naming/pages'
import { pages as networking } from '../networking/pages'
import { pages as observability } from '../observability/pages'
import { pages as operations } from '../operations/pages'
import { pages as resources } from '../resources/pages'
import { pages as traffic } from '../traffic/pages'
import { pages as own } from './pages'
import type { PageEntry, Workspace } from '@/lib/pages'
import type { Tier } from '@/lib/tiers'
import type { AppRoute } from './routes'

/** Every administrator page, as the domains declare them (`<domain>/pages.tsx`). */
export const pageEntries = [...own, ...observability, ...costs, ...traffic, ...access, ...resources, ...naming,
  ...operations, ...networking, ...governance] as const
export type PageName = (typeof pageEntries)[number]['page']

export type NavigationItem = PageEntry & { group: string }

// Twelve entries in four groups, in the order of the main journey: see the whole
// network, run the proxy service, manage the servers it runs on, watch alerts and
// lines and set up the panel. Pages about the same thing are tabs of one entry
// instead of separate entries. Downgraded workspaces stay hidden until the panel
// settings show them. Each page's domain declares its section and position; the
// sections' names and order are decided here. See docs/product-map.md and ADR 0102.
const workspaces: readonly { id: Workspace; label: string; tier?: Tier }[] = [
  { id: 'overview', label: '总览' },
  { id: 'proxy', label: '代理服务' },
  { id: 'servers', label: '服务器' },
  { id: 'settings', label: '监控与设置' },
  { id: 'experimental', label: '实验功能', tier: 'experimental' },
]

export const navigation: readonly NavigationItem[] = workspaces.flatMap(workspace => pageEntries
  .filter(entry => entry.workspace === workspace.id && !('within' in entry))
  .sort((a, b) => a.order - b.order)
  .map(entry => ({ ...entry, group: workspace.label, ...(workspace.tier ? { tier: workspace.tier } : {}) })))

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
export const homePath = '/home'

// Workspaces opened from a server keep their own sidebar entry; every other server
// section belongs to the server list. Pages reached from another page keep that
// page's entry marked.
const serverWorkspaces = new Set<string>(['ddns', 'network-workbench', 'network-configuration', 'operations'])

export function currentNavigation(route: AppRoute): NavigationItem | undefined {
  if (route.page === 'server') {
    const page = serverWorkspaces.has(route.section ?? '') ? route.section : 'servers'
    return navigation.find(item => item.page === page)
  }
  const entry = pageEntries.find(item => item.page === route.page)
  if (entry && 'within' in entry) return navigation.find(item => item.path === entry.within)
  return navigation.find(item => item.page === route.page)
}
