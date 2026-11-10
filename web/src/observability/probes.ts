import { resourceWriteError } from '../hooks'

export type ProbeNetwork = 'other' | 'telecom' | 'unicom' | 'mobile'
export type ProbeMonitoring = { network: ProbeNetwork; region: string; ip_version: 'auto' | 'ipv4' | 'ipv6'; authorization: { basis: 'unconfirmed' | 'owned' | 'permission'; confirmed: boolean; source: string; scope: string; expires_at: number | null } }
export type ProbeIdentity = { kind: 'tcp' | 'icmp'; target: string; port: number | null; address_family: 'any' | 'ipv4' | 'ipv6' }
export type ProbeAuthorization = { kind: 'owned' | 'consent'; source: string; scope: string; enabled: boolean; expires_at: number | null; identity: ProbeIdentity }
export type ProbeMonitor = { network?: ProbeNetwork; region: string; address_family: ProbeIdentity['address_family']; authorization: ProbeAuthorization | null }
export type Probe = { revision?: number | null; task_id?: string; id: string; name: string; kind: 'tcp' | 'icmp'; target: string; port: number | null; interval_secs: number; carrier: string; enabled: boolean; monitor?: ProbeMonitor | null; execution_authorized?: boolean | null }
export type ProbeResult = { id: string; probe_id: string; sampled_at: number; latency_ms: number | null; loss_percent: number; error: string | null; address_family?: 'ipv4' | 'ipv6' | null; attempts?: 4 }
export type ProbeOverview = { server_id: number; probe: Probe; results: ProbeResult[] }
export type ProbeField = 'latency_ms' | 'loss_percent'

export const lossLabel = (probe: Pick<Probe, 'kind'>) => probe.kind === 'tcp' ? '连接失败率' : '丢包率'
export const latency = (value: number | null) => value === null ? '—' : `${value.toFixed(1)} ms`
export const loss = (value: number | null) => value === null ? '—' : `${value.toFixed(1)}%`
export const carrierLabel = (value: string) => (({ telecom: '电信', unicom: '联通', mobile: '移动' } as Record<string, string>)[value] ?? value) || '线路未知'
export const familyLabel = (probe: Probe, result?: ProbeResult) => result?.address_family ? (result.address_family === 'ipv4' ? 'IPv4' : 'IPv6') : probe.monitor?.address_family === 'ipv4' ? 'IPv4（配置）' : probe.monitor?.address_family === 'ipv6' ? 'IPv6（配置）' : '自动（实际版本未知）'
export const networks: [ProbeNetwork, string][] = [['telecom', '电信'], ['unicom', '联通'], ['mobile', '移动'], ['other', '其他线路']]
export const emptyMonitoring = (): ProbeMonitoring => ({ network: 'other', region: '', ip_version: 'auto', authorization: { basis: 'unconfirmed', confirmed: false, source: '', scope: '', expires_at: null } })
export const networkLabel = (probe: Probe) => networks.find(([key]) => key === (probe.monitor?.network ?? probe.carrier))?.[1] ?? '其他线路'
export function monitoringOf(probe: Probe): ProbeMonitoring {
  const monitor = probe.monitor, authorization = monitor?.authorization
  return { network: monitor?.network ?? 'other', region: monitor?.region ?? '', ip_version: monitor?.address_family === 'any' || !monitor ? 'auto' : monitor.address_family,
    authorization: { basis: authorization ? authorization.kind === 'owned' ? 'owned' : 'permission' : 'unconfirmed', confirmed: Boolean(authorization?.enabled && authorizationMatches(probe)), source: authorization?.source ?? '', scope: authorization?.scope ?? '', expires_at: authorization?.expires_at ?? null } }
}
export function withMonitoring(probe: Probe, value: ProbeMonitoring): Probe {
  const address_family = value.ip_version === 'auto' ? 'any' : value.ip_version, authorization = value.authorization
  return { ...probe, monitor: { network: value.network, region: value.region, address_family,
    authorization: authorization.basis === 'unconfirmed' ? null : { kind: authorization.basis === 'owned' ? 'owned' : 'consent', source: authorization.source, scope: authorization.scope, enabled: authorization.confirmed, expires_at: authorization.expires_at,
      identity: { kind: probe.kind, target: probe.target.trim(), port: probe.port, address_family } } } }
}

