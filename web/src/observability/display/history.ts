import { number } from './data'
import type { Point } from './data'

export const historyWindows = [
  { value: '15m', minutes: 15, label: '15 分钟' }, { value: '1h', minutes: 60, label: '1 小时' },
  { value: '2h', minutes: 120, label: '2 小时' }, { value: '24h', minutes: 1440, label: '24 小时' },
  { value: '7d', minutes: 10080, label: '7 天' }, { value: '30d', minutes: 43200, label: '30 天' },
] as const
export type HistoryWindow = typeof historyWindows[number]['value']
export type MetricAggregate = { count: number; avg: number; min: number; max: number }
export type HistoryBucket = {
  bucket_at: number; sample_count: number; first_sampled_at: number; last_sampled_at: number
  metrics: Record<string, MetricAggregate>; partial: boolean
  network_counters: Record<string, { sampled_at: number; received_bytes: string | null; transmitted_bytes: string | null }>
}
export type AggregateHistory = { window: HistoryWindow; from: number; to: number; bucket_ms: number; retention_days: number; points: HistoryBucket[] }
export const windowMinutes = (window: HistoryWindow) => historyWindows.find(item => item.value === window)!.minutes
export const historyPollInterval = (bucketMs: number) => Math.max(15_000, Math.min(300_000, bucketMs))
export const duration = (milliseconds: number) => milliseconds >= 3_600_000 ? `${milliseconds / 3_600_000} 小时` : milliseconds >= 60_000 ? `${milliseconds / 60_000} 分钟` : `${milliseconds / 1000} 秒`

export function aggregatePoints(history: AggregateHistory, key: string): Point[] {
  return history.points.map(bucket => {
    const metric = bucket.metrics[key]
    const at = Math.max(history.from, Math.min(history.to, bucket.bucket_at + history.bucket_ms / 2))
    if (!metric || !Number.isSafeInteger(metric.count) || metric.count < 1 || [metric.avg, metric.min, metric.max].some(value => number(value) === null) || metric.min > metric.avg || metric.avg > metric.max) return { at, value: null }
    return { at, value: metric.avg, range: { from: bucket.first_sampled_at, to: bucket.last_sampled_at,
      count: metric.count, min: metric.min, max: metric.max, samples: bucket.sample_count, partial: bucket.partial,
      bucketFrom: bucket.bucket_at, bucketTo: bucket.bucket_at + history.bucket_ms } }
  }).sort((a, b) => a.at - b.at)
}

// Append only a real sample newer than every persisted bucket. This point is
// explicitly labelled live and does not invent an aggregate or a durable ACK.
export function appendLive(points: Point[], history: AggregateHistory, at: number | null, value: number | null, now: number): Point[] {
  const last = history.points.reduce((latest, bucket) => Math.max(latest, bucket.last_sampled_at), 0)
  if (at === null || at <= last || at < history.from || at > now || number(value) === null) return points
  // A bucket midpoint can be later than its newest sample. Keep the live point
  // separate and ordered without displaying a future aggregate coordinate.
  return [...points.map(point => point.at > at ? { ...point, at: Math.min(point.range?.to ?? point.at, at) } : point),
    { at, value, range: { from: at, to: at, count: 1, min: value!, max: value!, samples: 1, partial: false, live: true } }]
}
