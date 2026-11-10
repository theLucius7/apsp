import { resourceWriteError } from '../hooks'
import { assignmentRequestId, proxyResourceKey, validProxyResources } from '../access/groupTypes'
import type { ProxyResource, ResourceSnapshot } from '../access/groupTypes'
import { sourceRequest } from './sourceRequests'
import { sourceUuid } from './orderedSourceTypes'

export type ChainMutation = { kind: 'chain'; id: number; settings_revision: number; operation: 'edit' | 'versions'; fields: Record<string, unknown> }
export type PendingChainMutation = { command: string; serialized: string; request_id: string; attempted: boolean; path: string }
export type ChainMutationReceipt = { request_id: string; kind: 'direct' | 'chain'; id: number; settings_revision: number; generation: number | null }
function validateCommand(command: ChainMutation) {
  if (command.kind !== 'chain' || !Number.isSafeInteger(command.id) || command.id <= 0 || !Number.isSafeInteger(command.settings_revision) || command.settings_revision < 1) throw new Error('链路标识或设置版本无效。')
  const fields = command.fields
  if (!fields || typeof fields !== 'object' || Array.isArray(fields)) throw new Error('公开修改字段不完整。')
  if (command.operation === 'edit') {
    if (!Object.keys(fields).length || Object.keys(fields).some(key => !['name', 'entry_name', 'public_host', 'port', 'sni'].includes(key))) throw new Error('只允许修改名称与公开入口参数，不能修改拓扑或秘密。')
    for (const key of ['name', 'entry_name', 'public_host', 'sni']) if (fields[key] !== undefined && (typeof fields[key] !== 'string' || !(fields[key] as string).trim() || /[\u0000-\u001f\u007f]/.test(fields[key] as string))) throw new Error('公开文本字段无效。')
    if (fields.port !== undefined && (!Number.isSafeInteger(fields.port) || Number(fields.port) < 1 || Number(fields.port) > 65535 || fields.port === 18085)) throw new Error('入口端口无效。')
  } else if (command.operation === 'versions') {
    if (Object.keys(fields).sort().join(',') !== 'generation,versions' || !Number.isSafeInteger(fields.generation) || Number(fields.generation) < 1 || !Array.isArray(fields.versions) || !fields.versions.length || fields.versions.length > 8) throw new Error('版本更新须绑定当前路径代数与明确跳位置。')
    const positions = new Set<number>()
    for (const version of fields.versions) {
      if (!version || typeof version !== 'object' || Object.keys(version).sort().join(',') !== 'hop_position,node_version_id' || !Number.isSafeInteger(version.hop_position) || version.hop_position < 1 || version.hop_position > 8 || !sourceUuid(version.node_version_id) || positions.has(version.hop_position)) throw new Error('版本更新不能重复位置或携带其他节点身份及秘密。')
      positions.add(version.hop_position)
    }
  } else throw new Error('链路操作无效。')
}
export function chainMutationError(snapshot: ResourceSnapshot<ProxyResource[]>, command?: ChainMutation, replay = false) {
  const error = resourceWriteError(snapshot)
  if (error || !validProxyResources(snapshot.data)) return error || '资源列表格式尚未确认，草稿已保留。'
  if (!command) return ''
  const resource = snapshot.data.find(resource => proxyResourceKey(resource) === `${command.kind}:${command.id}`)
  if (!resource) return '所选链路已不可用，请刷新确认；原请求和草稿已保留。'
  const endpointEligible = (endpoint: ProxyResource['entry']) => endpoint.enabled && endpoint.plugin_enabled && !endpoint.node_deleted && !endpoint.server_deleted
  if (!resource.available || !endpointEligible(resource.entry) || resource.hops.some(hop => hop.kind === 'managed' ? !endpointEligible(hop.endpoint) : hop.source_archived || !hop.node_present)) return '链路目标或依赖已不可用，暂不能发送修改或重试；原请求和草稿已保留。'
  if (replay) return ''
  if (resource.settings_revision !== command.settings_revision) return '链路设置已变化；请确认原操作后重新打开编辑。草稿已保留。'
  if (command.operation === 'versions' && resource.path_state?.desired_generation !== command.fields.generation) return '路径代数已变化，请先确认候选与已应用版本；原请求已保留。'
  return ''
}
export function prepareChainMutation(command: ChainMutation, snapshot: ResourceSnapshot<ProxyResource[]>, previous?: PendingChainMutation, id = assignmentRequestId) {
  validateCommand(command)
  const fingerprint = JSON.stringify(command)
  const replay = previous?.attempted && previous.command === fingerprint
  const error = chainMutationError(snapshot, command, Boolean(replay))
  if (error) throw new Error(error)
  if (replay) return previous!
  if (!Number.isSafeInteger(command.id) || command.id <= 0 || !Number.isSafeInteger(command.settings_revision) || command.settings_revision < 1) throw new Error('链路标识或设置版本无效。')
  const request_id = id()
  if (!sourceUuid(request_id)) throw new Error('请求标识无效。')
  const path = `/api/plugins/sing-box/ordered-proxy-resources/chain/${command.id}${command.operation === 'versions' ? '/apply-node-versions' : ''}`
  return { command: fingerprint, serialized: JSON.stringify({ request_id, settings_revision: command.settings_revision, ...command.fields }), request_id, attempted: false, path }
}
export async function submitChainMutation(pending: PendingChainMutation, snapshot: ResourceSnapshot<ProxyResource[]>, writer: (path: string, method: string, body: unknown) => Promise<unknown> = sourceRequest): Promise<ChainMutationReceipt> {
  const command = JSON.parse(pending.command) as ChainMutation
  validateCommand(command)
  const expected = JSON.stringify({ request_id: pending.request_id, settings_revision: command.settings_revision, ...command.fields })
  const path = `/api/plugins/sing-box/ordered-proxy-resources/chain/${command.id}${command.operation === 'versions' ? '/apply-node-versions' : ''}`
  if (!sourceUuid(pending.request_id) || pending.serialized !== expected || pending.path !== path || command.kind !== 'chain' || !['edit', 'versions'].includes(command.operation)) throw new Error('原请求不完整，不能重试。')
  const error = chainMutationError(snapshot, command, pending.attempted)
  if (error) throw new Error(error)
  pending.attempted = true
  const result = await writer(path, command.operation === 'versions' ? 'POST' : 'PATCH', JSON.parse(pending.serialized)) as ChainMutationReceipt
  if (!result || Object.keys(result).sort().join(',') !== 'generation,id,kind,request_id,settings_revision' || result.request_id !== pending.request_id || result.kind !== command.kind || result.id !== command.id
    || !Number.isSafeInteger(result.settings_revision) || result.settings_revision < command.settings_revision || result.generation !== null && (!Number.isSafeInteger(result.generation) || result.generation < 1)) throw new Error('面板操作收据尚未确认；原键与内容已保留，可原样重试。')
  return result
}
