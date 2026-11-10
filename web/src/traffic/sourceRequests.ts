import { api } from '../api'
import { resourceWriteError } from '../hooks'
import { assignmentRequestId } from '../access/groupTypes'
import type { ResourceSnapshot } from '../access/groupTypes'
import { MAX_REFRESH_SECS, MAX_SOURCE_BYTES, MIN_REFRESH_SECS, orderedPreviewError, sourceJobRoot, sourceRoot, sourceUuid, validOrderedPreview, validSourceJob, validSourceNode, validSourceReceipt, validSubscriptionSources } from './orderedSourceTypes'
import type { OrderedPreview, SourceJob, SourceNode, SourceReceipt, SubscriptionSource } from './orderedSourceTypes'

export type SourceEditorMode = 'create' | 'metadata' | 'update' | 'replace' | 'archive' | 'unarchive'
export type SourceDraft = { name: string; kind: 'url' | 'inline'; url: string; content: string; interval: string; authorization: string; cookie: string; apiKey: string; authAction: 'preserve' | 'replace' | 'clear'; userAgent: string; autoRefresh: 'on' | 'off' }
export const emptySourceDraft = (): SourceDraft => ({ name: '', kind: 'url', url: '', content: '', interval: '86400', authorization: '', cookie: '', apiKey: '', authAction: 'preserve', userAgent: '', autoRefresh: 'on' })
export const previewRoot = '/api/plugins/sing-box/ordered-subscription-source-previews'
export type SourceCommand = { mode: SourceEditorMode; source_id?: number; settings_revision?: number; expected_identity_epoch?: number; fields: Record<string, unknown> }
export type PendingSourceMutation = { command: string; path: string; method: 'POST' | 'PATCH'; request_id: string; serialized: string; attempted: boolean }
export type SourceSnapshot = ResourceSnapshot<SubscriptionSource[]>
const encoder = new TextEncoder()
export function sourceMetadataError(snapshot: SourceSnapshot) {
  return resourceWriteError(snapshot) || (!validSubscriptionSources(snapshot.data) ? '来源信息尚未通过格式确认，请刷新来源列表。' : '')
}
export function sourceWriteError(snapshot: SourceSnapshot, sourceId?: number, revision?: number, allowArchived = false) {
  const error = sourceMetadataError(snapshot)
  if (error || sourceId === undefined) return error
  const source = snapshot.data!.find(item => item.id === sourceId)
  if (!source) return '来源已删除或不再可见，请刷新确认。'
  if (revision !== undefined && source.settings_revision !== revision) return '来源设置已变化；草稿仍保留。请先确认上次保存，再选择丢弃草稿并重新读取设置。'
  return source.archived && !allowArchived ? '来源已归档，请先恢复来源。' : ''
}
export function sourceAuthHeaders(draft: Pick<SourceDraft, 'authorization' | 'cookie' | 'apiKey'>) {
  const result: Record<string, string> = {}
  for (const [key, value] of [['authorization', draft.authorization], ['cookie', draft.cookie], ['x-api-key', draft.apiKey]]) {
    if (!value) continue
    if (!value.trim() || /[\r\n\u0000]/.test(value)) throw new Error('认证头不能只有空白，也不能包含换行。')
    result[key] = value
  }
  if (Object.entries(result).reduce((size, [key, value]) => size + encoder.encode(key).length + encoder.encode(value).length, 0) > 8192) throw new Error('认证头合计不能超过 8 KiB。')
  return result
}
function contentText(content: string) {
  if (!content.trim()) throw new Error('请粘贴订阅内容或选择文件。')
  if (encoder.encode(content).length > MAX_SOURCE_BYTES) throw new Error('订阅内容不能超过 2 MiB。')
  return content
}
// Printable ASCII only, as the panel sends it verbatim as the User-Agent header.
export function userAgentText(value: string) {
  const text = value.trim()
  if (!text) return null
  if (text.length > 256 || !/^[\x20-\x7e]+$/.test(text)) throw new Error('请求标识需为 1–256 个可打印 ASCII 字符。')
  return text
}
export function refreshInterval(value: string) {
  const interval = Number(value)
  if (!Number.isSafeInteger(interval) || interval < MIN_REFRESH_SECS || interval > MAX_REFRESH_SECS) throw new Error(`刷新周期需为 ${MIN_REFRESH_SECS}–${MAX_REFRESH_SECS} 秒。`)
  return interval
}
function urlText(value: string) {
  const text = value.trim()
  if (encoder.encode(text).length > 8192) throw new Error('订阅地址不能超过 8 KiB。')
  let url: URL
  try { url = new URL(text) } catch { throw new Error('请填写有效的 HTTPS 订阅地址。') }
  if (url.protocol !== 'https:' || !url.hostname || url.username || url.password || url.hash || /[\r\n\u0000]/.test(text)) throw new Error('订阅地址必须为 HTTPS，不能包含用户凭据、片段或换行。')
  return text
}
export function sourceCommand(mode: SourceEditorMode, draft: SourceDraft, source?: Pick<SubscriptionSource, 'id' | 'settings_revision' | 'identity_epoch' | 'kind'> & Partial<Pick<SubscriptionSource, 'user_agent' | 'auto_refresh'>>): SourceCommand {
  if (mode !== 'create' && !source) throw new Error('未指定来源。')
  const base = mode === 'create' ? {} : { source_id: source!.id, settings_revision: source!.settings_revision, expected_identity_epoch: source!.identity_epoch }
  if (mode === 'archive' || mode === 'unarchive') return { mode, ...base, fields: { archived: mode === 'archive' } }
  const name = draft.name.trim()
  if (!name || [...name].length > 128 || name.includes('://') || /[\u0000-\u001f\u007f-\u009f]/.test(name)) throw new Error('来源名称需为 1–128 个字符，不能包含链接或控制字符。')
  const interval = draft.kind === 'url' ? refreshInterval(draft.interval) : 0
  const agent = draft.kind === 'url' ? userAgentText(draft.userAgent) : null
  const fields: Record<string, unknown> = { name }
  // Changing the User-Agent is a setting; it never starts a new source identity.
  const requestSettings = () => {
    fields.auto_refresh = draft.autoRefresh === 'on'
    if (agent !== (source?.user_agent ?? null)) fields.user_agent = agent === null ? { action: 'clear' } : { action: 'replace', value: agent }
  }
  if (mode === 'create') {
    fields.input = draft.kind === 'url' ? { kind: 'url', url: urlText(draft.url), auth_headers: sourceAuthHeaders(draft) } : { kind: 'inline', content: contentText(draft.content) }
    fields.refresh_interval_secs = draft.kind === 'url' ? interval : null
    if (draft.kind === 'url') { fields.user_agent = agent; fields.auto_refresh = draft.autoRefresh === 'on' }
  } else if (mode === 'metadata') {
    if (source!.kind === 'url') { fields.refresh_interval_secs = interval; requestSettings() }
  } else if (mode === 'update' || mode === 'replace') {
    if (mode === 'update' && (source!.kind !== 'inline' || draft.kind !== 'inline')) throw new Error('只有粘贴或文件来源可以更新同一来源内容。')
    if (draft.kind === 'inline') fields.input = { kind: 'inline', content: contentText(draft.content), identity_action: mode === 'update' ? 'update' : 'replace' }
    else {
      const headers = draft.authAction === 'replace' ? { action: 'replace', value: sourceAuthHeaders(draft) } : draft.authAction === 'clear' ? { action: 'clear' } : null
      const url = draft.url.trim() ? urlText(draft.url) : null
      if (source!.kind === 'inline' && !url) throw new Error('切换为 URL 来源需填写 HTTPS 订阅地址。')
      if (!url && headers === null) throw new Error('请填写新订阅地址，或明确替换或清除认证头。')
      fields.input = { kind: 'url', url, auth_headers: headers }
      fields.refresh_interval_secs = interval
      requestSettings()
    }
  }
  return { mode, ...base, fields }
}
export function sourceCommandError(command: SourceCommand, snapshot: SourceSnapshot, replay = false) {
  if (command.mode !== 'create' && (!Number.isSafeInteger(command.source_id) || Number(command.source_id) < 1
    || !Number.isSafeInteger(command.settings_revision) || Number(command.settings_revision) < 1
    || !Number.isSafeInteger(command.expected_identity_epoch) || Number(command.expected_identity_epoch) < 1)) return '来源身份或设置版本无效，原草稿已保留。'
  const error = sourceWriteError(snapshot, command.source_id, replay ? undefined : command.settings_revision, ['archive', 'unarchive'].includes(command.mode))
  if (error || command.mode === 'create') return error
  const source = snapshot.data!.find(item => item.id === command.source_id)!
  return source.identity_epoch !== command.expected_identity_epoch ? '来源身份代次已变化；原请求和草稿已保留，请先确认上次保存结果。' : ''
}
export function prepareSourceMutation(command: SourceCommand, snapshot: SourceSnapshot, previous?: PendingSourceMutation, id = assignmentRequestId): PendingSourceMutation {
  const fingerprint = JSON.stringify(command)
  const replay = previous?.attempted && previous.command === fingerprint
  const error = sourceCommandError(command, snapshot, Boolean(replay))
  if (error) throw new Error(error)
  if (replay) return previous!
  const request_id = id()
  if (!sourceUuid(request_id)) throw new Error('请求标识无效。')
  const method = command.mode === 'create' ? 'POST' : 'PATCH'
  const path = command.mode === 'create' ? sourceRoot : `${sourceRoot}/${command.source_id}`
  const body = { request_id, ...(command.mode === 'create' ? {} : { settings_revision: command.settings_revision }), ...command.fields }
  return { command: fingerprint, path, method, request_id, serialized: JSON.stringify(body), attempted: false }
}
export const sourceMutationReplay = (command: SourceCommand | undefined, pending?: PendingSourceMutation) => !!command && !!pending?.attempted && pending.command === JSON.stringify(command)
export async function sourceRequest<T>(path: string, method: string, body?: unknown): Promise<T> {
  const controller = new AbortController()
  const timer = window.setTimeout(() => controller.abort(), 30000)
  try { return await api<T>(path, method, body, controller.signal) }
  catch (error) { if (controller.signal.aborted) throw new Error('请求超时，结果尚未确认；原请求已保留，可原样重试。'); throw error }
  finally { window.clearTimeout(timer) }
}
export async function submitSourceMutation(pending: PendingSourceMutation, snapshot: SourceSnapshot, writer: (path: string, method: string, body: unknown) => Promise<unknown> = sourceRequest): Promise<SourceReceipt> {
  let command: SourceCommand, body: Record<string, unknown>
  try { command = JSON.parse(pending.command); body = JSON.parse(pending.serialized) } catch { throw new Error('原请求已损坏，请重新填写。') }
  const expected = { request_id: pending.request_id, ...(command.mode === 'create' ? {} : { settings_revision: command.settings_revision }), ...command.fields }
  if (!sourceUuid(pending.request_id) || JSON.stringify(expected) !== pending.serialized || pending.path !== (command.mode === 'create' ? sourceRoot : `${sourceRoot}/${command.source_id}`) || pending.method !== (command.mode === 'create' ? 'POST' : 'PATCH')) throw new Error('原请求与草稿不一致，请重新填写。')
  const error = sourceCommandError(command, snapshot, pending.attempted)
  if (error) throw new Error(error)
  pending.attempted = true
  const receipt = await writer(pending.path, pending.method, body)
  if (!validSourceReceipt(receipt) || command.source_id !== undefined && receipt.source_id !== command.source_id) throw new Error('保存收据尚未确认，请保留原请求并重试。')
  return receipt
}
export async function refreshSource(snapshot: SourceSnapshot, source: SubscriptionSource, writer = sourceRequest<unknown>): Promise<SourceJob> {
  const error = sourceWriteError(snapshot, source.id, source.settings_revision)
  if (error) throw new Error(error)
  if (source.kind !== 'url') throw new Error('粘贴或文件来源需显式更新内容，不能抓取刷新。')
  const job = await writer(`${sourceRoot}/${source.id}/refresh`, 'POST', { settings_revision: source.settings_revision })
  if (!validSourceJob(job) || job.source_id !== source.id) throw new Error('刷新任务收据尚未确认，请重新读取来源状态。')
  return job
}
export async function deleteSource(snapshot: SourceSnapshot, source: SubscriptionSource, writer = sourceRequest<unknown>) {
  const error = sourceWriteError(snapshot, source.id, source.settings_revision, true)
  if (error) throw new Error(error)
  await writer(`${sourceRoot}/${source.id}`, 'DELETE', { settings_revision: source.settings_revision })
}
export async function cancelSourceJob(snapshot: ResourceSnapshot<SourceJob>, id: string, writer = sourceRequest<unknown>): Promise<SourceJob> {
  const error = resourceWriteError(snapshot)
  if (error) throw new Error(error)
  const job = snapshot.data
  if (!job || !validSourceJob(job) || job.id !== id || !['queued', 'running', 'cancelling'].includes(job.status)) throw new Error('当前任务已结束或信息不完整，请刷新任务。')
  const receipt = await writer(`${sourceJobRoot}/${id}/cancel`, 'POST')
  if (!validSourceJob(receipt) || receipt.id !== id || receipt.source_id !== job.source_id) throw new Error('取消收据尚未确认，请刷新任务。')
  return receipt
}
// A late browser file read may never restore a secret after switching input or saving.
export class SourceFileReader {
  private generation = 0
  invalidate() { this.generation++ }
  async read(file: Pick<File, 'size' | 'arrayBuffer'>) {
    const generation = ++this.generation
    if (file.size > MAX_SOURCE_BYTES) throw new Error('订阅文件不能超过 2 MiB。')
    let bytes: ArrayBuffer
    try { bytes = await file.arrayBuffer() }
    catch { if (generation !== this.generation) return null; throw new Error('订阅文件读取失败，请重新选择文件。') }
    if (generation !== this.generation) return null
    if (bytes.byteLength > MAX_SOURCE_BYTES) throw new Error('订阅文件不能超过 2 MiB。')
    try { return new TextDecoder('utf-8', { fatal: true }).decode(bytes) }
    catch { throw new Error('订阅文件必须使用 UTF-8 编码。') }
  }
}
export async function adoptSourceNode(snapshot: SourceSnapshot, source: SubscriptionSource, node: SourceNode, adopted: boolean, writer = sourceRequest<unknown>): Promise<SourceNode> {
  // Removing from the catalog stays possible on an archived source; adopting does not.
  const error = sourceWriteError(snapshot, source.id, source.settings_revision, !adopted)
  if (error) throw new Error(error)
  if (!validSourceNode(node) || node.source_id !== source.id || node.identity_epoch !== source.identity_epoch) throw new Error('节点信息已过期，请刷新来源详情后重新确认。')
  if (adopted && !node.selectable) throw new Error('此节点当前不可加入节点库：须在最近成功批次中、身份唯一且受支持。')
  const result = await writer(`${sourceRoot}/${source.id}/nodes/${node.id}`, 'PATCH', { adopted, settings_revision: source.settings_revision, identity_epoch: node.identity_epoch, node_version_id: node.version_id, metadata_revision: node.metadata_revision })
  if (!validSourceNode(result) || result.id !== node.id || result.adopted !== adopted) throw new Error('节点库状态尚未确认，请刷新来源详情。')
  return result
}
export type PreviewInput = { kind: 'url'; url: string; auth_headers: Record<string, string> } | { kind: 'inline'; content: string }
export function previewInput(draft: SourceDraft): { input: PreviewInput; user_agent: string | null } {
  if (draft.kind === 'url') return { input: { kind: 'url', url: urlText(draft.url), auth_headers: sourceAuthHeaders(draft) }, user_agent: userAgentText(draft.userAgent) }
  return { input: { kind: 'inline', content: contentText(draft.content) }, user_agent: null }
}
export async function createOrderedPreview(request: ReturnType<typeof previewInput>, writer = sourceRequest<unknown>): Promise<OrderedPreview> {
  const preview = await writer(previewRoot, 'POST', request)
  if (!validOrderedPreview(preview)) throw new Error('面板返回的导入预览格式不完整，请重新解析。')
  return preview
}
export async function discardOrderedPreview(id: string, writer = sourceRequest<unknown>) {
  if (!sourceUuid(id)) return
  await writer(`${previewRoot}/${id}`, 'DELETE')
}
export type PreviewCommit = { request_id: string; name: string; selected: string[]; refresh_interval_secs: number | null; auto_refresh: boolean }
export function previewCommit(preview: OrderedPreview, draft: SourceDraft, selected: string[], requestId: string, now = Date.now()): PreviewCommit {
  const error = orderedPreviewError(preview, selected, now)
  if (error) throw new Error(error)
  const name = draft.name.trim()
  if (!name || [...name].length > 128 || name.includes('://') || /[\u0000-\u001f\u007f-\u009f]/.test(name)) throw new Error('来源名称需为 1–128 个字符，不能包含链接或控制字符。')
  if (!sourceUuid(requestId)) throw new Error('请求标识无效。')
  return { request_id: requestId, name, selected: [...selected].sort(), refresh_interval_secs: draft.kind === 'url' ? refreshInterval(draft.interval) : null, auto_refresh: draft.kind === 'url' && draft.autoRefresh === 'on' }
}
export async function commitOrderedPreview(preview: OrderedPreview, body: PreviewCommit, writer = sourceRequest<unknown>): Promise<SourceReceipt> {
  const receipt = await writer(`${previewRoot}/${preview.id}/commit`, 'POST', body)
  if (!validSourceReceipt(receipt)) throw new Error('保存收据尚未确认，请保留原请求并重试。')
  return receipt
}
