import type { Tone } from '../../../components'
import type { Carrier, Edge, Host, Latency, Line, LineState, Point, StatusSource, Topology } from './types'

export const carriers: { key: Carrier; label: string }[] = [{ key: 'telecom', label: '电信' }, { key: 'unicom', label: '联通' }, { key: 'mobile', label: '移动' }]

let names: Intl.DisplayNames | null | undefined
/** Chinese region name for an ISO code; the code itself when names are unavailable. */
export function regionName(code?: string | null) {
  const value = code?.trim().toUpperCase()
  if (!value) return '地区未知'
  if (!/^[A-Z]{2}$/.test(value)) return value
  if (names === undefined) { try { names = new Intl.DisplayNames(['zh-CN'], { type: 'region' }) } catch { names = null } }
  try { return names?.of(value) ?? value } catch { return value }
}

export function flag(code?: string | null) {
  const value = code?.trim().toUpperCase()
  return value && /^[A-Z]{2}$/.test(value) ? String.fromCodePoint(...[...value].map(letter => 0x1f1e6 + letter.charCodeAt(0) - 65)) : '🌐'
}

export function kindBadges(topology: Topology): { label: string; tone: Tone }[] {
  const badges: { label: string; tone: Tone }[] = [{ label: topology.kind === 'direct' ? '直连入口' : topology.kind === 'relay' ? '转发线路' : '智能分流', tone: 'neutral' }]
  if (topology.residential) badges.push({ label: topology.dynamic ? '动态家宽' : '住宅宽带', tone: 'warm' })
  return badges
}

export const stateLabels: Record<LineState, { label: string; tone: Tone }> = {
  online: { label: '在线', tone: 'good' },
  entry_online: { label: '入口在线', tone: 'good' },
  offline: { label: '离线', tone: 'bad' },
  unknown: { label: '状态未知', tone: 'neutral' },
}

export const sourceLabels: Record<StatusSource, string> = { apsp: 'apsp 设备上报', legacy: '原面板监控', unknown: '暂无状态来源' }
export const trafficLabels: Record<Edge['traffic'], string> = { all: '全部流量', ai: 'AI 服务', apple_news: 'Apple News' }
export const topologySources: Record<Topology['source'], string> = {
  catalog_tag: '节点目录明确标注的前置入口',
  shared_ingress: '与已监控入口共用连接地址推断',
  node_catalog: '节点目录中的直连线路',
  unresolved: '目录未给出可确认的前置入口',
}

export function pointLabel(point: Point) {
  return point.region ? `${flag(point.region)} ${regionName(point.region)}` : '地区未知'
}

/** Route summary such as "香港 → 日本" or "香港 → 台湾 / 美国". */
export function routeLabel(topology: Topology) {
  const entry = topology.points.find(point => point.role === 'entry')
  const exits = topology.points.filter(point => point.role === 'egress')
  const name = (point?: Point) => point?.region ? regionName(point.region) : '未知'
  if (!exits.length) return name(entry)
  return `${name(entry)} → ${exits.map(name).join(' / ')}`
}

export function protocolLabel(protocol: Line['protocol']) {
  const name = { vless: 'VLESS', vmess: 'VMess', trojan: 'Trojan', shadowsocks: 'Shadowsocks', hysteria: 'Hysteria', hysteria2: 'Hysteria2', tuic: 'TUIC', anytls: 'AnyTLS' }[protocol.name] ?? protocol.name.toUpperCase()
  const security = { reality: 'Reality', tls: 'TLS', none: '' }[protocol.security]
  return [name, protocol.network.toUpperCase(), security].filter(Boolean).join(' · ')
}

export function rateLabel(rate: string | null) {
  const value = Number(rate)
  if (rate === null || rate === '' || !Number.isFinite(value)) return '倍率未知'
  return `${Number(value.toFixed(2))}×`
}

/** Lowest current latency across carriers, for sorting and the compact line view. */
export function bestLatency(latency: Latency) {
  if (!latency.monitored) return undefined
  const values = carriers.map(({ key }) => latency.carriers?.[key]?.latest).filter((value): value is number => typeof value === 'number' && Number.isFinite(value))
  return values.length ? Math.min(...values) : undefined
}

export function latencyTone(value?: number | null): Tone {
  if (typeof value !== 'number') return 'neutral'
  return value < 100 ? 'good' : value < 200 ? 'warm' : 'bad'
}

export function latencyLabel(value?: number | null) {
  return typeof value === 'number' && Number.isFinite(value) ? `${Math.round(value)} ms` : '—'
}

export type LineFilter = { query: string; region: string; kind: '' | Topology['kind'] | 'residential'; scope: 'all' | 'mine' | 'online'; sort: 'catalog' | 'latency' | 'rate' }
export const defaultFilter: LineFilter = { query: '', region: '', kind: '', scope: 'all', sort: 'catalog' }

/** Exit region of a line: the egress for relays, the entry for direct lines. */
export function exitRegion(line: Line) {
  return line.topology.region ?? line.topology.points.find(point => point.role === 'egress')?.region ?? line.topology.points[0]?.region ?? null
}

export function regions(lines: Line[]) {
  return [...new Set(lines.map(exitRegion).filter((value): value is string => Boolean(value)))].sort((a, b) => regionName(a).localeCompare(regionName(b), 'zh-CN'))
}

export function filterLines(lines: Line[], filter: LineFilter) {
  const query = filter.query.trim().toLowerCase()
  const shown = lines.filter(line => {
    if (query && ![line.name, regionName(exitRegion(line)), line.topology.provider ?? '', protocolLabel(line.protocol)].some(text => text.toLowerCase().includes(query))) return false
    if (filter.region && exitRegion(line) !== filter.region) return false
    if (filter.kind === 'residential' ? !line.topology.residential : filter.kind && line.topology.kind !== filter.kind) return false
    if (filter.scope === 'mine' && !line.access) return false
    if (filter.scope === 'online' && !['online', 'entry_online'].includes(line.status.state)) return false
    return true
  })
  if (filter.sort === 'catalog') return shown
  const key = (line: Line) => filter.sort === 'latency' ? bestLatency(line.latency) ?? Infinity : Number.isFinite(Number(line.rate)) && line.rate !== null ? Number(line.rate) : Infinity
  return shown.map((line, index) => ({ line, index })).sort((a, b) => key(a.line) - key(b.line) || a.index - b.index).map(({ line }) => line)
}

export function summary(lines: Line[]) {
  return {
    total: lines.length,
    online: lines.filter(line => ['online', 'entry_online'].includes(line.status.state)).length,
    mine: lines.filter(line => line.access).length,
    residential: lines.filter(line => line.topology.residential).length,
  }
}

/** Measured boolean flags that are set, such as 数据中心 or VPN, without duplicates. */
export function riskFlags(host: Host) {
  if (!host.network.available) return []
  const found = new Set<string>()
  for (const family of host.network.families) for (const dataset of family.datasets) for (const field of dataset.fields) if (field.kind === 'boolean' && field.value === true) found.add(field.label)
  return [...found]
}

export function asnLabel(value: unknown) {
  if (typeof value === 'number' && Number.isInteger(value) && value > 0) return `AS${value}`
  if (typeof value === 'string' && /^(?:AS)?\d{1,10}$/i.test(value.trim())) return `AS${value.trim().replace(/^AS/i, '')}`
  return undefined
}
