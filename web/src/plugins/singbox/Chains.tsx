import { useCallback, useEffect, useRef, useState } from 'react'
import { api } from '../../api'
import { Field, FormDialog, RefreshNotice, DialogPresence } from '../../components'
import { resourceWriteError, useAction, useRefreshNotice, useResource } from '../../hooks'
import type { Node, PluginServer } from '../../types'
import { assignmentRequestId, proxyResourceKey, validatedSnapshot, validProxyResources } from './groupTypes'
import type { Chain, ProxyResource, ResourceSnapshot } from './groupTypes'
import { sourceRoot, validSourceNodePage, validSubscriptionSources } from './orderedSourceTypes'
import type { SourceNode, SourceNodePage, SubscriptionSource } from './orderedSourceTypes'
import ChainPathEditor from './ChainPathEditor'
import ChainExitExpansion from './ChainExitExpansion'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Notice } from '@/components/panel'

const root = '/api/plugins/sing-box'

type Snapshot<T> = { data?: T[]; fresh: boolean; error: string }
export type ChainWriteSnapshot = { chains: Snapshot<Chain>; nodes: Snapshot<Node>; servers: Snapshot<PluginServer> }
type ChainRequest = { name: string; entry_node_id: number; exit_node_id: number }

export function chainWriteError(snapshot: ChainWriteSnapshot, id?: number) {
  const error = resourceWriteError(snapshot.chains, snapshot.nodes, snapshot.servers)
  if (error) return error
  if (!snapshot.chains.data || !snapshot.nodes.data || !snapshot.servers.data) return '正在刷新相关信息，暂不能修改；草稿已保留。'
  return id !== undefined && !snapshot.chains.data.some(chain => chain.id === id)
    ? '此链路已不可用，暂不能提交。可关闭窗口后重新选择。' : ''
}

function nodeId(value: string) {
  const id = Number(value)
  return /^[1-9]\d*$/.test(value) && Number.isSafeInteger(id) ? id : null
}

export function chainSelectionError(snapshot: ChainWriteSnapshot, entry: string, exit: string) {
  for (const value of [entry, exit].filter(Boolean)) {
    const id = nodeId(value)
    const node = snapshot.nodes.data?.find(node => node.id === id)
    const server = node && snapshot.servers.data?.find(server => server.id === node.server_id)
    if (!node || node.protocol !== 'vless-reality' || node.enabled === false || !server?.enabled) {
      return '已选节点已不可用或所属服务器尚未启用，请重新选择；名称和其他草稿已保留。'
    }
  }
  const entryId = nodeId(entry), exitId = nodeId(exit)
  if (entryId !== null && exitId !== null) {
    const entryNode = snapshot.nodes.data?.find(node => node.id === entryId)
    const exitNode = snapshot.nodes.data?.find(node => node.id === exitId)
    if (entryNode?.server_id === exitNode?.server_id) return '入口和出口必须属于两台不同服务器；草稿已保留。'
  }
  if (snapshot.chains.data?.some(chain => chain.entry_node_id === entryId || chain.exit_node_id === entryId || chain.entry_node_id === exitId)) {
    return '已选节点的链路身份已变更，不支持重复入口、嵌套或循环链路；请重新选择，草稿已保留。'
  }
  return ''
}

export async function createChain(form: FormData, snapshot: ChainWriteSnapshot, write: (request: ChainRequest) => Promise<unknown> = request => api(`${root}/chains`, 'POST', request)) {
  const entry = String(form.get('entry_node_id') ?? ''), exit = String(form.get('exit_node_id') ?? '')
  const error = chainWriteError(snapshot) || chainSelectionError(snapshot, entry, exit)
  if (error) throw new Error(error)
  const entryId = nodeId(entry), exitId = nodeId(exit)
  if (entryId === null || exitId === null) throw new Error('请选择入口节点和出口节点；草稿已保留。')
  const name = String(form.get('name') ?? '').trim()
  if (!name || Array.from(name).length > 128) throw new Error('请填写不超过 128 个字符的名称；草稿已保留。')
  return write({ name, entry_node_id: entryId, exit_node_id: exitId })
}

export async function deleteChain(id: number, snapshot: ChainWriteSnapshot, write: (id: number) => Promise<unknown> = id => api(`${root}/chains/${id}`, 'DELETE')) {
  const error = chainWriteError(snapshot, id)
  if (error) throw new Error(error)
  return write(id)
}

