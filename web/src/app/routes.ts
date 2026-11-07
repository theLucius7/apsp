import { dashboardRoute } from '../display/dashboard'
import { canonicalPath } from './paths'
import { nodeRoute } from '../plugins/singbox/nodeRoute'
import { resourceRoute } from '../plugins/singbox/resourceTypes'
import type { ResourceKey } from '../plugins/singbox/resourceTypes'
import type { NodeView } from '../plugins/singbox/nodeRoute'

// Sections of one server; ddns, fleet, network and operations are workspaces
// opened with this server preselected.
type ServerSection = 'assets' | 'agent' | 'ip-info' | 'node-quality' | 'tcp-quality' | 'plugins' | 'ddns' | 'fleet' | 'network-workbench' | 'network-configuration' | 'operations'
type SimplePage = 'singbox-overview' | 'servers' | 'statistics' | 'latency' | 'latency-reachability' | 'alicloud' | 'ddns'
  | 'subscription-board' | 'groups' | 'plugins' | 'catalog' | 'settings' | 'backup' | 'batch-enrollment' | 'alert-settings' | 'notifications' | 'security' | 'fleet' | 'network-workbench' | 'network-configuration' | 'operations' | 'control-center' | 'not-found'

export type AppRoute =
  | { page: 'dashboard'; serverId?: number }
  | { page: 'member-login' }
  | { page: 'member-join'; token?: string }
  | { page: 'proxy-portal'; account: string; activation?: string }
  | { page: 'server'; serverId: number; section?: ServerSection }
  | { page: 'proxy-users'; userId?: number }
  | { page: 'expenses'; item?: number; server?: number }
  | { page: 'nodes'; serverId?: number; chains?: boolean; selected?: ResourceKey; kind?: 'direct'; serverRole?: 'any' | 'entry' | 'middle' | 'exit'; view?: NodeView }
  | { page: SimplePage }

const pages: Readonly<Record<string, SimplePage>> = {
  '/servers': 'servers',
  '/servers/batch': 'batch-enrollment',
  '/fleet': 'fleet',
  '/network-workbench': 'network-workbench',
  '/network-configuration': 'network-configuration',
  '/operations': 'operations',
  '/settings/admin': 'control-center',
  '/home': 'singbox-overview',
  '/statistics': 'statistics',
  '/subscriptions': 'subscription-board',
  '/latency': 'latency',
  '/latency/reachability': 'latency-reachability',
  '/alerts/settings': 'alert-settings',
  '/alicloud': 'alicloud',
  '/dns': 'ddns',
  '/groups': 'groups',
  '/settings/plugins': 'catalog',
  '/settings/plugins/servers': 'plugins',
  '/settings': 'settings',
  '/settings/backup': 'backup',
  '/alerts': 'notifications',
  '/settings/account': 'security',
}

export function resolveRoute(address: string): AppRoute {
  // Addresses published before the rename resolve to the same pages.
  const path = canonicalPath(address)
  if (path === '/member') return { page: 'member-login' }
  if (path === '/join') return { page: 'member-join' }
  const join = path.match(/^\/join\/([A-Za-z0-9_-]{43}|[A-Za-z0-9]{4}(?:-?[A-Za-z0-9]{4}){3})$/)
  if (join) return { page: 'member-join', token: join[1] }
  const portal = path.match(/^\/account\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})(?:\?activate=([A-Za-z0-9_-]{43}))?$/)
  if (portal) return { page: 'proxy-portal', account: portal[1], activation: portal[2] }
  const display = dashboardRoute(path)
  if (display) return { page: 'dashboard', ...display }

  const server = path.match(/^\/servers\/([1-9]\d*)(?:\/(assets|agent|ip-info|node-quality|tcp-quality|plugins|ddns|fleet|network-workbench|network-configuration|operations))?$/)
  if (server && Number.isSafeInteger(Number(server[1]))) {
    return { page: 'server', serverId: Number(server[1]), section: server[2] as ServerSection | undefined }
  }

  if (path === '/users') return { page: 'proxy-users' }
  const proxyUser = path.match(/^\/users\/([1-9]\d*)$/)
  if (proxyUser && Number.isSafeInteger(Number(proxyUser[1]))) return { page: 'proxy-users', userId: Number(proxyUser[1]) }

  // One entry of the costs and renewals ledger opens its detail.
  if (path === '/expenses') return { page: 'expenses' }
  const expense = path.match(/^\/expenses\/(items|servers)\/([1-9]\d*)$/)
  if (expense && Number.isSafeInteger(Number(expense[2]))) return { page: 'expenses', ...(expense[1] === 'items' ? { item: Number(expense[2]) } : { server: Number(expense[2]) }) }

  const node = nodeRoute(path)
  if (node) {
    if (node.resource) return { page: 'nodes', selected: node.resource }
    return { page: 'nodes', ...(node.serverId === undefined ? {} : { serverId: node.serverId }), chains: node.chains,
      ...(node.kind === 'direct' ? { kind: 'direct' as const } : {}), ...(node.serverRole ? { serverRole: node.serverRole } : {}),
      ...(node.view ? { view: node.view } : {}) }
  }
  const resource = resourceRoute(path)
  if (resource) return { page: 'nodes', selected: resource }

  return { page: Object.hasOwn(pages, path) ? pages[path] : 'not-found' }
}
