export const sourceRoot = '/api/traffic/ordered-subscription-sources'
export const sourceJobRoot = '/api/traffic/ordered-subscription-source-jobs'
export const MAX_SOURCE_BYTES = 2 * 1024 * 1024
export type SourceCounts = { supported: number; unsupported: number; ambiguous: number; missing: number }
export type SourceFailure = { stage: string; kind: string; message: string; http_status: number | null }
export type SourceRevision = { id: string; source_id: number; settings_revision: number; identity_epoch: number; parser_version: string; format: string; parsed_at: number; counts: SourceCounts }
export type SourceJobStatus = 'queued' | 'running' | 'cancelling' | 'succeeded' | 'unchanged' | 'failed' | 'cancelled' | 'superseded'
export type SourceJob = { id: string; source_id: number; settings_revision: number; identity_epoch: number; parser_version: string; status: SourceJobStatus; stage: 'queued' | 'fetch' | 'parse' | 'store' | 'done'; created_at: number; started_at: number | null; finished_at: number | null; source_revision_id: string | null; error: SourceFailure | null }
export type SourceDependency = { chain_id: number; chain_name: string; generation: number; state: 'applied' | 'candidate' | 'recovery'; hop_position: number; external_node_id: string; node_version_id: string; identity_epoch: number }
export const MIN_REFRESH_SECS = 300
export const MAX_REFRESH_SECS = 2592000
export type SourceTraffic = { upload?: number; download?: number; total?: number; expire?: number; updated_at?: number }
export type SourceChanges = { added: number; updated: number; missing: number; unsupported: number }
export type SubscriptionSource = {
  id: number; name: string; kind: 'url' | 'inline'; host: string | null; configured: boolean; auth_configured: boolean;
  settings_revision: number; identity_epoch: number; archived: boolean; refresh_interval_secs: number;
  user_agent: string | null; auto_refresh: boolean; traffic: SourceTraffic; changes: SourceChanges;
  last_attempt_at: number | null; last_success_at: number | null; latest_success: SourceRevision | null;
  active_job: SourceJob | null; last_error: SourceFailure | null; stale_reason: string | null; counts: SourceCounts;
  dependencies: SourceDependency[];
}
export type SourceNode = {
  id: string; public_id: number; adopted: boolean; metadata_revision: number; source_id: number; identity_epoch: number; version_id: string; source_revision_id: string;
  present_in_latest: boolean; identity_state: 'unique' | 'ambiguous' | 'unresolved'; supported: boolean; selectable: boolean;
  reasons: string[]; capabilities: { tcp: boolean; udp: boolean }; ordinal: number; name: string;
  protocol: string | null; server: string | null; server_port: number | null; sni: string | null; transport: string | null;
  parse_status: 'supported' | 'unsupported'; unsupported_reasons: { code: string; message: string }[];
}
export type SourceNodePage = { source_id: number; current_settings_revision: number; current_identity_epoch: number; success_revision: SourceRevision | null; nodes: SourceNode[] }
export type SourceHistory = { source_id: number; revisions: SourceRevision[] }
export type SourceReceipt = { source_id: number; settings_revision: number; identity_epoch: number; job_id: string | null }
const record = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v)
const exact = (v: Record<string, unknown>, keys: string) => Object.keys(v).length === keys.split(' ').length && keys.split(' ').every(key => Object.hasOwn(v, key))
const integer = (v: unknown, min = 0): v is number => typeof v === 'number' && Number.isSafeInteger(v) && v >= min
const text = (v: unknown): v is string => typeof v === 'string' && v.length <= 4096 && !/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(v)
const nullableText = (v: unknown) => v === null || text(v)
const nullableTime = (v: unknown) => v === null || integer(v)
export const sourceUuid = (v: unknown): v is string => typeof v === 'string' && /^[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/i.test(v)
const nullableUuid = (v: unknown) => v === null || sourceUuid(v)
const publicHost = (v: unknown) => v === null || text(v) && v.length > 0 && !/[\s/@?#\\]/.test(v)
export function validSourceTraffic(v: unknown): v is SourceTraffic {
  return record(v) && Object.entries(v).every(([key, value]) => ['upload', 'download', 'total', 'expire', 'updated_at'].includes(key) && integer(value))
}
export function validSourceChanges(v: unknown): v is SourceChanges {
  return record(v) && exact(v, 'added updated missing unsupported') && Object.values(v).every(value => integer(value))
}
export function validSourceCounts(v: unknown): v is SourceCounts {
  return record(v) && exact(v, 'supported unsupported ambiguous missing') && Object.entries(v).every(([key, value]) => integer(value) && (key === 'missing' || value <= 5000))
}
export function validSourceFailure(v: unknown): v is SourceFailure {
  return record(v) && exact(v, 'stage kind message http_status') && text(v.stage) && text(v.kind) && text(v.message)
    && (v.http_status === null || integer(v.http_status, 100) && v.http_status <= 599)
}
export function validSourceRevision(v: unknown): v is SourceRevision {
  return record(v) && exact(v, 'id source_id settings_revision identity_epoch parser_version format parsed_at counts')
    && sourceUuid(v.id) && integer(v.source_id, 1) && integer(v.settings_revision, 1) && integer(v.identity_epoch, 1)
    && text(v.parser_version) && ['uri_list', 'base64_uri_list', 'sing_box_json', 'clash_yaml'].includes(String(v.format)) && integer(v.parsed_at) && validSourceCounts(v.counts)
}
export const sourceJobActive = (job: Pick<SourceJob, 'status'> | null | undefined) => !!job && ['queued', 'running', 'cancelling'].includes(job.status)
export function validSourceDependency(v: unknown): v is SourceDependency {
  return record(v) && exact(v, 'chain_id chain_name generation state hop_position external_node_id node_version_id identity_epoch') && integer(v.chain_id, 1) && text(v.chain_name) && integer(v.generation, 1)
    && ['applied', 'candidate', 'recovery'].includes(String(v.state)) && integer(v.hop_position, 1) && v.hop_position <= 8 && sourceUuid(v.external_node_id) && sourceUuid(v.node_version_id) && integer(v.identity_epoch, 1)
}
export function validSourceJob(v: unknown): v is SourceJob {
  return record(v) && exact(v, 'id source_id settings_revision identity_epoch parser_version status stage created_at started_at finished_at source_revision_id error')
    && sourceUuid(v.id) && integer(v.source_id, 1) && integer(v.settings_revision, 1) && integer(v.identity_epoch, 1) && text(v.parser_version)
    && ['queued', 'running', 'cancelling', 'succeeded', 'unchanged', 'failed', 'cancelled', 'superseded'].includes(String(v.status))
    && ['queued', 'fetch', 'parse', 'store', 'done'].includes(String(v.stage)) && integer(v.created_at)
    && nullableTime(v.started_at) && nullableTime(v.finished_at) && nullableUuid(v.source_revision_id)
    && (v.error === null || validSourceFailure(v.error))
}
export function validSubscriptionSource(v: unknown): v is SubscriptionSource {
  return record(v) && exact(v, 'id name kind host configured auth_configured settings_revision identity_epoch archived refresh_interval_secs user_agent auto_refresh traffic changes last_attempt_at last_success_at latest_success active_job last_error stale_reason counts dependencies')
    && integer(v.id, 1) && text(v.name) && (v.kind === 'url' || v.kind === 'inline') && publicHost(v.host)
    && typeof v.configured === 'boolean' && typeof v.auth_configured === 'boolean' && integer(v.settings_revision, 1) && integer(v.identity_epoch, 1)
    && typeof v.archived === 'boolean' && integer(v.refresh_interval_secs) && (v.kind === 'inline' ? v.refresh_interval_secs === 0 : v.refresh_interval_secs >= MIN_REFRESH_SECS && v.refresh_interval_secs <= MAX_REFRESH_SECS)
    && (v.user_agent === null || text(v.user_agent) && v.user_agent.length > 0 && v.user_agent.length <= 256) && typeof v.auto_refresh === 'boolean' && validSourceTraffic(v.traffic) && validSourceChanges(v.changes)
    && nullableTime(v.last_attempt_at) && nullableTime(v.last_success_at) && nullableText(v.stale_reason) && validSourceCounts(v.counts)
    && (v.latest_success === null || validSourceRevision(v.latest_success) && v.latest_success.source_id === v.id)
    && (v.active_job === null || validSourceJob(v.active_job) && v.active_job.source_id === v.id && sourceJobActive(v.active_job))
    && (v.last_error === null || validSourceFailure(v.last_error)) && Array.isArray(v.dependencies) && v.dependencies.every(validSourceDependency)
}
export function validSubscriptionSources(v: unknown): v is SubscriptionSource[] {
  return Array.isArray(v) && v.length <= 128 && v.every(validSubscriptionSource) && new Set(v.map(item => item.id)).size === v.length
}
export function validSourceNode(v: unknown): v is SourceNode {
  return record(v) && exact(v, 'id public_id adopted metadata_revision source_id identity_epoch version_id source_revision_id present_in_latest identity_state supported selectable reasons capabilities ordinal name protocol server server_port sni transport parse_status unsupported_reasons')
    && sourceUuid(v.id) && integer(v.public_id, 1) && typeof v.adopted === 'boolean' && integer(v.metadata_revision) && integer(v.source_id, 1) && integer(v.identity_epoch, 1) && sourceUuid(v.version_id) && sourceUuid(v.source_revision_id)
    && typeof v.present_in_latest === 'boolean' && ['unique', 'ambiguous', 'unresolved'].includes(String(v.identity_state)) && typeof v.supported === 'boolean' && typeof v.selectable === 'boolean'
    && Array.isArray(v.reasons) && v.reasons.every(text) && record(v.capabilities) && exact(v.capabilities, 'tcp udp') && typeof v.capabilities.tcp === 'boolean' && typeof v.capabilities.udp === 'boolean'
    && integer(v.ordinal) && v.ordinal < 5000 && text(v.name) && nullableText(v.protocol) && publicHost(v.server) && publicHost(v.sni) && nullableText(v.transport)
    && (v.server_port === null || integer(v.server_port, 1) && v.server_port <= 65535) && ['supported', 'unsupported'].includes(String(v.parse_status))
    && (v.supported === (v.parse_status === 'supported')) && (!v.selectable || v.supported && v.present_in_latest && v.identity_state === 'unique')
    && Array.isArray(v.unsupported_reasons) && v.unsupported_reasons.every(reason => record(reason) && exact(reason, 'code message') && text(reason.code) && text(reason.message))
}
export function validSourceNodePage(v: unknown): v is SourceNodePage {
  return record(v) && exact(v, 'source_id current_settings_revision current_identity_epoch success_revision nodes') && integer(v.source_id, 1) && integer(v.current_settings_revision, 1) && integer(v.current_identity_epoch, 1)
    && (v.success_revision === null || validSourceRevision(v.success_revision) && v.success_revision.source_id === v.source_id)
    && Array.isArray(v.nodes) && v.nodes.length <= 5000 && (v.success_revision !== null || v.nodes.length === 0) && v.nodes.every(node => validSourceNode(node) && node.source_id === v.source_id
      && (!node.selectable || node.identity_epoch === v.current_identity_epoch))
    && new Set(v.nodes.map(node => node.version_id)).size === v.nodes.length
}
export function validSourceHistory(v: unknown): v is SourceHistory {
  return record(v) && exact(v, 'source_id revisions') && integer(v.source_id, 1) && Array.isArray(v.revisions)
    && v.revisions.every(revision => validSourceRevision(revision) && revision.source_id === v.source_id) && new Set(v.revisions.map(revision => revision.id)).size === v.revisions.length
}
export function validSourceReceipt(v: unknown): v is SourceReceipt {
  return record(v) && exact(v, 'source_id settings_revision identity_epoch job_id') && integer(v.source_id, 1) && integer(v.settings_revision, 1) && integer(v.identity_epoch, 1) && nullableUuid(v.job_id)
}
export const sourceStatusText: Record<SourceJobStatus, string> = { queued: '等待处理', running: '处理中', cancelling: '等待取消确认', succeeded: '解析成功', unchanged: '内容未变化', failed: '处理失败', cancelled: '已取消', superseded: '已被新设置取代' }
export const sourceStageText: Record<SourceJob['stage'], string> = { queued: '排队', fetch: '抓取订阅', parse: '解析节点', store: '保存版本', done: '结束' }
export const sourceFormatText: Record<string, string> = { uri_list: 'URI 列表', base64_uri_list: 'Base64 URI 列表', sing_box_json: 'sing-box JSON', clash_yaml: 'Clash YAML' }
export function sourceInterval(seconds: number) {
  if (seconds % 86400 === 0) return `每 ${seconds / 86400} 天`
  if (seconds % 3600 === 0) return `每 ${seconds / 3600} 小时`
  return seconds % 60 === 0 ? `每 ${seconds / 60} 分钟` : `每 ${seconds} 秒`
}

// Previews let an administrator choose which nodes join the catalog before saving.
export type OrderedPreviewNode = Omit<SourceNode, 'id' | 'public_id' | 'adopted' | 'metadata_revision' | 'source_id' | 'identity_epoch' | 'version_id' | 'source_revision_id' | 'present_in_latest' | 'supported' | 'reasons'> & { key: string }
export type OrderedPreview = { id: string; expires_at: number; format: string; supported_count: number; unsupported_count: number; warnings: { code: string; message: string }[]; nodes: OrderedPreviewNode[] }
export function validOrderedPreview(v: unknown): v is OrderedPreview {
  return record(v) && exact(v, 'id expires_at format supported_count unsupported_count warnings nodes') && sourceUuid(v.id) && integer(v.expires_at, 1) && Number.isSafeInteger(v.expires_at * 1000)
    && ['uri_list', 'base64_uri_list', 'sing_box_json', 'clash_yaml'].includes(String(v.format)) && integer(v.supported_count) && integer(v.unsupported_count)
    && Array.isArray(v.warnings) && v.warnings.every(warning => record(warning) && exact(warning, 'code message') && text(warning.code) && text(warning.message))
    && Array.isArray(v.nodes) && v.nodes.length <= 5000 && v.nodes.every(node => record(node) && exact(node, 'key selectable identity_state capabilities ordinal name protocol server server_port sni transport parse_status unsupported_reasons')
      && typeof node.key === 'string' && node.key === `node-${node.ordinal}` && typeof node.selectable === 'boolean' && ['unique', 'ambiguous', 'unresolved'].includes(String(node.identity_state))
      && record(node.capabilities) && exact(node.capabilities, 'tcp udp') && typeof node.capabilities.tcp === 'boolean' && typeof node.capabilities.udp === 'boolean'
      && integer(node.ordinal) && node.ordinal < 5000 && text(node.name) && nullableText(node.protocol) && publicHost(node.server) && publicHost(node.sni) && nullableText(node.transport)
      && (node.server_port === null || integer(node.server_port, 1) && node.server_port <= 65535) && ['supported', 'unsupported'].includes(String(node.parse_status))
      && (!node.selectable || node.parse_status === 'supported' && node.identity_state === 'unique')
      && Array.isArray(node.unsupported_reasons) && node.unsupported_reasons.every(reason => record(reason) && exact(reason, 'code message') && text(reason.code) && text(reason.message)))
    && new Set(v.nodes.map(node => node.key)).size === v.nodes.length
    && v.nodes.filter(node => node.parse_status === 'supported').length === v.supported_count && v.nodes.filter(node => node.parse_status !== 'supported').length === v.unsupported_count
}
export function orderedPreviewError(preview: OrderedPreview, selected: string[], now = Date.now()): string {
  if (!validOrderedPreview(preview)) return '预览信息不完整，请重新解析；当前选择已保留。'
  if (!Number.isSafeInteger(now) || preview.expires_at * 1000 <= now) return '导入预览已过期，请重新解析；当前选择已保留。'
  if (!selected.length || selected.length > 5000 || new Set(selected).size !== selected.length || selected.some(key => !preview.nodes.some(node => node.key === key && node.selectable))) return '请至少选择一个可导入的节点；所选节点须受支持且身份唯一。'
  return ''
}
