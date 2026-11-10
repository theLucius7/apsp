export type TrafficPoint = { day: number; uploaded: string | null; downloaded: string | null; total: string | null; incomplete?: boolean; sampled_servers?: number }
export type TrafficRanking = { id: number; name: string; uploaded: string; downloaded: string; total: string; deleted?: boolean; incomplete?: boolean }
type TrafficTotal = { uploaded: string | null; downloaded: string | null; total: string | null }
export type ServerStatistics = {
  generated_at: number; from: number; days: number
  servers: { total: number; online: number; offline: number; pending: number; hidden: number }
  traffic: TrafficTotal & { sampled_servers: number; incomplete: boolean; last_sample_at: number | null }
  points: TrafficPoint[]; by_server: TrafficRanking[]
}
export type ProxyStatistics = {
  generated_at: number; from: number; days: number; nodes: number; users: number
  traffic: TrafficTotal & { recorded_users: number; recorded_nodes: number; last_record_at: number | null }
  points: TrafficPoint[]; by_user: TrafficRanking[]; by_node: TrafficRanking[]
}
export function amount(value: string | null | undefined): bigint | null {
  return value != null && /^\d+$/.test(value) ? BigInt(value) : null
}
export function ratio(value: string | null | undefined, maximum: bigint): number {
  const number = amount(value)
  if (number === null || maximum <= 0n) return 0
  return Number((number > maximum ? maximum : number) * 10_000n / maximum) / 100
}
export const utcDay = (day: number) => new Date(day * 1000).toISOString().slice(5, 10).replace('-', '/')
