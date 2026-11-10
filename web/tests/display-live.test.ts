import { expect, test } from 'bun:test'
import { mergeLiveServers, mergeLiveSnapshot } from '../src/observability/display/live'
import type { LiveSnapshot } from '../src/observability/display/live'
import type { Server } from '../src/types'

const node = { id: 1, name: '节点一', public_view: false, metrics_sampled_at: 2000, latest_metrics: { cpu_percent: 20 } } as Server
test('live visibility snapshots prune removed nodes and only allow safe newer metrics', () => {
  const update = { id: 1, online: true, metrics_sampled_at: 1000, latest_metrics: { cpu_percent: 10 }, name: '覆盖名称', asset_settings: { price: '999' } }
  const live = { served_at: 3000, public_view: false, servers: [update] } as unknown as LiveSnapshot
  const result = mergeLiveServers([node, { ...node, id: 2 }], live)
  expect(result).toHaveLength(1)
  expect(result[0].name).toBe('节点一')
  expect(result[0].latest_metrics.cpu_percent).toBe(20)
  expect(result[0].asset_settings).toBeUndefined()
  expect(mergeLiveServers([node], { ...live, public_view: true })).toEqual([])
  expect(mergeLiveServers([node], { ...live, servers: [] })).toEqual([])
})

test('consecutive live reads retain the newest real sample even while metadata is older', () => {
  const sample = (at: number, cpu: number) => ({ id: 1, online: true, last_seen: 3, last_heartbeat_at: 3, metrics_stale: false,
    metrics_sampled_at: at, metrics_received_at: at + 10, metrics_persisted_at: 2000, latest_metrics: { cpu_percent: cpu } })
  const first: LiveSnapshot = { served_at: 4000, public_view: false, servers: [sample(3000, 30)] }
  const delayed: LiveSnapshot = { served_at: 5000, public_view: false, servers: [{ ...sample(2500, 25), online: false, last_seen: 4, metrics_persisted_at: 2500 }] }
  const result = mergeLiveSnapshot(first, delayed)
  expect(result.served_at).toBe(5000)
  expect(result.servers[0]).toMatchObject({ online: false, last_seen: 4, metrics_sampled_at: 3000, metrics_received_at: 3010, metrics_persisted_at: 2500, latest_metrics: { cpu_percent: 30 } })
  expect(mergeLiveServers([node], result)[0].latest_metrics.cpu_percent).toBe(30)
  const newest = mergeLiveSnapshot(result, { ...first, servers: [sample(6000, 0)] })
  expect(newest.servers[0].latest_metrics.cpu_percent).toBe(0)
  expect(newest.servers[0].metrics_received_at).toBe(6010)
})

test('live sample retention never restores hidden servers or crosses visibility projections', () => {
  const previous: LiveSnapshot = { served_at: 5000, public_view: false, servers: [{ ...node, metrics_received_at: 2010 }, { ...node, id: 2 }] }
  const missing: LiveSnapshot = { served_at: 6000, public_view: false, servers: [] }
  expect(mergeLiveSnapshot(previous, missing).servers).toEqual([])
  const publicRead: LiveSnapshot = { ...missing, public_view: true, servers: [{ ...node, metrics_sampled_at: 1000, latest_metrics: { cpu_percent: 10 } }] }
  expect(mergeLiveSnapshot(previous, publicRead)).toBe(publicRead)
  expect(mergeLiveSnapshot(undefined, publicRead)).toBe(publicRead)
  const replay = { ...previous, servers: [{ ...node, metrics_sampled_at: 1000, metrics_received_at: 1010 }] }
  expect(mergeLiveServers([{ ...node, metrics_received_at: 2010 }], replay)[0].metrics_received_at).toBe(2010)
})

test('retained samples use their own age while connection state follows the latest response', () => {
  const previous: LiveSnapshot = { served_at: 101_000, public_view: false, servers: [{ ...node, online: true, metrics_sampled_at: 100_000, metrics_received_at: 100_010, metrics_stale: false }] }
  const older = (served_at: number, stale = true): LiveSnapshot => ({ served_at, public_view: false,
    servers: [{ ...node, online: false, metrics_sampled_at: 40_000, metrics_received_at: 40_010, metrics_stale: stale }] })
  const retained = mergeLiveSnapshot(previous, older(103_000))
  expect(retained.servers[0]).toMatchObject({ online: false, metrics_sampled_at: 100_000, metrics_received_at: 100_010, metrics_stale: false })
  expect(mergeLiveServers(previous.servers as Server[], older(103_000))[0].metrics_stale).toBe(false)
  expect(mergeLiveSnapshot(retained, older(115_000, false)).servers[0].metrics_stale).toBe(false)
  const expired = mergeLiveSnapshot(retained, older(115_001, false))
  expect(expired.servers[0].metrics_stale).toBe(true)
  expect(mergeLiveSnapshot(expired, older(116_000, false)).servers[0].metrics_stale).toBe(true)
  const resumed = { ...older(117_000), servers: [{ ...previous.servers[0], online: true, metrics_sampled_at: 117_000, metrics_stale: false }] }
  expect(mergeLiveSnapshot(expired, resumed).servers[0]).toMatchObject({ online: true, metrics_sampled_at: 117_000, metrics_stale: false })
  expect(mergeLiveSnapshot(previous, older(99_000)).servers[0].metrics_stale).toBe(true)
  expect(mergeLiveSnapshot(previous, older(NaN)).servers[0].metrics_stale).toBe(true)
})
