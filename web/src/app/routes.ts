import { dashboardRoute } from '../observability/display/dashboard'
import { nodeRoute } from '../traffic/nodeRoute'
import { resourceRoute } from '../traffic/resourceTypes'
import type { ResourceKey } from '../traffic/resourceTypes'
import type { NodeView } from '../traffic/nodeRoute'
import { pageEntries } from './navigation'
import type { PageName } from './navigation'

// Sections of one server; ddns, fleet, network and operations are workspaces
// opened with this server preselected.
type ServerSection = 'assets' | 'agent' | 'ip-info' | 'node-quality' | 'tcp-quality' | 'services' | 'ddns' | 'fleet' | 'network-workbench' | 'network-configuration' | 'operations'
// Pages whose address carries parameters are parsed here; every other page is found
// through the address its domain declares (`<domain>/pages.tsx`).
type ParameterizedPage = 'dashboard' | 'proxy-users' | 'expenses' | 'nodes'
type SimplePage = Exclude<PageName, ParameterizedPage> | 'not-found'

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

type SimpleEntry = Extract<(typeof pageEntries)[number], { page: Exclude<SimplePage, 'not-found'> }>
const parameterized = new Set<string>(['dashboard', 'proxy-users', 'expenses', 'nodes'] satisfies ParameterizedPage[])
const isSimple = (entry: (typeof pageEntries)[number]): entry is SimpleEntry => !parameterized.has(entry.page)
const pages = new Map<string, SimplePage>(pageEntries.filter(isSimple)
  .flatMap(entry => [entry.path, ...('aliases' in entry ? entry.aliases : [])].map(path => [path, entry.page] as const)))

export function resolveRoute(path: string): AppRoute {
  if (path === '/member') return { page: 'member-login' }
  if (path === '/join') return { page: 'member-join' }
  const join = path.match(/^\/join\/([A-Za-z0-9_-]{43}|[A-Za-z0-9]{4}(?:-?[A-Za-z0-9]{4}){3})$/)
  if (join) return { page: 'member-join', token: join[1] }
  const portal = path.match(/^\/account\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})(?:\?activate=([A-Za-z0-9_-]{43}))?$/)
  if (portal) return { page: 'proxy-portal', account: portal[1], activation: portal[2] }
  const display = dashboardRoute(path)
  if (display) return { page: 'dashboard', ...display }

  // `plugins` is the network services tab's address until 2026-11-09 (ADR 0131); kept for one release.
  const server = path.match(/^\/servers\/([1-9]\d*)(?:\/(assets|agent|ip-info|node-quality|tcp-quality|services|plugins|ddns|fleet|network-workbench|network-configuration|operations))?$/)
  if (server && Number.isSafeInteger(Number(server[1]))) {
    const section = server[2] === 'plugins' ? 'services' : server[2]
    return { page: 'server', serverId: Number(server[1]), section: section as ServerSection | undefined }
  }
  // The plugin centre's runtime list until 2026-11-09 is now a view of the network services workspace.
  if (path === '/settings/plugins/servers') return { page: 'nodes', view: 'runtimes' }

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

  return { page: pages.get(path) ?? 'not-found' }
}
