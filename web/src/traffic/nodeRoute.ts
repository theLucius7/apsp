import type { ProxyResourceFilter, ProxyResourceServerRole } from '../access/groupTypes'

/** Node page sections, selected by the optional view parameter. */
export type NodeView = 'catalog' | 'chains' | 'sources' | 'runtimes'

type NodeRoute = {
  serverId?: number
  serverRole?: ProxyResourceServerRole
  chains: boolean
  kind: ProxyResourceFilter
  view?: NodeView
  resource?: { kind: 'direct' | 'chain'; id: number }
}

const parameters = ['server', 'kind', 'role', 'view']

export function nodeRoute(path: string): NodeRoute | null {
  const detail = path.match(/^\/nodes\/(direct|chain)\/([1-9]\d*)$/)
  if (detail) return Number.isSafeInteger(Number(detail[2])) ? { kind: 'all', chains: false, resource: { kind: detail[1] as 'direct' | 'chain', id: Number(detail[2]) } } : null
  const [pathname, query = '', ...extra] = path.split('?')
  if (pathname !== '/nodes' || extra.length) return null
  const params = new URLSearchParams(query)
  if ([...params.keys()].some(key => !parameters.includes(key)) || parameters.some(key => params.getAll(key).length > 1)) return null
  const server = params.get('server'), kind = params.get('kind')
  if (kind !== null && !['all', 'direct', 'chains'].includes(kind) || server !== null && (!/^[1-9]\d*$/.test(server) || !Number.isSafeInteger(Number(server)))) return null
  const role = params.get('role')
  if (role !== null && !['any', 'entry', 'middle', 'exit'].includes(role)) return null
  const view = params.get('view')
  if (view !== null && !['catalog', 'chains', 'sources', 'runtimes'].includes(view)) return null
  return {
    ...(server === null ? {} : { serverId: Number(server) }),
    ...(role === null ? {} : { serverRole: role as ProxyResourceServerRole }),
    chains: kind === 'chains',
    kind: (kind ?? 'all') as ProxyResourceFilter,
    ...(view === null ? {} : { view: view as NodeView }),
  }
}

/** Builds a node page hash. Without a view, the chains filter opens the chain section and other filters the catalog. */
export function nodeHash({ server, kind = 'all', role = 'any', view }: { server?: string; kind?: ProxyResourceFilter; role?: ProxyResourceServerRole; view?: NodeView }) {
  const params = new URLSearchParams()
  if (kind !== 'all') params.set('kind', kind)
  if (server) params.set('server', server)
  if (role !== 'any') params.set('role', role)
  if (view && view !== (kind === 'chains' ? 'chains' : 'catalog')) params.set('view', view)
  return `#/nodes${params.size ? `?${params}` : ''}`
}
