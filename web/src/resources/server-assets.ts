import type { ServerAssets } from '../types'

export const defaultAssets: ServerAssets = { region: '', group_name: '', tags: [], hidden: false, offline_notify: true, agent_mirror: '', price: null, currency: 'CNY', billing_cycle: 30, expires_at: null, auto_renewal: false, traffic_limit: '0', traffic_limit_type: 'sum', reset_day: 1, network_interface: '' }
export const trafficModes = { sum: '上下行合计', max: '取较大值', min: '取较小值', up: '仅上行', down: '仅下行' }
export const trafficUnits = { GB: 1_000_000_000n, TB: 1_000_000_000_000n, GiB: 1n << 30n, TiB: 1n << 40n, B: 1n }
export type TrafficUnit = keyof typeof trafficUnits
export type AssetDraft = Omit<ServerAssets, 'tags' | 'price' | 'expires_at' | 'traffic_limit' | 'billing_cycle' | 'reset_day'> & { tags: string; price: string; expiry: string; amount: string; unit: TrafficUnit; billing_cycle: string; reset_day: string }

export function assetDraft(settings?: ServerAssets): AssetDraft {
  const asset = { ...defaultAssets, ...settings }
  const bytes = BigInt(asset.traffic_limit)
  const unit = bytes === 0n ? 'GB' : (['TiB', 'GiB', 'TB', 'GB'] as const).find(unit => bytes >= trafficUnits[unit] && bytes * 1_000_000n % trafficUnits[unit] === 0n) ?? 'B'
  const scaled = bytes * 1_000_000n / trafficUnits[unit]
  const fraction = String(scaled % 1_000_000n).padStart(6, '0').replace(/0+$/, '')
  const amount = `${scaled / 1_000_000n}${fraction ? `.${fraction}` : ''}`
  return { ...asset, tags: asset.tags.join(', '), price: asset.price ?? '', expiry: asset.expires_at === null ? '' : new Date(asset.expires_at * 1000).toISOString().slice(0, 10), billing_cycle: String(asset.billing_cycle), reset_day: String(asset.reset_day), amount, unit }
}

export function parseTraffic(amount: string, unit: TrafficUnit): string {
  const value = amount.trim()
  if (!/^\d+(\.\d{1,6})?$/.test(value)) throw new Error('流量额度需为非负数，最多六位小数。')
  const [whole, fraction = ''] = value.split('.')
  const scale = 10n ** BigInt(fraction.length)
  const scaled = (BigInt(whole) * scale + BigInt(fraction || '0')) * trafficUnits[unit]
  if (scaled % scale !== 0n) throw new Error('流量额度换算后必须为整数字节。')
  const bytes = scaled / scale
  if (bytes > (1n << 64n) - 1n) throw new Error('流量额度不能超过 2^64−1 字节。')
  return bytes.toString()
}

export function assetPayload(draft: AssetDraft): ServerAssets {
  const expires_at = draft.expiry ? Date.parse(`${draft.expiry}T00:00:00Z`) / 1000 : null
  if (expires_at !== null && !Number.isFinite(expires_at)) throw new Error('到期日期无效。')
  return { region: draft.region.trim().toUpperCase(), group_name: draft.group_name.trim(), tags: [...new Set(draft.tags.split(/[,，]/).map(value => value.trim()).filter(Boolean))], hidden: draft.hidden, offline_notify: draft.offline_notify, agent_mirror: draft.agent_mirror.trim(), price: draft.price.trim() || null, currency: draft.currency, billing_cycle: Number(draft.billing_cycle), expires_at, auto_renewal: draft.auto_renewal, traffic_limit: parseTraffic(draft.amount, draft.unit), traffic_limit_type: draft.traffic_limit_type, reset_day: Number(draft.reset_day), network_interface: draft.network_interface.trim() }
}

export function trafficSize(value: string | null | undefined): string {
  if (value == null || !/^\d+$/.test(value)) return '—'
  const bytes = BigInt(value), units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB']
  let scale = 1n, index = 0
  while (bytes >= scale * 1024n && index < units.length - 1) { scale *= 1024n; index++ }
  const tenths = (bytes * 10n + scale / 2n) / scale
  return `${tenths / 10n}${index > 0 ? `.${tenths % 10n}` : ''} ${units[index]}`
}

export const assetDate = (value: number | null | undefined) => value == null ? '未设置' : new Date(value * 1000).toISOString().slice(0, 10)
export const assetPrice = (asset: ServerAssets) => asset.price === null ? '未填写成本' : `${asset.currency} ${asset.price} / ${asset.billing_cycle === 0 ? '一次性' : `${asset.billing_cycle} 天`}`
export function expiryState(asset: ServerAssets, now = Date.now()) {
  if (asset.expires_at === null) return { label: '未设置到期', tone: 'neutral' as const }
  const days = Math.ceil((asset.expires_at * 1000 - now) / 86_400_000)
  return days <= 0 ? { label: '已到期', tone: 'bad' as const } : days <= 7 ? { label: `${days} 天后到期`, tone: 'warm' as const } : { label: `${days} 天后到期`, tone: 'good' as const }
}
