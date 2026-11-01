import { expect, test } from 'bun:test'
import { asnLabel, bestLatency, defaultFilter, filterLines, flag, kindBadges, protocolLabel, rateLabel, riskFlags, routeLabel } from '../src/plugins/singbox/lines/atlas'
import type { Host, Line, Topology } from '../src/plugins/singbox/lines/types'

const direct: Topology = { region: 'HK', provider: 'Example', residential: false, dynamic: false, classification_source: 'node_catalog', kind: 'direct', source: 'node_catalog', inferred: false, position_precision: 'region', front_node_id: null, points: [{ id: 'entry', role: 'entry', region: 'HK', provider: 'Example', node_id: 1 }], edges: [] }
const relay: Topology = { ...direct, region: 'JP', residential: true, dynamic: true, classification_source: 'catalog_tag', kind: 'relay', source: 'shared_ingress', inferred: true, front_node_id: 1, points: [direct.points[0], { id: 'egress', role: 'egress', region: 'JP', provider: null, node_id: 2 }], edges: [{ from: 'entry', to: 'egress', traffic: 'all' }] }
const line = (id: number, topology: Topology, extra: Partial<Line> = {}): Line => ({ id, name: `线路 ${id}`, protocol: { name: 'vless', network: 'tcp', security: 'reality' }, topology, status: { state: 'online', checked_at: 1, source: 'legacy' }, rate: '1', timed_rate: false, latency: { monitored: false }, host: 'h1', network_scope: 'entry', plans: [], access: false, ...extra })

test('route labels and badges describe the hop chain', () => {
  expect(flag('hk')).toBe('🇭🇰')
  expect(flag(null)).toBe('🌐')
  expect(routeLabel(relay)).toContain('→')
  expect(kindBadges(direct).map(badge => badge.label)).toEqual(['直连入口'])
  expect(kindBadges(relay).map(badge => badge.label)).toEqual(['转发线路', '动态家宽'])
  expect(protocolLabel({ name: 'hysteria2', network: 'udp', security: 'tls' })).toBe('Hysteria2 · UDP · TLS')
  expect(rateLabel('1.50')).toBe('1.5×')
  expect(rateLabel(null)).toBe('倍率未知')
  expect(asnLabel(4134)).toBe('AS4134')
  expect(asnLabel('AS9808')).toBe('AS9808')
  expect(asnLabel('Example')).toBeUndefined()
})

test('filters keep catalogue order and sort by latency or rate', () => {
  const lines = [
    line(1, direct, { rate: '2', latency: { monitored: true, carriers: { telecom: { latest: 90 }, mobile: { latest: 40 } } } }),
    line(2, relay, { access: true, status: { state: 'offline', checked_at: null, source: 'unknown' }, rate: '0.5' }),
    line(3, direct, { latency: { monitored: true, carriers: { unicom: { latest: 20 } } } }),
  ]
  expect(bestLatency(lines[0].latency)).toBe(40)
  expect(bestLatency({ monitored: false })).toBeUndefined()
  expect(filterLines(lines, defaultFilter).map(item => item.id)).toEqual([1, 2, 3])
  expect(filterLines(lines, { ...defaultFilter, sort: 'latency' }).map(item => item.id)).toEqual([3, 1, 2])
  expect(filterLines(lines, { ...defaultFilter, sort: 'rate' }).map(item => item.id)).toEqual([2, 3, 1])
  expect(filterLines(lines, { ...defaultFilter, scope: 'mine' }).map(item => item.id)).toEqual([2])
  expect(filterLines(lines, { ...defaultFilter, scope: 'online' }).map(item => item.id)).toEqual([1, 3])
  expect(filterLines(lines, { ...defaultFilter, kind: 'residential' }).map(item => item.id)).toEqual([2])
  expect(filterLines(lines, { ...defaultFilter, region: 'JP' }).map(item => item.id)).toEqual([2])
  expect(filterLines(lines, { ...defaultFilter, query: '线路 3' }).map(item => item.id)).toEqual([3])
})

test('risk flags list each measured flag once', () => {
  const host: Host = { key: 'h1', region: 'HK', provider: null, residential_declared: false, online: true, checked_at: 1, source: 'apsp', load: {}, uptime_secs: null, latency: { monitored: false }, lines: [1], network: { available: true, checked_at: 1, families: [{ family: 'IPv4', summary: { asn: 1 }, datasets: [
    { source: 'A', database: 'a', checked_at: 1, fields: [{ label: '数据中心', value: true, kind: 'boolean' }, { label: 'VPN', value: false, kind: 'boolean' }] },
    { source: 'B', database: 'b', checked_at: 1, fields: [{ label: '数据中心', value: true, kind: 'boolean' }] },
  ] }] } }
  expect(riskFlags(host)).toEqual(['数据中心'])
  expect(riskFlags({ ...host, network: { available: false, reason: 'no_agent' } })).toEqual([])
})
