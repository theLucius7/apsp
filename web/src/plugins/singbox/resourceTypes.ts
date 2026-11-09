/** A managed node or a chain, as addressed by `#/nodes/{kind}/{id}`. */
export type ResourceKey = { kind: 'direct' | 'chain'; id: number }

export const roleNames = { direct: '直连节点', managed_hop: '受管内部段' }
export const resourceLink = ({ kind, id }: ResourceKey) => `#/nodes/${kind}/${id}`
export function resourceRoute(path: string): ResourceKey | null {
  const match = path.match(/^\/nodes\/(direct|chain)\/([1-9]\d*)$/)
  return match && Number.isSafeInteger(Number(match[2])) ? { kind: match[1] as ResourceKey['kind'], id: Number(match[2]) } : null
}
export const endpoint = (host: string, port: number) => `${host.includes(':') ? `[${host.replace(/^\[|\]$/g, '')}]` : host}:${port}`
