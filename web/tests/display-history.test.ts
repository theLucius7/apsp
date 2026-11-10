import { expect, test } from 'bun:test'
import { aggregatePoints, appendLive, historyPollInterval } from '../src/observability/display/history'
import type { AggregateHistory, HistoryBucket } from '../src/observability/display/history'
import { segments } from '../src/observability/display/data'

const bucket = (at: number, metrics: HistoryBucket['metrics']): HistoryBucket => ({ bucket_at: at, first_sampled_at: at + 100, last_sampled_at: at + 900, sample_count: 10, metrics, network_counters: { eth0: { sampled_at: at + 900, received_bytes: '18446744073709551615', transmitted_bytes: null } }, partial: false })
const history: AggregateHistory = { window: '24h', from: 500, to: 10_000, bucket_ms: 1000, retention_days: 30, points: [
  { ...bucket(0, { cpu_percent: { count: 7, avg: 25, min: 0, max: 90 } }), partial: true },
  bucket(1000, {}), bucket(2000, { cpu_percent: { count: 10, avg: 0, min: 0, max: 0 } }),
  bucket(8000, { cpu_percent: { count: 10, avg: 30, min: 10, max: 99 } }),
] }

test('aggregate plots retain real metric counts, extrema, boundaries and missing intervals', () => {
  const points = aggregatePoints(history, 'cpu_percent')
  expect(points[0]).toEqual({ at: 500, value: 25, range: { from: 100, to: 900, count: 7, min: 0, max: 90, samples: 10, partial: true, bucketFrom: 0, bucketTo: 1000 } })
  expect(points[1].value).toBeNull()
  expect(points[2].value).toBe(0)
  expect(segments(points, history.from, history.to, 1500).map(part => part.length)).toEqual([1, 1, 1])
  expect(aggregatePoints(history, 'received_bytes').every(point => point.value === null)).toBeTrue()
  expect(history.points[0].network_counters.eth0.received_bytes).toBe('18446744073709551615')
  const invalid = { ...history, points: [bucket(0, { cpu_percent: { count: 0, avg: 0, min: 0, max: 0 } }), bucket(1000, { cpu_percent: { count: 1, avg: 2, min: 3, max: 4 } })] }
  expect(aggregatePoints(invalid, 'cpu_percent').every(point => point.value === null)).toBeTrue()
})

test('latest real sample is distinct from aggregates and never duplicates, replays or forecasts a point', () => {
  const points = aggregatePoints(history, 'cpu_percent')
  const live = appendLive(points, history, 9200, 0, 9500)
  expect(live).toHaveLength(points.length + 1)
  expect(live.at(-1)).toEqual({ at: 9200, value: 0, range: { from: 9200, to: 9200, count: 1, min: 0, max: 0, samples: 1, partial: false, live: true } })
  for (const [at, value] of [[8900, 0], [8800, 12], [9600, 3], [9200, null], [null, 0]]) expect(appendLive(points, history, at, value, 9500)).toEqual(points)
  expect(historyPollInterval(2000)).toBe(15_000)
  expect(historyPollInterval(120_000)).toBe(120_000)
  expect(historyPollInterval(3_600_000)).toBe(300_000)
})

test('chart reduction preserves a peak inside an aggregate with an ordinary mean', () => {
  const points = Array.from({ length: 2000 }, (_, i) => ({ at: i, value: 10, range: { from: i, to: i, count: 1, samples: 1, min: i === 377 ? 0 : 8, max: i === 376 ? 100 : 12, partial: false } }))
  const reduced = segments(points, 0, 2000, 2).flat()
  expect(reduced.length).toBeLessThanOrEqual(180 * 4)
  expect(reduced.some(point => point.range?.max === 100)).toBeTrue()
  expect(reduced.some(point => point.range?.min === 0)).toBeTrue()
})