export type ProxyWriteSnapshot = { resources: ResourceSnapshot<ProxyResource[]>; nodes: ResourceSnapshot<Node[]>; servers: ResourceSnapshot<PluginServer[]>; sources?: ResourceSnapshot<SubscriptionSource[]>; sourceNodes?: Record<number, ResourceSnapshot<SourceNodePage>> }
export type ChainDraftHop = { kind: 'managed'; node_id: string } | { kind: 'subscription'; source_id: number; node: Pick<SourceNode, 'id' | 'source_id' | 'identity_epoch' | 'version_id' | 'name' | 'protocol' | 'server' | 'server_port' | 'capabilities'> | null; update_mode: 'follow_node' | 'pinned' }
export type ChainBatchDraft = { mode: 'new' | 'existing'; server_id: string; public_host: string; sni: string; entry_node_id: string; exit_node_id: string; hops?: ChainDraftHop[]; rows: { name: string; port: string; hops?: ChainDraftHop[] }[] }
export type ChainRequestHop = { kind: 'managed'; node_id: number } | { kind: 'subscription'; source_id: number; external_node_id: string; node_version_id: string; update_mode: 'follow_node' | 'pinned' }
export type ChainBatchRequest = { request_id: string; items: { name: string; entry: { mode: 'new'; server_id: number; public_host: string; sni: string; port: number | null } | { mode: 'existing'; node_id: number }; hops: ChainRequestHop[] }[] }
export type PendingChainBatch = { draft: string; source_draft: string; serialized: string; request_id: string; attempted: boolean }
export type ChainBatchResult = { request_id: string; chain_ids: number[]; entry_node_ids: number[] }

