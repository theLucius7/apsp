import { nonnegativeNumber as number } from '@/lib/validate'
import { bytes, numberText } from '../../format'
import type { Metrics, Server } from '../../types'
import type { Tone } from '@/components/board'

export type Sample = { id: string; sampled_at: number; metrics: Metrics }
export type NetworkField = 'received_bytes' | 'transmitted_bytes' | 'receive_bytes_per_sec' | 'transmit_bytes_per_sec'


export function ratio(used: unknown, total: unknown): number | null {
  const amount = number(used), capacity = number(total)
  return amount !== null && capacity !== null && capacity > 0 ? amount / capacity * 100 : null
}

export const percentage = (value: unknown) => number(value) === null ? '—' : `${(value as number).toFixed(1)}%`
export const size = (value: unknown) => number(value) === null ? '—' : bytes(value as number)
export const speed = (value: unknown) => number(value) === null ? '—' : `${bytes(value as number)}/秒`
export const count = (value: unknown) => number(value) === null ? '—' : numberText(value as number)

export function interfaceSelected(name: string, patterns = ''): boolean {
  const entries = patterns.split(',').map(value => value.trim()).filter(Boolean)
  const matches = (pattern: string) => new RegExp(`^${pattern.split('*').map(part => part.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('.*')}$`).test(name)
  return (!entries.some(pattern => !pattern.startsWith('!')) || entries.some(pattern => !pattern.startsWith('!') && matches(pattern))) && !entries.some(pattern => pattern.startsWith('!') && matches(pattern.slice(1)))
}

export function network(metrics: Metrics, field: NetworkField, patterns = ''): number | null {
  const interfaces = Object.entries(metrics.network_interfaces ?? {}).filter(([name]) => interfaceSelected(name, patterns)).map(([, metrics]) => metrics)
  if (!interfaces.length || interfaces.some(item => number(item[field]) === null)) return null
  return interfaces.reduce((sum, item) => sum + item[field]!, 0)
}

export function fresh(server: Server): boolean {
  return server.online && !server.metrics_stale && number(server.metrics_sampled_at) !== null && server.metrics_sampled_at! > 0
}

export function status(server: Server, unavailable = false): { label: string; tone: Tone } {
  if (unavailable) return { label: '状态未知', tone: 'warning' }
  if (server.online) return { label: '在线', tone: 'good' }
  return (server.registered ?? Boolean(server.device_public_key)) ? { label: '离线', tone: 'danger' } : { label: '待接入', tone: 'muted' }
}

export function aggregate(servers: Server[], field: NetworkField, live = false) {
  const values = servers.map(server => !live || fresh(server) ? network(server.latest_metrics, field, server.asset_settings?.network_interface) : null)
    .filter((value): value is number => value !== null)
  return { value: values.length ? values.reduce((sum, value) => sum + value, 0) : null, count: values.length }
}

export function filterServers(servers: Server[], query: string, filter: string) {
  const text = query.trim().toLocaleLowerCase()
  return servers.filter(server => {
    const matches = [server.name, server.static_info.hostname, server.static_info.system, server.static_info.arch, server.asset_settings?.region, server.asset_settings?.group_name, ...(server.asset_settings?.tags ?? [])]
      .filter(Boolean).join(' ').toLocaleLowerCase().includes(text)
    return matches && (filter === 'all' || (filter === 'online' ? server.online : filter === 'offline' ? !server.online && (server.registered ?? Boolean(server.device_public_key)) : !server.online && !(server.registered ?? Boolean(server.device_public_key))))
  })
}

export function sampleGap(samples: Sample[]): number {
  const times = [...new Set(samples.map(sample => sample.sampled_at))].sort((a, b) => a - b)
  const differences = times.slice(1).map((time, index) => time - times[index]).filter(value => value > 0).sort((a, b) => a - b)
  // Agent sampling intervals are 1–60 seconds. Sparse outages must not become the baseline.
  const interval = Math.min(60_000, differences[Math.floor(differences.length / 2)] ?? 5000)
  return Math.max(15_000, interval * 3)
}
