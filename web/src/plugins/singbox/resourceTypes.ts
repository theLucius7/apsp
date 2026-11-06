import type { Node } from '../../types'
import type { SubscriptionHopReference } from './sourceTypes'

export type ResourceKey = { kind: 'direct' | 'chain'; id: number }
export type ProxyResource = ResourceKey & {
  name: string; server_id: number; server_name: string; public_host: string; port: number; protocol: string
  enabled: boolean; available: boolean; role: 'direct' | 'managed_hop' | 'chain_entry'; entry_node_id: number | null
  tcp: boolean; udp: boolean; legacy: boolean; active_generation: number | null; pending_generation: number | null
  managed_server_ids?: number[]; managed_middle_server_ids?: number[]; managed_exit_server_ids?: number[]
  minimum_generation: number; stage: string; last_error: string | null; reference_count: number; entry_eligible?: boolean
}
export type HopView = {
  position: number; kind: 'managed' | 'subscription'; node_id: number; server_id: number | null
  source_id: number | null; version_id: number | null; update_mode: 'follow_node' | 'pinned' | null
  name: string; protocol: string; server: string; port: number; present: boolean; latest_version_id: number | null
}
export type MixedConversion = { state: 'preparing' | 'switched' | 'completed' | 'reverted'; mixed_generation: number; ordered_generation: number; attempts: number; started_at: number; switched_at: number | null; finished_at: number | null; last_error: string | null }
export type ConversionCheck = { chain_id: number; ready: boolean; reasons: string[]; messages: string[]; mixed_generation: number | null; ordered_generation: number | null }
export type ResourceDetail = { resource: ProxyResource; node: Node; hops: HopView[]; versions: { generation: number; stage: string; last_error: string | null; created_at: number }[]; conversion?: MixedConversion | null }
export type HopInput = { kind: 'managed'; node_id: number } | SubscriptionHopReference
export type EntryInput = { mode: 'new'; server_id: number; public_host: string; sni: string; port: number | null } | { mode: 'existing'; node_id: number }
export type ChainInput = { name: string; entry: EntryInput; hops: HopInput[] }
export type ChainReceipt = { request_id: string; chain_ids: number[]; entry_node_ids: number[] }

export const roleNames = { direct: '直连节点', managed_hop: '受管内部段', chain_entry: '独立链路入口' }
const stages: Record<string, string> = {
  direct: '直连配置', active: '已应用', waiting_dependencies: '等待受管段配置', preparing_entry: '准备入口配置',
  checking_candidate: '验证候选路径', switching_entry: '切换入口', checking_active: '验证切换结果',
  establishing_barrier: '确认恢复边界', retiring_old: '清理旧版本', rolling_back: '恢复原版本',
  failed: '应用失败', retired: '旧版本已退出',
}
export const stageName = (stage: string) => stages[stage] ?? '等待设备确认'
export const resourceLink = ({ kind, id }: ResourceKey) => `#/nodes/${kind}/${id}`
export function resourceRoute(path: string): ResourceKey | null {
  const match = path.match(/^\/nodes\/(direct|chain)\/([1-9]\d*)$/)
  return match && Number.isSafeInteger(Number(match[2])) ? { kind: match[1] as ResourceKey['kind'], id: Number(match[2]) } : null
}
export const endpoint = (host: string, port: number) => `${host.includes(':') ? `[${host.replace(/^\[|\]$/g, '')}]` : host}:${port}`

export function flatResourceInScope(resource: ProxyResource, serverId?: number, role: 'any' | 'entry' | 'middle' | 'exit' = 'any') {
  if (serverId === undefined) return true
  if (resource.kind === 'direct' || role === 'entry') return resource.server_id === serverId
  const ids = role === 'middle' ? resource.managed_middle_server_ids : role === 'exit' ? resource.managed_exit_server_ids : resource.managed_server_ids
  if (ids === undefined) return role === 'any' && resource.server_id === serverId
  return Array.isArray(ids) && ids.every(id => Number.isSafeInteger(id) && id > 0) && ids.includes(serverId)
}
