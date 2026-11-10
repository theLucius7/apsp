import { expect, test } from 'bun:test'
import { aggregate, filterServers, fresh, network, number, percentage, ratio, sampleGap, segments, size, status } from '../src/observability/display/data'
import type { Sample } from '../src/observability/display/data'
import type { Server } from '../src/types'

const server: Server = { id: 1, name: '测试服务器', device_public_key: 'TEST_ONLY', static_info: { system: 'FreeBSD', arch: 'arm64' }, last_seen: 1, last_heartbeat_at: 1, metrics_sampled_at: 1000, metrics_stale: false, online: true, latest_metrics: { network_interfaces: { eth0: { transmit_bytes_per_sec: 0, receive_bytes_per_sec: 1024 } } }, manifest_rev: 0 }

test('display distinguishes real zero from absent, invalid and zero-capacity readings', () => {
  for (const value of [undefined, null, NaN, Infinity, -1, '0']) expect(number(value)).toBeNull()
  expect(size(null)).toBe('—')
  expect(percentage(0)).toBe('0.0%')
  expect(ratio(0, 1024)).toBe(0)
  expect(ratio(0, 0)).toBeNull()
  expect(network({}, 'transmit_bytes_per_sec')).toBeNull()
  expect(network(server.latest_metrics, 'transmit_bytes_per_sec')).toBe(0)
  expect(network({ network_interfaces: { eth0: { transmit_bytes_per_sec: 512 }, eth1: {} } }, 'transmit_bytes_per_sec')).toBeNull()
})

test('live totals exclude offline, stale and undated samples without changing online status', () => {
  const stale = { ...server, metrics_stale: true }
  const unknown = { ...server, metrics_sampled_at: null }
  const offline = { ...server, online: false }
  expect(fresh(server)).toBeTrue()
  for (const entry of [stale, unknown, offline]) expect(fresh(entry)).toBeFalse()
  expect(aggregate([server, stale, unknown, offline], 'receive_bytes_per_sec', true)).toEqual({ value: 1024, count: 1 })
  expect(aggregate([offline], 'receive_bytes_per_sec', true)).toEqual({ value: null, count: 0 })
  expect(status(stale).label).toBe('在线')
  expect(status(stale, true).label).toBe('状态未知')
})

test('search and filters preserve server identities and distinguish pending enrollment', () => {
  const offline = { ...server, id: 2, online: false }
  const pending = { ...offline, id: 3, device_public_key: null }
  const all = [server, offline, pending]
  expect(filterServers(all, '  FREEbsd ', 'online').map(value => value.id)).toEqual([1])
  expect(filterServers(all, 'arm64', 'offline').map(value => value.id)).toEqual([2])
  expect(filterServers(all, '', 'pending').map(value => value.id)).toEqual([3])
  expect(filterServers(all, 'not-found', 'all')).toHaveLength(0)
})

test('history leaves missing values and outages disconnected, with zero samples visible', () => {
  const points = [{ at: 2, value: 0 }, { at: 1, value: 5 }, { at: 3, value: null }, { at: 4, value: 0 }, { at: 100, value: 8 }]
  expect(segments(points, 0, 100, 10)).toEqual([[points[1], points[0]], [points[3]], [points[4]]])
  expect(segments(points, 2, 4, 10)).toEqual([[points[0]], [points[3]]])
  const samples = [0, 1000, 2000, 100_000].map(sampled_at => ({ sampled_at })) as Sample[]
  expect(sampleGap(samples)).toBe(15_000)
  expect(sampleGap([{ sampled_at: 0 }, { sampled_at: 1_000_000 }] as Sample[])).toBe(180_000)
})

test('bounded chart rendering retains short spikes and dips', () => {
  const points = Array.from({ length: 7200 }, (_, at) => ({ at, value: at === 301 ? 100 : at === 302 ? 0 : 10 }))
  const reduced = segments(points, 0, 7200, 2).flat()
  expect(reduced.length).toBeLessThanOrEqual(180 * 4)
  expect(reduced.some(point => point.value === 100)).toBeTrue()
  expect(reduced.some(point => point.at === 302 && point.value === 0)).toBeTrue()
  expect(reduced[0].at).toBe(0)
  expect(reduced[reduced.length - 1].at).toBe(7199)
})
