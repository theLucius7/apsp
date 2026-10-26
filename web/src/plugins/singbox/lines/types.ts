import type { QualityField } from '../../../types'

// Member line catalogue and probe panel (ADR 0101).
export type Carrier = 'telecom' | 'unicom' | 'mobile'
export type CarrierLatency = { latest?: number | null; average?: number | null; loss?: number | null }
export type Latency = {
  monitored: boolean; source?: 'apsp_probe' | 'legacy_monitor'; updated_at?: number | null; window_minutes?: number
  direction?: 'node_to_carrier'; carriers?: Partial<Record<Carrier, CarrierLatency>>; referenced?: boolean
}
export type Point = { id: string; role: 'entry' | 'egress'; region: string | null; provider: string | null; node_id: number | null }
export type Edge = { from: string; to: string; traffic: 'all' | 'ai' | 'apple_news' }
export type Topology = {
  region: string | null; provider: string | null; residential: boolean; dynamic: boolean
  classification_source: 'catalog_tag' | 'catalog_name' | 'node_catalog'
  kind: 'direct' | 'relay' | 'policy'; source: 'catalog_tag' | 'shared_ingress' | 'node_catalog' | 'unresolved'
  inferred: boolean; position_precision: 'region'; front_node_id: number | null; points: Point[]; edges: Edge[]
}
export type LineState = 'online' | 'entry_online' | 'offline' | 'unknown'
export type StatusSource = 'apsp' | 'legacy' | 'unknown'
export type Line = {
  id: number; name: string; protocol: { name: string; network: string; security: 'reality' | 'tls' | 'none' }
  topology: Topology; status: { state: LineState; checked_at: number | null; source: StatusSource }
  rate: string | null; timed_rate: boolean; latency: Latency; host: string | null
  network_scope: 'entry' | 'egress_unverified'; plans: { name: string; current: boolean }[]; access: boolean
}
export type Dataset = { source: string; database: string; checked_at: number | null; historical?: boolean; fields: QualityField[] }
export type Family = { family: 'IPv4' | 'IPv6'; summary: { asn?: unknown; country?: unknown; organization?: unknown }; datasets: Dataset[] }
export type HostNetwork = { available: false; reason?: 'no_agent' } | { available: true; checked_at: number | null; families: Family[] }
export type Host = {
  key: string; region: string | null; provider: string | null; residential_declared: boolean
  online: boolean; checked_at: number | null; source: StatusSource; load: { cpu?: number | null; memory?: number | null; disk?: number | null }
  uptime_secs: number | null; latency: Latency; network: HostNetwork; lines: number[]
}
export type LinesView = { generated_at: number; package: string | null; lines: Line[]; hosts: Host[] }