/** Path shapes the creation form offers. A template only prefills the shared hops; every hop stays editable, up to 8. */
export type ChainTemplate = 'two-hop' | 'relay' | 'subscription'
const blankManagedHop = (): ChainDraftHop => ({ kind: 'managed', node_id: '' })
const blankSubscriptionHop = (): ChainDraftHop => ({ kind: 'subscription', source_id: 0, node: null, update_mode: 'follow_node' })
/** The template a shared path currently matches; longer managed-only paths are custom. */
export function chainTemplateOf(hops: ChainDraftHop[]): ChainTemplate | 'custom' {
  if (hops.some(hop => hop.kind === 'subscription')) return 'subscription'
  return hops.length === 1 ? 'two-hop' : hops.length === 2 ? 'relay' : 'custom'
}
/** Re-shapes the shared path for a template and keeps the chosen hops that still fit it. */
export function chainTemplateHops(template: ChainTemplate, hops: ChainDraftHop[]): ChainDraftHop[] {
  const managed = hops.filter(hop => hop.kind === 'managed')
  const exit = hops.at(-1)
  if (template === 'two-hop') return [exit?.kind === 'managed' ? exit : blankManagedHop()]
  if (template === 'relay') return [managed.length > 1 ? managed[0] : blankManagedHop(), exit?.kind === 'managed' ? exit : blankManagedHop()]
  // A path that already has a subscription hop keeps it; otherwise the last hop becomes the subscription node.
  return hops.some(hop => hop.kind === 'subscription') ? hops : [...hops.slice(0, -1), blankSubscriptionHop()]
}
/** A new form starts from the two-hop template: the entry plus one exit. */
const blankDraft = (): ChainBatchDraft => ({ mode: 'new', server_id: '', public_host: '', sni: '', entry_node_id: '', exit_node_id: '', hops: chainTemplateHops('two-hop', []), rows: [{ name: '', port: '' }] })

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value)
function validSettings(value: unknown) {
  if (value === undefined) return true
  if (!record(value)) return false
  for (const [key, field] of Object.entries(value)) {
    if (['listen'].includes(key) && typeof field !== 'string') return false
    if (key === 'public_port' && field !== null && (!Number.isSafeInteger(field) || Number(field) < 1 || Number(field) > 65535)) return false
    if (key === 'tcp_fast_open' && typeof field !== 'boolean') return false
    if (key === 'tls_alpn' && (!Array.isArray(field) || !field.every(item => typeof item === 'string'))) return false
    if (['reality', 'hysteria2', 'tuic', 'anytls'].includes(key)) {
      if (!record(field)) return false
      for (const [name, setting] of Object.entries(field)) {
        if (['handshake_server', 'fingerprint', 'congestion_control'].includes(name) && typeof setting !== 'string' && !(name === 'handshake_server' && setting === null)) return false
        if (['ignore_client_bandwidth', 'obfs_enabled', 'zero_rtt_handshake'].includes(name) && typeof setting !== 'boolean') return false
        if (['handshake_port', 'up_mbps', 'down_mbps', 'auth_timeout_seconds', 'heartbeat_seconds', 'idle_session_check_seconds', 'idle_session_timeout_seconds', 'min_idle_session'].includes(name)
          && setting !== null && (!Number.isSafeInteger(setting) || Number(setting) < 0)) return false
      }
    }
  }
  return true
}
export function validNodeList(value: unknown): value is Node[] {
  return Array.isArray(value) && value.every(node => node && Number.isSafeInteger(node.id) && node.id > 0 && Number.isSafeInteger(node.server_id) && node.server_id > 0
    && typeof node.name === 'string' && typeof node.protocol === 'string' && typeof node.public_host === 'string' && typeof node.sni === 'string'
    && Number.isSafeInteger(node.port) && node.port >= 1 && node.port <= 65535 && (node.enabled === undefined || typeof node.enabled === 'boolean') && validSettings(node.settings)
    && (node.protocol_config === undefined || record(node.protocol_config) && typeof node.protocol_config.type === 'string'
      && (node.protocol_config.method === undefined || typeof node.protocol_config.method === 'string')
      && (node.protocol_config.tls === undefined || record(node.protocol_config.tls) && (node.protocol_config.tls.mode === 'acme' || node.protocol_config.tls.mode === 'manual')
        && (node.protocol_config.tls.email === undefined || typeof node.protocol_config.tls.email === 'string')
        && (node.protocol_config.tls.configured === undefined || typeof node.protocol_config.tls.configured === 'boolean')
        && (node.protocol_config.tls.challenge === undefined || node.protocol_config.tls.challenge === 'http-01' || node.protocol_config.tls.challenge === 'tls-alpn-01'))))
    && new Set(value.map(node => node.id)).size === value.length
}
export function validServerList(value: unknown): value is PluginServer[] {
  return Array.isArray(value) && value.every(server => server && Number.isSafeInteger(server.id) && server.id > 0 && typeof server.name === 'string'
    && typeof server.enabled === 'boolean' && typeof server.online === 'boolean' && typeof server.read_only === 'boolean'
    && (server.installation === undefined || record(server.installation) && typeof server.installation.state === 'string' && typeof server.installation.reason === 'string'
      && Number.isSafeInteger(server.installation.target_rev) && Number(server.installation.target_rev) >= 0 && Number.isSafeInteger(server.installation.applied_rev) && Number(server.installation.applied_rev) >= 0))
    && new Set(value.map(server => server.id)).size === value.length
}
export function proxyWriteError(snapshot: ProxyWriteSnapshot, resource?: Pick<ProxyResource, 'kind' | 'id'>) {
  const error = resourceWriteError(snapshot.resources, snapshot.nodes, snapshot.servers)
  if (error) return error
  if (!validProxyResources(snapshot.resources.data) || !validNodeList(snapshot.nodes.data) || !validServerList(snapshot.servers.data)) return '最新资源信息格式不完整，暂不能修改；草稿已保留。'
  return resource && !snapshot.resources.data.some(item => proxyResourceKey(item) === proxyResourceKey(resource)) ? '此资源已不可用，暂不能提交。可关闭窗口后重新选择。' : ''
}
function managedSelectionError(snapshot: ProxyWriteSnapshot, entry: number | null, exit: number | null, entryServer?: number) {
  for (const id of [entry, exit].filter((id): id is number => id !== null)) {
    const node = snapshot.nodes.data?.find(node => node.id === id)
    const server = node && snapshot.servers.data?.find(server => server.id === node.server_id)
    if (!node || node.protocol !== 'vless-reality' || node.enabled === false || !server?.enabled) return '已选节点已不可用或所属服务器尚未启用，请重新选择；名称和其他草稿已保留。'
    if (snapshot.resources.data?.some(resource => resource.kind === 'chain' && resource.entry.id === id)) return '已选节点的链路身份已变更，不支持重复入口、嵌套或循环链路；请重新选择，草稿已保留。'
    const direct = snapshot.resources.data?.find(resource => resource.kind === 'direct' && resource.id === id)
    if (!direct) return '已选节点的链路身份已变更，请重新选择，草稿已保留。'
    if (!direct.available) return '已选节点已不可用，公开配置需要修复；草稿已保留。'
  }
  const entryNode = snapshot.nodes.data?.find(node => node.id === entry)
  const exitNode = snapshot.nodes.data?.find(node => node.id === exit)
  if (exitNode && (entryServer ?? entryNode?.server_id) === exitNode.server_id) return '入口和出口必须属于两台不同服务器；草稿已保留。'
  const entryResource = snapshot.resources.data?.find(resource => resource.kind === 'direct' && resource.id === entry)
  if (entryResource && (entryResource.chain_refs.length || entryResource.policy_group_ids.length || entryResource.user_count)) return '现有入口已有授权或链路引用，不能转为专用入口；请选择未授权且未被引用的节点。'
  return ''
}
export function chainBatchSelectionError(draft: ChainBatchDraft, snapshot: ProxyWriteSnapshot) {
  const error = proxyWriteError(snapshot)
  if (error) return error
  const entry = draft.mode === 'existing' ? nodeId(draft.entry_node_id) : null
  const serverId = draft.mode === 'new' ? nodeId(draft.server_id) : undefined
  if (draft.mode === 'new' && draft.server_id && !snapshot.servers.data?.some(server => server.id === serverId && server.enabled)) return '已选入口服务器已不可用或尚未启用，请重新选择；草稿已保留。'
  if (draft.mode === 'existing' && draft.entry_node_id && entry === null) return '请选择有效的入口；草稿已保留。'
  const entryError = managedSelectionError(snapshot, entry, null, serverId ?? undefined)
  if (entryError) return entryError
  for (const [rowIndex, row] of draft.rows.entries()) {
    const hops = row.hops ?? draft.hops ?? [{ kind: 'managed' as const, node_id: draft.exit_node_id }]
    if (!hops.length || hops.length > 8) return `第 ${rowIndex + 1} 条路径需要 1–8 个入口后的代理跳。`
    const entryServer = serverId ?? snapshot.nodes.data?.find(node => node.id === entry)?.server_id
    const servers = new Set<number>(entryServer ? [entryServer] : [])
    const identities = new Set<string>()
    for (const [index, hop] of hops.entries()) {
      const prefix = `第 ${rowIndex + 1} 条、第 ${index + 1} 跳：`
      if (hop.kind === 'managed') {
        const id = nodeId(hop.node_id)
        if (id === null) return `${prefix}请选择受管节点。`
        const error = managedSelectionError(snapshot, null, id)
        if (error) return prefix + error
        const server = snapshot.nodes.data!.find(node => node.id === id)!.server_id
        if (servers.has(server)) return `${prefix}受管服务器不能与入口相同，也不能在同一路径中重复。`
        servers.add(server)
      } else {
        if (resourceWriteError(snapshot.sources ?? { fresh: false, error: '' }) || !validSubscriptionSources(snapshot.sources?.data)) return `${prefix}来源列表等待最新确认；草稿已保留。`
        const source = snapshot.sources!.data!.find(source => source.id === hop.source_id)
        const page = snapshot.sourceNodes?.[hop.source_id]
        const currentPage = page?.getCurrent ? page.getCurrent() : page?.data
        if (!source || source.archived || !page || resourceWriteError(page) || !validSourceNodePage(currentPage) || currentPage.source_id !== source.id || currentPage.current_identity_epoch !== source.identity_epoch) return `${prefix}来源或当前节点版本等待确认，归档及历史节点不能用于新引用。`
        const selected = hop.node && currentPage.nodes.find(node => node.id === hop.node!.id && node.version_id === hop.node!.version_id)
        if (!selected?.selectable || selected.identity_epoch !== source.identity_epoch || !hop.node || hop.node.identity_epoch !== selected.identity_epoch) return `${prefix}所选节点已缺失、更换代次或产生新版本；请明确重新选点，草稿不会自动替换。`
        if (!['follow_node', 'pinned'].includes(hop.update_mode)) return `${prefix}请选择跟随节点或固定版本。`
      }
      const identity = hop.kind === 'managed' ? `managed:${hop.node_id}` : `source:${hop.source_id}:${hop.node?.id ?? ''}`
      if (identities.has(identity)) return `${prefix}同一路径不能重复引用同一个节点。`
      identities.add(identity)
    }
  }
  return ''
}
function publicHost(value: string) { return value.length <= 253 && !/[\s/?#@\\]/.test(value) && !value.includes('://') && (value.includes(':') ? /^\[?[0-9a-fA-F:]+\]?$/.test(value) : /^[a-zA-Z0-9.-]+$/.test(value)) }
export function prepareChainBatch(draft: ChainBatchDraft, snapshot: ProxyWriteSnapshot, previous?: PendingChainBatch, requestId: () => string = assignmentRequestId): PendingChainBatch {
  const writeError = proxyWriteError(snapshot)
  if (writeError) throw new Error(writeError)
  const error = chainBatchSelectionError(draft, snapshot)
  if (error) throw new Error(error)
  if (previous?.attempted && previous.source_draft === JSON.stringify(draft)) {
    savedBatchRequest(previous)
    return previous
  }
  if (!['new', 'existing'].includes(draft.mode) || draft.rows.length < 1 || draft.rows.length > 32 || draft.mode === 'existing' && draft.rows.length !== 1) throw new Error('新入口批次支持 1–32 条链路；现有入口只能创建一条。')
  const entry = nodeId(draft.entry_node_id), serverId = nodeId(draft.server_id)
  if (draft.mode === 'existing' && entry === null || draft.mode === 'new' && serverId === null) throw new Error('请选择入口服务器或节点；草稿已保留。')
  const host = draft.public_host.trim(), sni = draft.sni.trim()
  if (draft.mode === 'new' && (!publicHost(host) || !sni || sni.length > 253 || !/^[a-zA-Z0-9.-]+$/.test(sni))) throw new Error('请填写有效的公开域名或 IP，以及 Reality 协议域名；不包含协议、端口或路径。')
  const ports = new Set<number>()
  const items: ChainBatchRequest['items'] = draft.rows.map(row => {
    const name = row.name.trim(), port = row.port.trim() ? Number(row.port.trim()) : null
    if (!name || Array.from(name).length > 128) throw new Error('每条链路需填写不超过 128 个字符的名称；草稿已保留。')
    if (draft.mode === 'new' && port !== null) {
      if (!/^[1-9]\d*$/.test(row.port.trim()) || !Number.isSafeInteger(port) || port > 65535 || port === 18085) throw new Error('入口端口需为 1–65535 的整数，18085 为保留端口；留空可自动分配。')
      if (ports.has(port)) throw new Error('同一入口服务器上的批次端口不能重复；自动端口会分别分配。')
      ports.add(port)
    }
    const hops: ChainRequestHop[] = (row.hops ?? draft.hops ?? [{ kind: 'managed' as const, node_id: draft.exit_node_id }]).map(hop => hop.kind === 'managed'
      ? { kind: 'managed', node_id: nodeId(hop.node_id)! }
      : { kind: 'subscription', source_id: hop.source_id, external_node_id: hop.node!.id, node_version_id: hop.node!.version_id, update_mode: hop.update_mode })
    return { name, entry: draft.mode === 'new' ? { mode: 'new', server_id: serverId!, public_host: host, sni, port } : { mode: 'existing', node_id: entry! }, hops }
  })
  const fingerprint = JSON.stringify(items)
  if (previous?.draft === fingerprint && previous.source_draft === JSON.stringify(draft)) return previous
  const request_id = requestId()
  return { draft: fingerprint, source_draft: JSON.stringify(draft), request_id, serialized: JSON.stringify({ request_id, items }), attempted: false }
}
function savedBatchRequest(pending: PendingChainBatch): ChainBatchRequest {
  const request = JSON.parse(pending.serialized) as ChainBatchRequest
  if (!request || request.request_id !== pending.request_id || !/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(request.request_id)
    || !Array.isArray(request.items) || JSON.stringify(request.items) !== pending.draft || request.items.length < 1 || request.items.length > 32
    || JSON.stringify(request) !== pending.serialized) throw new Error('原批次信息不完整，请重新填写草稿。')
  return request
}
async function requestChainBatch(body: ChainBatchRequest) {
  const controller = new AbortController()
  const timer = window.setTimeout(() => controller.abort(), 30000)
  try { return await api(`${root}/chains/ordered-batch`, 'POST', body, controller.signal) }
  catch (error) { if (controller.signal.aborted) throw new Error('提交超时，批次可能已完成；请刷新确认或重试原批次，原请求已保留。'); throw error }
  finally { window.clearTimeout(timer) }
}
export async function submitChainBatch(pending: PendingChainBatch, snapshot: ProxyWriteSnapshot, write: (body: ChainBatchRequest) => Promise<unknown> = requestChainBatch) {
  const error = proxyWriteError(snapshot)
  if (error) throw new Error(error)
  const request = savedBatchRequest(pending)
  for (const item of request.items) {
    const entry = item.entry
    const selection = managedSelectionError(snapshot, entry.mode === 'existing' ? entry.node_id : null, null, entry.mode === 'new' ? entry.server_id : undefined)
    if (selection) throw new Error(selection)
    if (entry.mode === 'new' && !snapshot.servers.data?.some(server => server.id === entry.server_id && server.enabled)) throw new Error('已选入口服务器已不可用或尚未启用；草稿已保留。')
  }
  const selection = chainBatchSelectionError(JSON.parse(pending.source_draft) as ChainBatchDraft, snapshot)
  if (selection) throw new Error(selection)
  pending.attempted = true
  return write(request)
}
export function proxyDeleteError(resources: ResourceSnapshot<ProxyResource[]>, resource: Pick<ProxyResource, 'kind' | 'id'>) {
  const error = resourceWriteError(resources)
  if (error) return error
  if (!validProxyResources(resources.data)) return '最新资源信息格式不完整，暂不能删除。'
  return !resources.data.some(item => proxyResourceKey(item) === proxyResourceKey(resource)) ? '此资源已不可用，暂不能提交。可关闭窗口后重新选择。' : ''
}
export async function deleteProxyResource(resource: Pick<ProxyResource, 'kind' | 'id'>, snapshot: ProxyWriteSnapshot, write: (path: string) => Promise<unknown> = path => api(path, 'DELETE')) {
  const error = proxyDeleteError(snapshot.resources, resource)
  if (error) throw new Error(error)
  return write(`${root}/ordered-proxy-resources/${resource.kind}/${resource.id}`)
}

export default function Chains({ snapshot, serverId, getServerId, refresh, onCreated, onAddSource, replacement, openRequest }: { snapshot: ProxyWriteSnapshot; serverId?: number; getServerId?: () => number | undefined; refresh: () => void; onCreated: (result: ChainBatchResult) => void; onAddSource?: () => void; replacement?: { generation: number; resource: ProxyResource }; openRequest?: number }) {
  const action = useAction()
  const [creating, setCreating] = useState(false)
  const [draft, setDraft] = useState<ChainBatchDraft>(blankDraft)
  const pending = useRef<PendingChainBatch | undefined>(undefined)
  const [submittedDraft, setSubmittedDraft] = useState('')
  const [lastRequestId, setLastRequestId] = useState('')
  const [replacementBlocked, setReplacementBlocked] = useState(false)
  const [replacementDraft, setReplacementDraft] = useState(false)
  const replacementSeen = useRef(0)
  useEffect(() => {
    if (!replacement || replacementSeen.current === replacement.generation || action.busy || proxyWriteError(snapshot)) return
    replacementSeen.current = replacement.generation
    if (pending.current?.attempted) { setReplacementBlocked(true); setCreating(true); return }
    setReplacementBlocked(false); setReplacementDraft(true)
    const resource = replacement.resource
    setDraft({ mode: 'new', server_id: String(resource.entry.server_id), public_host: resource.entry.public_host, sni: resource.entry.sni, entry_node_id: '', exit_node_id: '', rows: [{ name: `${resource.name} · 替代`, port: '' }], hops: resource.hops.map(hop => hop.kind === 'managed' ? { kind: 'managed', node_id: String(hop.node_id) } : { kind: 'subscription', source_id: hop.source_id, update_mode: hop.update_mode, node: { id: hop.external_node_id, source_id: hop.source_id, identity_epoch: hop.identity_epoch, version_id: hop.node_version_id, name: hop.name, protocol: hop.protocol, server: hop.server, server_port: hop.server_port, capabilities: hop.capabilities } }) })
    pending.current = undefined; setSubmittedDraft(''); setLastRequestId(''); setCreating(true)
  }, [replacement, action.busy, snapshot])
  const sourceQuery = useResource<unknown>(creating ? sourceRoot : null)
  const sourceHistory = useRef<SubscriptionSource[] | undefined>(undefined)
  const sources = validatedSnapshot(sourceQuery, validSubscriptionSources, sourceHistory.current)
  if (sources.fresh) sourceHistory.current = sources.data
  const [sourceNodes, setSourceNodes] = useState<Record<number, ResourceSnapshot<SourceNodePage>>>({})
  const observe = useCallback((id: number, value: ResourceSnapshot<SourceNodePage>) => setSourceNodes(previous => ({ ...previous, [id]: value })), [])
  const selectedSources = [...new Set([...(draft.hops ?? []), ...draft.rows.flatMap(row => row.hops ?? [])].flatMap(hop => hop.kind === 'subscription' && hop.source_id > 0 ? [hop.source_id] : []))]
  const writeSnapshot: ProxyWriteSnapshot = { ...snapshot, sources, sourceNodes }
  const enabledServers = snapshot.servers.data?.filter(server => server.enabled) ?? []
  const choices = snapshot.nodes.data?.filter(node => node.protocol === 'vless-reality' && node.enabled !== false && enabledServers.some(server => server.id === node.server_id)
    && snapshot.resources.data?.some(resource => resource.kind === 'direct' && resource.id === node.id && resource.available)) ?? []
  // Managed segments must be VLESS + Reality nodes, so enabled nodes of other protocols never appear in the selectors.
  const otherProtocolNodes = snapshot.nodes.data?.filter(node => node.protocol !== 'vless-reality' && node.enabled !== false && enabledServers.some(server => server.id === node.server_id)).length ?? 0
  const template = chainTemplateOf(draft.hops ?? [])
  const filterError = () => {
    const selected = getServerId ? getServerId() : serverId
    if (selected === undefined) return ''
    if (!snapshot.servers.data?.some(server => server.id === selected && server.enabled)) return '当前筛选的入口服务器已不存在或未启用，原草稿已保留。'
    const entryServer = draft.mode === 'new' ? Number(draft.server_id) : snapshot.nodes.data?.find(node => node.id === Number(draft.entry_node_id))?.server_id
    return (draft.mode === 'new' ? Boolean(draft.server_id) : Boolean(draft.entry_node_id)) && entryServer !== selected ? '原入口不属于当前服务器筛选，请明确重新选择；草稿已保留。' : ''
  }
  const writeError = proxyWriteError(snapshot) || filterError()
  const unchanged = submittedDraft === JSON.stringify(draft)
  const selectionError = writeError || chainBatchSelectionError(draft, writeSnapshot)
  const update = (value: Partial<ChainBatchDraft>) => { action.clearError(); setDraft(current => ({ ...current, ...value })) }
  const rowUpdate = (index: number, value: Partial<ChainBatchDraft['rows'][number]>) => update({ rows: draft.rows.map((row, i) => i === index ? { ...row, ...value } : row) })
  // A click made before a refresh visibly disables the buttons is reported, not ignored.
  const refreshNotice = useRefreshNotice()
  // A stale or malformed snapshot blocks opening; a draft that conflicts with the server filter opens with its
  // reason shown and submission disabled, so the administrator can re-select instead of seeing nothing happen.
  const open = () => {
    if (action.busy || !refreshNotice.allows(proxyWriteError(snapshot))) return
    action.clearError()
    if (!draft.server_id) { const selected = getServerId ? getServerId() : serverId; setDraft(current => ({ ...current, server_id: (selected === undefined ? enabledServers[0] : enabledServers.find(server => server.id === selected))?.id.toString() ?? '' })) }
    setCreating(true)
  }
  // The page header and the empty chain list each raise the request counter; every increment opens the form once.
  const openedRequest = useRef(openRequest ?? 0)
  useEffect(() => {
    if (!openRequest || openRequest === openedRequest.current) return
    openedRequest.current = openRequest
    open()
  })
  const submit = () => {
    if (!creating || action.busy || proxyWriteError(snapshot) || filterError() || chainBatchSelectionError(draft, writeSnapshot)) return
    void action.run(async () => {
      const filter = filterError(); if (filter) throw new Error(filter)
      const submission = prepareChainBatch(draft, writeSnapshot, pending.current)
      pending.current = submission
      setSubmittedDraft(JSON.stringify(draft)); setLastRequestId(submission.request_id)
      const result = await submitChainBatch(submission, writeSnapshot) as ChainBatchResult
      const ids = (value: unknown): value is number[] => Array.isArray(value) && value.length === draft.rows.length && value.every(id => Number.isSafeInteger(id) && id > 0) && new Set(value).size === value.length
      if (!result || result.request_id !== submission.request_id || !ids(result.chain_ids) || !ids(result.entry_node_ids)) throw new Error('面板返回的批次结果不完整，请刷新确认或重试原批次；原请求已保留。')
      return result
    }, result => { pending.current = undefined; setSubmittedDraft(''); setLastRequestId(''); setReplacementBlocked(false); setReplacementDraft(false); setCreating(false); setDraft(blankDraft()); onCreated(result); refresh() })
  }
  return <>
    {creating && selectedSources.map(id => <SourceObservation key={id} id={id} observe={observe} />)}
    {refreshNotice.visible && <RefreshNotice />}
    <DialogPresence>{creating && <FormDialog wide title="创建链路" onClose={() => setCreating(false)} onSubmit={submit} busy={action.busy} disabled={Boolean(writeError)} submitDisabled={Boolean(selectionError)} error={writeError || selectionError || action.error} retry={writeError || selectionError ? refresh : undefined} submitLabel={lastRequestId && unchanged ? '重试原批次' : '创建未授权链路'}>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">入口固定为受管 Reality；入口后可按顺序选择 1–8 个受管或订阅节点，最后一跳为出口。一次批量提交全部创建或全部拒绝。完整路径承载与运行时能力由面板在提交时校验，不以解析或 TCP/UDP 字段推断连通。</p>
      <Field label="链路模板" hint={`模板只预填入口之后的跳数，之后仍可增删、调整顺序，最多 8 跳。${draft.rows.some(row => row.hops) ? '已单独编辑的路径不随模板变化。' : ''}`}><NativeSelect className="w-full" value={template} onChange={event => { const next = event.target.value; if (next === 'two-hop' || next === 'relay' || next === 'subscription') update({ hops: chainTemplateHops(next, draft.hops ?? []) }) }}><option value="two-hop">入口 → 落地（两跳）</option><option value="relay">入口 → 中转 → 落地</option><option value="subscription">含订阅来源节点</option>{template === 'custom' && <option value="custom" disabled>自定义路径（{draft.hops?.length ?? 0} 跳）</option>}</NativeSelect></Field>
      <Field label="入口方式"><NativeSelect className="w-full" name="entry_mode" value={draft.mode} onChange={event => update({ mode: event.target.value as 'new' | 'existing', rows: event.target.value === 'existing' ? draft.rows.slice(0, 1) : draft.rows })}><option value="new">新建 Reality 专用入口</option><option value="existing">使用现有未授权入口（单条）</option></NativeSelect></Field>
      <div className="grid gap-x-5 sm:grid-cols-2">
        {draft.mode === 'new' ? <><Field label="入口服务器"><NativeSelect className="w-full" name="server_id" required value={draft.server_id} onChange={event => update({ server_id: event.target.value })}><option value="" disabled>选择入口服务器</option>{draft.server_id && !enabledServers.some(server => String(server.id) === draft.server_id) && <option value={draft.server_id}>服务器 #{draft.server_id}（已不可用）</option>}{enabledServers.map(server => <option key={server.id} value={server.id}>{server.name}{server.online ? ' · 在线' : ' · 离线，等待应用'}</option>)}</NativeSelect></Field><Field label="入口公开地址" hint="同批次共享客户端连接域名或 IP，不含协议、端口和路径。"><Input name="public_host" required value={draft.public_host} onChange={event => update({ public_host: event.target.value })} placeholder="entry.example.com" autoComplete="off" /></Field><Field label="Reality 协议域名" hint="同批次共享 SNI。"><Input name="sni" required value={draft.sni} onChange={event => update({ sni: event.target.value })} placeholder="www.example.com" autoComplete="off" /></Field></> : <Field label="入口节点" hint="只能选择未授权、无链路引用的独立 Reality 节点。"><NativeSelect className="w-full" name="entry_node_id" required value={draft.entry_node_id} onChange={event => update({ entry_node_id: event.target.value })}><option value="" disabled>选择入口</option>{draft.entry_node_id && !choices.some(node => String(node.id) === draft.entry_node_id) && <option value={draft.entry_node_id}>节点 #{draft.entry_node_id}（已不可用，请重新选择）</option>}{choices.map(node => <option key={node.id} value={node.id}>{node.name}（服务器 #{node.server_id}）</option>)}</NativeSelect></Field>}
      </div>
      {draft.hops && <p data-slot="reality-hint" className="text-muted-foreground mt-3 text-xs leading-relaxed">链路中的受管段只能使用 VLESS + Reality 节点；其他协议的节点不会出现在这里。{otherProtocolNodes > 0 && `另有 ${otherProtocolNodes} 个已启用的其他协议节点未列出。`}</p>}
      {draft.hops && <ChainPathEditor label="共享有序路径" hops={draft.hops} onChange={hops => update({ hops })} snapshot={writeSnapshot} />}
      {draft.hops && <ChainExitExpansion draft={draft} snapshot={writeSnapshot} onChange={value => update(value)} />}
      {draft.hops && onAddSource && <Button variant="ghost" size="sm" type="button" onClick={() => { setCreating(false); onAddSource() }}>添加订阅来源并保留链路草稿</Button>}
      {replacementBlocked ? <Notice role="status">先确认已发送批次的结果，原草稿和精确请求已保留。确认后再选择创建替代链路。</Notice> : replacementDraft && <Notice role="status">替代链路将创建新入口与凭据，不继承任何授权。完成后需明确调整策略组引用，等待新路径应用，再清理旧链路；不承诺无损切换。</Notice>}
      <h3 className="mt-3 mb-4 text-sm font-semibold">链路与入口端口 <span className="bg-muted text-muted-foreground min-w-5 rounded px-1.5 text-center text-xs font-medium tabular-nums">{draft.rows.length} / 32</span></h3>
      {draft.rows.map((row, index) => <div data-slot="chain-draft-row" className="grid items-start gap-4 max-md:mb-4 max-md:gap-0 max-md:border-b max-md:pb-3 md:grid-cols-[1fr_10.5rem_auto]" key={index}><Field label={`链路 ${index + 1} 名称`}><Input name={index === 0 ? 'name' : `name_${index}`} required maxLength={128} value={row.name} onChange={event => rowUpdate(index, { name: event.target.value })} autoComplete="off" /></Field>{draft.mode === 'new' && <Field label={`入口端口 ${index + 1}`} hint="留空时分别自动分配；18085 为保留端口。"><Input name={`entry_port_${index}`} type="number" min={1} max={65535} step={1} value={row.port} onChange={event => rowUpdate(index, { port: event.target.value })} placeholder="自动分配" /></Field>}{draft.rows.length > 1 && <Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20 mt-8 max-md:mt-0 max-md:justify-self-start" type="button" onClick={() => update({ rows: draft.rows.filter((_, i) => i !== index) })}>移除第 {index + 1} 条</Button>}</div>)}
      {draft.hops && draft.rows.map((row, index) => <section key={index} className="my-5"><h4 className="text-sm font-medium">第 {index + 1} 条路径</h4>{row.hops ? <><ChainPathEditor label={`第 ${index + 1} 条独立路径`} hops={row.hops} onChange={hops => rowUpdate(index, { hops })} snapshot={writeSnapshot} /><Button variant="ghost" size="sm" type="button" onClick={() => rowUpdate(index, { hops: undefined })}>恢复使用共享路径</Button></> : <Button variant="ghost" size="sm" type="button" onClick={() => rowUpdate(index, { hops: draft.hops!.map(hop => ({ ...hop })) })}>单独编辑第 {index + 1} 条路径</Button>}</section>)}
      {draft.mode === 'new' && <Button type="button" variant="outline" size="sm" disabled={draft.rows.length >= 32} onClick={() => update({ rows: [...draft.rows, { name: '', port: '' }] })}>添加一条链路</Button>}
      <div className="my-5 rounded-lg border p-4" aria-label="批次预览"><h3 className="mt-3 mb-4 text-sm font-semibold">批次预览</h3><p className="text-sm leading-relaxed wrap-anywhere">{draft.rows.length} 条链路，共享入口服务器；{draft.mode === 'new' ? '每条新建独立入口监听，自动端口由面板分别分配。' : '所选现有监听将成为专用入口，不能再单独授权。'}</p><ol className="list-decimal pl-5">{draft.rows.map((row, index) => <li className="text-sm leading-relaxed wrap-anywhere" key={index}><strong>{row.name.trim() || `链路 ${index + 1}`}</strong><span>客户端 → {draft.mode === 'new' ? `${draft.public_host || '待填写地址'}:${row.port || '提交时自动分配'}` : `入口节点 #${draft.entry_node_id || '待选择'}`} → {(row.hops ?? draft.hops ?? [{ kind: 'managed' as const, node_id: draft.exit_node_id }]).map(hop => hop.kind === 'managed' ? `受管 #${hop.node_id || '待选择'}` : `${sources.data?.find(source => source.id === hop.source_id)?.name ?? `来源 #${hop.source_id}`} / ${hop.node?.name ?? '待选择'}（${hop.update_mode === 'pinned' ? '固定版本' : '跟随所选节点'} · ${hop.node?.version_id ?? '无版本'}）`).join(' → ')} → 互联网</span></li>)}</ol></div>
      {lastRequestId && <p className="text-muted-foreground mt-3 text-xs leading-relaxed" role="status">批次编号：<code className="wrap-anywhere">{lastRequestId}</code>。{unchanged ? '请求内容已保留；失败或超时后，目标与依赖仍符合当前资格时可复用同一编号和内容。依赖已消失或身份变化时请先确认原结果，草稿不会自动替换。' : '草稿已修改，下次提交会使用新批次编号；可先刷新列表确认原批次结果。'}关闭窗口后仍可重新打开继续，离开此页面前请先确认提交结果。</p>}
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">创建成功不会自动授权给用户。有序路径先准备全部受管依赖，并验证指定出站后才切用户入口；配置应用、路径探测和第三方持续健康分别记录，失败不会跳过任何一段或改直连。</p>
    </FormDialog>}</DialogPresence>
  </>
}

function SourceObservation({ id, observe }: { id: number; observe: (id: number, value: ResourceSnapshot<SourceNodePage>) => void }) {
  const query = useResource<unknown>(`${sourceRoot}/${id}/nodes`)
  const previous = useRef<SourceNodePage | undefined>(undefined)
  const value = validatedSnapshot(query, validSourceNodePage, previous.current)
  if (value.fresh && value.data?.source_id === id) previous.current = value.data
  useEffect(() => { observe(id, { ...value, isCurrent: () => value.isCurrent?.() === true && value.getCurrent?.()?.source_id === id, getCurrent: () => value.getCurrent?.()?.source_id === id ? value.getCurrent() : undefined, fresh: value.fresh && value.data?.source_id === id }) }, [id, observe, value.data, value.fresh, value.error])
  return null
}