export const probeIdentity = (probe: Probe): ProbeIdentity => ({ kind: probe.kind, target: probe.target.trim(), port: probe.port, address_family: probe.monitor?.address_family ?? 'any' })
const sameIdentity = (left: ProbeIdentity, right: ProbeIdentity) => left.kind === right.kind && left.target === right.target && left.port === right.port && left.address_family === right.address_family
export const authorizationMatches = (probe: Probe) => Boolean(probe.monitor?.authorization?.identity && sameIdentity(probe.monitor.authorization.identity, probeIdentity(probe)))

export function changeProbe(probe: Probe, part: Partial<Probe>): Probe {
  const changed = { ...probe, ...part }
  if (sameIdentity(probeIdentity(probe), probeIdentity(changed)) || !changed.monitor?.authorization) return changed
  return { ...changed, monitor: { ...changed.monitor, authorization: { ...changed.monitor.authorization, enabled: false } } }
}

export function bindProbeAuthorization(probe: Probe): Probe {
  const { execution_authorized: _derived, ...payload } = probe
  const monitor = payload.monitor, authorization = monitor?.authorization
  if (!authorization) return { ...payload, enabled: false }
  if (!authorization.source.trim() || !authorization.scope.trim()) return { ...payload, enabled: false, monitor: { ...monitor!, authorization: null } }
  const confirmed = authorization.enabled && authorizationMatches(probe)
  return { ...payload, enabled: payload.enabled && confirmed, monitor: { ...monitor!, authorization: { ...authorization, enabled: confirmed } } }
}

export function authorizationState(probe: Probe, now = Date.now()): string | null {
  const authorization = probe.monitor?.authorization
  if (authorization && !authorizationMatches(probe)) return '当前目标需重新确认授权'
  if (authorization?.expires_at != null && authorization.expires_at * 1000 <= now) return '授权已过期'
  if (authorization && !authorization.enabled) return '授权已撤销'
  if (probe.execution_authorized === false || !authorization && probe.execution_authorized !== true) return '未取得执行授权'
  return null
}

// Legacy Agents also encode an unavailable measurement with an error and a numeric loss placeholder.
export function probeValue(result: ProbeResult | undefined, field: ProbeField): number | null {
  if (!result || result.error != null && result.attempts !== 4) return null
  const value = result[field]
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 && value <= (field === 'loss_percent' ? 100 : 60_000) ? value : null
}

export function probeState(probe: Probe, latest: ProbeResult | undefined, now: number, unavailable = false): string {
  if (unavailable) return '状态未知'
  const authorization = authorizationState(probe, now)
  if (authorization) return authorization
  if (!probe.enabled) return '已暂停'
  if (!latest) return '等待采样'
  if (latest.sampled_at > now || now - latest.sampled_at > Math.max(60_000, probe.interval_secs * 3000)) return '采样已过期'
  if (latest.error != null && latest.attempts !== 4) return '检测不可用'
  return '最近采样'
}

export function probeTone(value: number | null, field: ProbeField) {
  if (value === null) return 'empty'
  const thresholds = field === 'latency_ms' ? [60, 100, 160, 200] : [1, 3, 6, 9]
  return ['good', 'fair', 'warning', 'poor', 'danger'][thresholds.findIndex(threshold => value <= threshold)] ?? 'danger'
}

export function probeSlots(results: ProbeResult[], probe: Probe, now: number, count = 20) {
  const width = Math.max(10, probe.interval_secs) * 1000
  const end = Math.ceil(now / width) * width
  const slots: (ProbeResult | undefined)[] = Array(count).fill(undefined)
  for (const point of results) {
    if (point.probe_id !== probe.id || !Number.isFinite(point.sampled_at) || point.sampled_at > now) continue
    const index = Math.min(count - 1, Math.floor((point.sampled_at - (end - count * width)) / width))
    if (index < 0) continue
    if (!slots[index] || point.sampled_at > slots[index]!.sampled_at) slots[index] = point
  }
  return slots
}

