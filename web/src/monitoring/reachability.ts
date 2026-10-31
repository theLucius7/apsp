export type ReachabilityState = 'reachable' | 'partial' | 'service_down' | 'port_blocked' | 'ip_blocked' | 'unconfirmed' | 'unknown' | 'not_applicable'
export type Probe = { carrier: string; network: string; city: string; connected: boolean; avg_ms: number | null }
type ProbeGroup = { probes: Probe[]; error: string | null }
export type ReachabilityDetail = {
  china?: ProbeGroup
  overseas?: ProbeGroup
  control?: { status: string; port?: number; probes?: Probe[]; error?: string }
  reason?: string
}
export type Risk = { code: string; value: string }
export type ReachabilityTarget = {
  key: string; host: string; port: number; tcp: boolean
  state: ReachabilityState; blocked: boolean; pending_state: ReachabilityState | null; failing_streak: number
  observed_state: ReachabilityState | null; resolved_ip: string | null; detail: ReachabilityDetail
  checked_at: number | null; state_changed_at: number | null; deferred_at: number | null
  nodes: { kind: 'native' | 'nexus'; id: number; name: string; server_id: number; server_name: string; protocol: string }[]
  risks: Risk[]
}
export type RunSummary = {
  started_at?: number; finished_at?: number; manual?: boolean; targets?: number; due?: number; measured?: number
  deferred?: number; unresolved?: number; tests_used?: number; allowance?: number | null; limit?: number | null
  remaining?: number | null; reset_secs?: number | null; error?: string | null
}
export type ReachabilitySettings = {
  enabled: boolean; interval_minutes: number; minimum_interval_minutes: number; maximum_interval_minutes: number
  token_configured: boolean; revision: number; next_run_at: number; run_requested: boolean; running: boolean; last_run: RunSummary
}
export type ReachabilityEvent = {
  id: string; kind: 'blocked' | 'cleared'; from_state: ReachabilityState; to_state: ReachabilityState; message: string; created_at: number
  deliveries: { channel: string; state: string; attempts: number; last_error: string | null }[]
}
export type ReachabilityOverview = { settings: ReachabilitySettings; targets: ReachabilityTarget[]; events: ReachabilityEvent[] }

export const stateLabels: Record<ReachabilityState, string> = {
  reachable: '可达',
  partial: '部分运营商可达',
  service_down: '服务不可用',
  port_blocked: '端口被封锁',
  ip_blocked: 'IP 被封锁',
  unconfirmed: '中国不可达（原因未定）',
  unknown: '未知',
  not_applicable: '不适用',
}
export const stateTones: Record<ReachabilityState, 'good' | 'bad' | 'warm' | 'neutral'> = {
  reachable: 'good', partial: 'warm', service_down: 'warm', port_blocked: 'bad', ip_blocked: 'bad', unconfirmed: 'bad', unknown: 'neutral', not_applicable: 'neutral',
}

const carrierLabels: Record<string, string> = { telecom: '电信', unicom: '联通', mobile: '移动', other: '其他' }
const carrierOrder = ['telecom', 'unicom', 'mobile', 'other']

/** Connected over returned probes for each carrier, such as "电信 1/2 · 联通 0/1". */
export function carrierSummary(probes: Probe[]) {
  const counts = new Map<string, [number, number]>()
  for (const probe of probes) {
    const carrier = carrierOrder.includes(probe.carrier) ? probe.carrier : 'other'
    const [connected, total] = counts.get(carrier) ?? [0, 0]
    counts.set(carrier, [connected + (probe.connected ? 1 : 0), total + 1])
  }
  return carrierOrder.filter(carrier => counts.has(carrier)).map(carrier => `${carrierLabels[carrier]} ${counts.get(carrier)![0]}/${counts.get(carrier)![1]}`).join(' · ')
}

const connectedCount = (probes: Probe[]) => probes.filter(probe => probe.connected).length

