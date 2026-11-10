import { expect, test } from 'bun:test'
import type { Server } from '../src/types'
import { defaultAssets } from '../src/resources/server-assets'
import { dashboardCounts, dashboardRoute, dashboardServer, savedSort, savedView, selectServers, snapshotUnavailable } from '../src/observability/display/dashboard'

const base: Server = { id: 1, name: '服务器 1', device_public_key: 'TEST_ONLY', online: true, last_seen: 1000, last_heartbeat_at: 1000, metrics_sampled_at: 1000, metrics_stale: false, static_info: { system: 'Debian', memory_total: 100, disk_total: 100 }, latest_metrics: { cpu_percent: 0, memory_used: 0, disk_used: 0, network_interfaces: { eth0: { transmit_bytes_per_sec: 0, receive_bytes_per_sec: 0 } } }, manifest_rev: 0 }

const inventory: Server[] = [
  base,
  { ...base, id: 2, name: '服务器 10', latest_metrics: { cpu_percent: 95, memory_used: 80, disk_used: 15, network_interfaces: { eth0: { transmit_bytes_per_sec: 512, receive_bytes_per_sec: 1024 } } } },
  { ...base, id: 3, online: false, latest_metrics: { cpu_percent: 100 } },
  { ...base, id: 4, metrics_stale: true, latest_metrics: { cpu_percent: 100 } },
  { ...base, id: 5, online: false, device_public_key: null },
  { ...base, id: 6, metrics_sampled_at: null },
]

test('dashboard canonical routes and previous bookmarks keep strict server identities', () => {
  for (const path of ['/', '/overview', '/dashboard']) expect(dashboardRoute(path)).toEqual({})
  for (const path of ['/overview/12', '/dashboard/12']) expect(dashboardRoute(path)).toEqual({ serverId: 12 })
  for (const path of ['/dashboard/0', '/dashboard/01', '/dashboard/-1', '/dashboard/1.1', '/dashboard/9007199254740992', '/dashboard/1/extra', '/servers/1', '/dashboard/%31']) expect(dashboardRoute(path)).toBeNull()
  expect(dashboardServer(12)).toBe('#/dashboard/12')
})

test('online status and missing/stale telemetry are distinct counts', () => {
  expect(dashboardCounts(inventory)).toEqual({ all: 6, online: 4, offline: 1, pending: 1, stale: 2 })
  expect(selectServers(inventory, '', 'stale', '', '', 'default', false).map(s => s.id)).toEqual([4, 6])
  expect(selectServers(inventory, '', 'offline', '', '', 'default', false).map(s => s.id)).toEqual([3])
  expect(selectServers(inventory, '', 'pending', '', '', 'default', false).map(s => s.id)).toEqual([5])
})

test('every filter and view excludes hidden inventory and combines asset filters', () => {
  const servers = [
    { ...base, asset_settings: { ...defaultAssets, region: 'JP', group_name: '亚洲', tags: ['测试'] } },
    { ...base, id: 2, asset_settings: { ...defaultAssets, region: 'US', group_name: '美洲', tags: ['测试'] } },
    { ...base, id: 3, asset_settings: { ...defaultAssets, region: 'JP', group_name: '亚洲', hidden: true, tags: ['测试'] } },
  ]
  expect(selectServers(servers, ' 测试 ', 'online', '亚洲', 'JP', 'name', false).map(s => s.id)).toEqual([1])
  expect(selectServers(servers, '', 'all', '', '', 'default', false).map(s => s.id)).toEqual([1, 2])
  expect(selectServers(servers, '', 'all', '不存在', '', 'default', false)).toEqual([])
})

test('metric sorting puts known live measurements before missing data without treating zero as absent', () => {
  for (const sort of ['cpu', 'memory', 'network'] as const) {
    expect(selectServers(inventory, '', 'all', '', '', sort, false).map(s => s.id)).toEqual([2, 1, 3, 4, 5, 6])
    expect(selectServers(inventory, '', 'all', '', '', sort, true).map(s => s.id)).toEqual([1, 2, 3, 4, 5, 6])
  }
  const onlyCpu = { ...base, id: 7, latest_metrics: { cpu_percent: 30 } }
  expect(selectServers([onlyCpu, base], '', 'all', '', '', 'network', false).map(s => s.id)).toEqual([1, 7])
})

test('attention sorting is explicit, stable, and never mutates the input collection', () => {
  const before = structuredClone(inventory)
  expect(selectServers(inventory, '', 'all', '', '', 'attention', false).map(s => s.id)).toEqual([3, 4, 6, 2, 5, 1])
  expect(inventory).toEqual(before)
  expect(selectServers(inventory, '', 'all', '', '', 'attention', true).map(s => s.id)).toEqual([1, 2, 3, 4, 5, 6])
})

test('paused, failed, old, or invalid snapshots cannot be presented as live', () => {
  expect(snapshotUnavailable(1000, 16000, false, '')).toBeFalse()
  expect(snapshotUnavailable(1000, 16001, false, '')).toBeTrue()
  expect(snapshotUnavailable(1000, 1000, true, '')).toBeTrue()
  expect(snapshotUnavailable(1000, 1000, false, '读取失败')).toBeTrue()
  for (const time of [null, NaN, Infinity, 62000]) expect(snapshotUnavailable(time, 1000, false, '')).toBeTrue()
})

test('browser preferences only accept supported display values', () => {
  expect(savedView('table')).toBe('table')
  for (const value of [null, 'invalid', '{}', 'cards']) expect(savedView(value)).toBe('cards')
  expect(savedSort('network')).toBe('network')
  for (const value of [null, 'invalid', '{}']) expect(savedSort(value)).toBe('default')
})
