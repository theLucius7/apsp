import type { roleNames } from './resourceTypes'
import { isInteger, isPort, isRecord } from '@/lib/validate'

/** One row of the node catalog: a managed node or an adopted external node. */
export type CatalogNode = {
  kind: 'direct' | 'external'; id: number; name: string; role: keyof typeof roleNames | 'external'
  server_id: number | null; server_name: string | null; protocol: string | null; public_host: string | null; port: number | null
  enabled: boolean; available: boolean; tcp: boolean; udp: boolean; stage: string; last_error: string | null; reference_count: number
  original_name: string; tags: string[]; note: string; sort_order: number; revision: string
  source_id?: number; source_name?: string; version_id?: number | null; identity_epoch?: number
}
export const catalogKey = (node: Pick<CatalogNode, 'kind' | 'id'>) => `${node.kind}:${node.id}`
export const catalogRef = (node: CatalogNode) => ({ kind: node.kind, id: node.id, revision: node.revision })
export type CatalogFilter = { search: string; kind: string; protocol: string; tag: string; status: string; source: string; server: string; role: string }
export function filterCatalog(nodes: CatalogNode[], filter: CatalogFilter): CatalogNode[] {
  const query = filter.search.trim().toLocaleLowerCase()
  return nodes.filter(node => (!filter.kind || node.kind === filter.kind)
    && (!filter.protocol || node.protocol === filter.protocol) && (!filter.tag || node.tags.includes(filter.tag))
    && (!filter.source || node.source_id === Number(filter.source)) && catalogInScope(node, filter.server)
    && (!filter.role || node.role === filter.role)
    && (!filter.status || (filter.status === 'disabled' ? !node.enabled : filter.status === 'available' ? node.enabled && node.available : node.enabled && !node.available))
    && (!query || [node.name, node.original_name, node.public_host ?? '', node.note, ...node.tags].join(' ').toLocaleLowerCase().includes(query)))
}
export function renamed(name: string, mode: string, value: string, replacement = ''): string {
  return (mode === 'prefix' ? value + name : mode === 'suffix' ? name + value : value ? name.split(value).join(replacement) : name).trim()
}
export function parsedTags(value: string): string[] { return [...new Set(value.split(/[,，\n]/).map(tag => tag.trim()).filter(Boolean))] }
export function tagsError(tags: string[]): string { return tags.length > 16 || tags.some(tag => new TextEncoder().encode(tag).length > 64) ? '最多 16 个标签，每个标签不超过 64 字节。' : '' }

const text = (value: unknown) => typeof value === 'string'
export const validCatalogRevision = (value: unknown): value is string => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value)
export function validCatalogNode(value: unknown): value is CatalogNode {
  if (!isRecord(value) || !['direct', 'external'].includes(String(value.kind)) || !isInteger(value.id, 1) || !validCatalogRevision(value.revision) || !['name', 'original_name', 'note', 'stage'].every(key => text(value[key])) || !Array.isArray(value.tags) || !value.tags.every(text) || !isInteger(value.sort_order, Number.MIN_SAFE_INTEGER) || !['enabled', 'available', 'tcp', 'udp'].every(key => typeof value[key] === 'boolean') || !isInteger(value.reference_count)) return false
  if (!['protocol', 'public_host'].every(key => value[key] === null || text(value[key])) || !(value.port === null || isPort(value.port))) return false
  if (value.kind === 'external') return value.role === 'external' && value.server_id === null && value.server_name === null && isInteger(value.source_id, 1) && text(value.source_name) && isInteger(value.identity_epoch, 1) && (value.version_id === null || isInteger(value.version_id, 1))
  return ['direct', 'managed_hop'].includes(String(value.role)) && isInteger(value.server_id, 1) && text(value.server_name) && text(value.protocol) && text(value.public_host) && isInteger(value.port, 1)
}
export function validCatalog(value: unknown): value is CatalogNode[] {
  return Array.isArray(value) && value.every(validCatalogNode) && new Set(value.map(catalogKey)).size === value.length
}
/** A managed node belongs to the server it listens on; external nodes belong to no server. */
export function catalogInScope(node: CatalogNode, server: string) {
  if (!server) return true
  return node.kind !== 'external' && Number.isSafeInteger(Number(server)) && Number(server) > 0 && node.server_id === Number(server)
}
export function catalogMutationError(current: CatalogNode[] | undefined, frozen: CatalogNode[], server: string) {
  if (!current || !validCatalog(current) || !frozen.length || frozen.some(node => !validCatalogNode(node))) return '节点信息尚未确认或修订号不完整；当前草稿已保留。'
  if (frozen.some(node => { const row = current.find(value => catalogKey(value) === catalogKey(node)); return !row || row.revision !== node.revision || !catalogInScope(row, server) })) return '所选节点已更新、不存在或离开当前服务器范围；请重新确认，当前草稿已保留。'
  return ''
}