// A persisted revision is required for edits and deletion; unavailable/stale reads
// cannot supply execution authority or overwrite a newer administrator's draft.
export const probeRevisionMatches = (draft: { revision?: number | null }, current: { revision?: number | null } | undefined) => Boolean(current && Number.isSafeInteger(draft.revision) && (draft.revision ?? 0) > 0 && current.revision === draft.revision)

export function probeWriteError(probe: Probe, current: Probe | undefined): string {
  if (!current || !probeRevisionMatches(probe, current) || probe.id !== current?.id || !sameIdentity(probeIdentity(probe), probeIdentity(current)))
    return '此拨测已不存在或目标、版本已变化，请刷新后重新确认；当前草稿已保留。'
  return ''
}

export type ProbeResource<T> = { data?: T[]; fresh: boolean; error: string; isCurrent?: () => boolean; getCurrent?: () => T[] | undefined }
export type ProbeWriteSnapshot = { serverId: number; probes: ProbeResource<Probe> }
export type LatencyTask = { id: string; spec: Probe; default_enabled: boolean; server_ids: number[]; revision: number }
export type TaskWriteSnapshot = { tasks: ProbeResource<LatencyTask>; servers: ProbeResource<{ id: number }> }
export const probeLeaseNotice = '新 Agent 仅在有效短期许可内检测；断连或刷新失败后停止续期，许可最长 90 秒。保存、暂停或删除配置不代表设备已经立即停止。'
const currentValues = <T,>(resource: ProbeResource<T>) => resource.getCurrent ? resource.getCurrent() : resource.data
export function probeReadError(...resources: ProbeResource<unknown>[]) {
  return resourceWriteError(...resources) || (resources.some(resource => !Array.isArray(currentValues(resource))) ? '拨测列表格式未知，请刷新后再提交；草稿已保留。' : '')
}
export function probePayloadError(probe: Probe, now = Date.now()): string {
  const bytes = (value: string) => new TextEncoder().encode(value).length
  const controls = /[\u0000-\u001f\u007f-\u009f]/
  if (!probe.name.trim() || controls.test(probe.name) || bytes(probe.name) > 128) return '拨测名称无效。'
  if (!['tcp', 'icmp'].includes(probe.kind) || !probe.target.trim() || !/^[A-Za-z0-9._:-]+$/.test(probe.target.trim()) || bytes(probe.target) > 253) return '拨测目标必须为主机名或 IP 地址。'
  if (!Number.isSafeInteger(probe.interval_secs) || probe.interval_secs < 10 || probe.interval_secs > 3600) return '拨测间隔应为 10–3600 秒的整数。'
  if (probe.kind === 'tcp' && (!Number.isSafeInteger(probe.port) || probe.port! < 1 || probe.port! > 65535)) return '拨测端口应为 1–65535 的整数。'
  if (controls.test(probe.carrier) || bytes(probe.carrier) > 64 || typeof probe.enabled !== 'boolean') return '拨测配置无效。'
  const monitor = probe.monitor, authorization = monitor?.authorization
  if (monitor && (!['any', 'ipv4', 'ipv6'].includes(monitor.address_family) || controls.test(monitor.region) || bytes(monitor.region) > 64)) return '目标地区或地址家族无效。'
  if (authorization && (!['owned', 'consent'].includes(authorization.kind) || controls.test(authorization.source) || bytes(authorization.source) > 256 || controls.test(authorization.scope) || bytes(authorization.scope) > 512
    || authorization.expires_at !== null && (!Number.isSafeInteger(authorization.expires_at) || authorization.expires_at <= 0))) return '目标授权记录无效。'
  if (probe.enabled && (!authorization?.enabled || !authorization.source.trim() || !authorization.scope.trim() || !authorizationMatches(probe) || authorization.expires_at !== null && authorization.expires_at * 1000 <= now)) return '请明确登记当前目标的有效授权；草稿已保留。'
  return ''
}
export function initialProbePayloads(probes: Probe[]) {
  if (probes.length > 32) throw new Error('初始拨测目标最多 32 个。')
  return probes.map(probe => { const error = probePayloadError(probe); if (error) throw new Error(error); const { revision: _revision, task_id: _task, ...payload } = bindProbeAuthorization(probe); return payload })
}
export function currentServerProbe(snapshot: ProbeWriteSnapshot, expectedServer: number, original: Probe): Probe {
  const error = probeReadError(snapshot.probes)
  if (error) throw new Error(error)
  if (snapshot.serverId !== expectedServer) throw new Error('目标服务器已变化，请重新打开配置。')
  const current = currentValues(snapshot.probes)?.find(value => value.id === original.id)
  if (!current || current.task_id || original.task_id) throw new Error('拨测目标或版本已变化；草稿已保留。')
  const changed = probeWriteError(original, current); if (changed) throw new Error(changed)
  return current
}
export async function saveServerProbe<T>(snapshot: ProbeWriteSnapshot, expectedServer: number, original: Probe | null, value: Probe, writer: (body: Probe) => Promise<T>) {
  const error = probeReadError(snapshot.probes) || probePayloadError(value)
  if (error) throw new Error(error)
  if (snapshot.serverId !== expectedServer) throw new Error('目标服务器已变化；草稿已保留。')
  const current = original ? currentServerProbe(snapshot, expectedServer, original) : null
  if (!current && (currentValues(snapshot.probes)?.length ?? 32) >= 32) throw new Error('当前服务器的拨测目标已达上限。')
  if (current && (value.id !== current.id || !sameIdentity(probeIdentity(value), probeIdentity(current)))) throw new Error('目标、端口、方法或地址家族不能更换，请新建拨测。')
  return writer({ ...bindProbeAuthorization(value), ...(current ? { revision: current.revision } : {}) })
}
export async function deleteServerProbe<T>(snapshot: ProbeWriteSnapshot, expectedServer: number, original: Probe, writer: (id: string, body: { revision: number }) => Promise<T>) {
  const current = currentServerProbe(snapshot, expectedServer, original)
  return writer(current.id, { revision: current.revision! })
}
export function latencyDraftError(snapshot: TaskWriteSnapshot, original: LatencyTask | null, serverIds: number[]) {
  const error = probeReadError(snapshot.tasks, snapshot.servers)
  if (error) return error
  const current = original && currentValues(snapshot.tasks)?.find(value => value.id === original.id)
  if (original && (!current || !probeRevisionMatches(original, current) || !sameIdentity(probeIdentity(original.spec), probeIdentity(current.spec)))) return '延迟任务目标或版本已变化；草稿已保留。'
  if (serverIds.some(id => !Number.isSafeInteger(id) || id <= 0 || !currentValues(snapshot.servers)?.some(server => server.id === id)) || new Set(serverIds).size !== serverIds.length) return '已选服务器已不存在或标识无效，原选择保留。'
  return ''
}
export async function saveLatencyTask<T>(snapshot: TaskWriteSnapshot, original: LatencyTask | null, value: LatencyTask, writer: (body: { spec: Probe; default_enabled: boolean; server_ids: number[]; revision?: number }) => Promise<T>) {
  const error = latencyDraftError(snapshot, original, value.server_ids) || probePayloadError(value.spec)
  if (error) throw new Error(error)
  if (!original && (currentValues(snapshot.tasks)?.length ?? 32) >= 32) throw new Error('统一延迟任务已达上限。')
  if (original && (value.id !== original.id || value.spec.id !== original.spec.id || !sameIdentity(probeIdentity(value.spec), probeIdentity(original.spec)))) throw new Error('目标、端口、方法或地址家族不能更换，请新建任务。')
  return writer({ spec: bindProbeAuthorization(value.spec), default_enabled: value.default_enabled, server_ids: value.server_ids, ...(original ? { revision: original.revision } : {}) })
}
export async function deleteLatencyTask<T>(snapshot: TaskWriteSnapshot, original: LatencyTask, writer: (id: string, body: { revision: number }) => Promise<T>) {
  const error = latencyDraftError(snapshot, original, [])
  if (error) throw new Error(error)
  return writer(original.id, { revision: original.revision })
}