function controlLine(control: NonNullable<ReachabilityDetail['control']>) {
  switch (control.status) {
    case 'checked': return `对照端口 ${control.port}：${carrierSummary(control.probes ?? []) || '无结果'}`
    case 'no_port': return '22、443、80 在海外都未开放，无法对照'
    case 'deferred': return '额度不足，对照已延后'
    case 'failed': return '对照测量失败'
    default: return ''
  }
}

/** The lines shown in the carrier column; empty when nothing has been measured. */
export function detailLines(detail: ReachabilityDetail) {
  if (detail.reason === 'udp_only') return ['节点只使用 UDP，TCP 检测无法判断']
  if (detail.reason === 'no_public_address') return ['未解析到公网地址，没有发送检测']
  const lines: string[] = []
  if (detail.china) lines.push(detail.china.error ? '中国：测量失败' : `中国：${carrierSummary(detail.china.probes) || '无结果'}`)
  if (detail.overseas) lines.push(detail.overseas.error ? '海外：测量失败' : `海外：${connectedCount(detail.overseas.probes)}/${detail.overseas.probes.length}`)
  if (detail.control) { const line = controlLine(detail.control); if (line) lines.push(line) }
  return lines
}

export function riskLabel(risk: Risk) {
  switch (risk.code) {
    case 'port_not_443': return `公开端口 ${risk.value} 不是 443`
    case 'name_risky_tld': return `目标或 SNI 使用高风险后缀：${risk.value}`
    case 'name_risky_brand': return `目标或 SNI 含易触发封锁的名称：${risk.value}`
    default: return risk.value
  }
}

const severity: ReachabilityState[] = ['ip_blocked', 'port_blocked', 'unconfirmed', 'partial', 'service_down', 'unknown', 'reachable', 'not_applicable']

/** Problems first, then a failure still waiting for its second run, then by endpoint. */
export function sortTargets(targets: ReachabilityTarget[]) {
  return [...targets].sort((a, b) => severity.indexOf(a.state) - severity.indexOf(b.state)
    || Number(Boolean(b.pending_state)) - Number(Boolean(a.pending_state))
    || a.key.localeCompare(b.key))
}

export function pendingNote(target: ReachabilityTarget) {
  return target.pending_state ? `待确认：${stateLabels[target.pending_state]}（第 ${target.failing_streak} 次，需连续 2 次）` : ''
}

export function intervalError(value: string, settings: Pick<ReachabilitySettings, 'minimum_interval_minutes' | 'maximum_interval_minutes'>) {
  const minutes = Number(value)
  if (!Number.isInteger(minutes) || minutes < settings.minimum_interval_minutes || minutes > settings.maximum_interval_minutes) return `检测间隔须为 ${settings.minimum_interval_minutes} 至 ${settings.maximum_interval_minutes} 之间的整数分钟。`
  return ''
}

const runErrors: Record<string, string> = {
  token_unavailable: '无法读取已保存的 API 令牌，本轮没有发送检测。',
  run_failed: '本轮执行失败，请查看面板日志。',
}

export function runErrorText(code: string) {
  return code.startsWith('limits_') ? '无法读取 Globalping 额度，本轮没有发送检测。' : runErrors[code] ?? `本轮未完成（${code}）。`
}

/** One line about the last run; undefined before the first run. */
export function runLine(run: RunSummary) {
  if (!run.started_at) return undefined
  const parts = [`检测 ${run.measured ?? 0} 组`]
  if (run.deferred) parts.push(`因额度延后 ${run.deferred} 组`)
  if (run.unresolved) parts.push(`${run.unresolved} 个地址未解析`)
  parts.push(`用量 ${run.tests_used ?? 0} 次`)
  if (run.remaining !== null && run.remaining !== undefined && run.limit) parts.push(`开始时额度剩余 ${run.remaining}/${run.limit}`)
  return parts.join('，')
}

export const deliveryLabels: Record<string, string> = { pending: '待发送', sending: '发送中', sent: '已发送', failed: '发送失败', cancelled: '已取消' }
