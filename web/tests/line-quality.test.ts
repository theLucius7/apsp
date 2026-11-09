import { expect, test } from 'bun:test'
import { carrierFigures, carrierHeading, lineProblems, lineSample, matchLines, reachabilityFigures, serverLines, sortLines } from '../src/monitoring/line-quality'
import { worstState } from '../src/monitoring/reachability'
import type { Probe, ProbeOverview, ProbeResult } from '../src/probes'
import type { ReachabilityState, ReachabilityTarget } from '../src/monitoring/reachability'

const now = 1_800_000_000_000
const id = (n: number) => `00000000-0000-0000-0000-${String(n).padStart(12, '0')}`
const authorization = (target: string) => ({ kind: 'owned' as const, source: 'TEST_ONLY', scope: 'TEST_ONLY', enabled: true, expires_at: null, identity: { kind: 'icmp' as const, target, port: null, address_family: 'any' as const } })
function probe(n: number, network: 'telecom' | 'unicom' | 'mobile' | 'other', extra: Partial<Probe> = {}): Probe {
  const target = `192.0.2.${n}`
  return { id: id(n), name: `TEST_ONLY 广东${({ telecom: '电信', unicom: '联通', mobile: '移动', other: '其他' })[network]}`, kind: 'icmp', target, port: null, interval_secs: 60, carrier: '', enabled: true, monitor: { network, region: '广东', address_family: 'any', authorization: authorization(target) }, execution_authorized: true, ...extra }
}
function samples(p: Probe, values: [number | null, number][], at = now - 10_000): ProbeResult[] {
  return values.map(([latency_ms, loss_percent], k) => ({ id: id(1000 + k), probe_id: p.id, sampled_at: at - k * 60_000, latency_ms, loss_percent, error: null, attempts: 4 }))
}
const row = (server_id: number, p: Probe, values: [number | null, number][], at?: number): ProbeOverview => ({ server_id, probe: p, results: samples(p, values, at) })
const server = (n: number, name: string, extra: Record<string, unknown> = {}) => ({ id: n, name, online: true, asset_settings: { region: '🇭🇰', group_name: 'TEST_ONLY 厂商' }, ...extra })
function target(key: string, serverId: number, state: ReachabilityState, pending: ReachabilityState | null = null): ReachabilityTarget {
  return { key, host: `${key}.example.invalid`, port: 443, tcp: true, state, blocked: state.endsWith('blocked'), pending_state: pending, failing_streak: pending ? 1 : 0, observed_state: state, resolved_ip: null, detail: {}, checked_at: 1, state_changed_at: null, deferred_at: null, nodes: [{ kind: 'native', id: serverId * 10, name: `TEST_ONLY ${key}`, server_id: serverId, server_name: 'TEST_ONLY', protocol: 'vless-reality' }], risks: [] }
}

test('a sample reads the latest value and averages loss over the measured recent samples', () => {
  const p = probe(1, 'telecom')
  const sample = lineSample(p, samples(p, [[40, 25], [42, 0], [41, 0], [null, 100]]), now)
  expect(sample.state).toBe('最近采样')
  expect(sample.latency).toBe(40)
  expect(sample.loss).toBe(25)
  expect(sample.recentLoss).toBe(31.25)
  expect(sample.slots.filter(Boolean)).toHaveLength(4)
  // An Agent that could not measure reports an error without four attempts: no value, no loss.
  const failed = lineSample(p, [{ id: id(9), probe_id: p.id, sampled_at: now - 5_000, latency_ms: null, loss_percent: 100, error: 'TEST_ONLY no tool', attempts: 1 }], now)
  expect(failed.state).toBe('检测不可用')
  expect([failed.latency, failed.loss, failed.recentLoss]).toEqual([null, null, null])
})

test('each server gets one row with its carriers, other probes and endpoints', () => {
  const telecom = probe(1, 'telecom'), unicom = probe(2, 'unicom'), other = probe(3, 'other', { name: 'TEST_ONLY 主站' })
  const lines = serverLines([server(1, 'TEST_ONLY 香港'), server(2, 'TEST_ONLY 东京')], [row(1, telecom, [[20, 0]]), row(1, unicom, [[22, 0]]), row(1, other, [[5, 0]])], [target('a', 1, 'reachable'), target('b', 1, 'port_blocked'), target('c', 3, 'ip_blocked')], now)
  expect(lines.map(line => line.server.name)).toEqual(['TEST_ONLY 香港', 'TEST_ONLY 东京'])
  expect(lines[0].carriers.telecom.map(sample => sample.probe.id)).toEqual([telecom.id])
  expect(lines[0].carriers.mobile).toEqual([])
  expect(lines[0].others.map(sample => sample.probe.name)).toEqual(['TEST_ONLY 主站'])
  expect(lines[0].endpoints.map(endpoint => endpoint.key)).toEqual(['a', 'b'])
  expect(lines[0].reachability).toBe('port_blocked')
  expect(lines[0].server).toEqual({ id: 1, name: 'TEST_ONLY 香港', online: true, region: '🇭🇰', group: 'TEST_ONLY 厂商' })
  // Without endpoints or probes the row stays, so missing coverage is visible.
  expect(lines[1].reachability).toBeNull()
  expect(lines[1].problems).toEqual([])
})

test('problems are about availability, loss and reachability, not distance', () => {
  const telecom = probe(1, 'telecom'), unicom = probe(2, 'unicom'), mobile = probe(3, 'mobile')
  const far = serverLines([server(1, 'TEST_ONLY 法兰克福')], [row(1, telecom, [[260, 0]]), row(1, unicom, [[270, 0]])], [], now)[0]
  expect(far.problems).toEqual([])
  const lossy = serverLines([server(1, 'TEST_ONLY 孟买')], [row(1, telecom, [[180, 0], [181, 25], [182, 0], [183, 0]]), row(1, unicom, [[190, 0], [191, 4], [192, 0]])], [], now)[0]
  expect(lossy.problems).toEqual(['电信丢包 6.3%'])
  const stale = serverLines([server(1, 'TEST_ONLY 台北')], [row(1, mobile, [[50, 0]], now - 600_000)], [target('a', 1, 'partial')], now)[0]
  expect(stale.problems).toEqual(['移动采样已过期', '中国可达性：部分运营商可达'])
  // Waiting for the first sample, a paused task and an unreadable overview are not problems of the line.
  const quiet = serverLines([server(1, 'TEST_ONLY 新加坡')], [{ server_id: 1, probe: telecom, results: [] }, row(1, { ...unicom, enabled: false }, [[30, 50]])], [target('a', 1, 'reachable', 'ip_blocked')], now)[0]
  expect(quiet.problems).toEqual([])
  expect(serverLines([server(1, 'TEST_ONLY 新加坡')], [row(1, telecom, [[30, 50]])], [], now, true)[0].problems).toEqual([])
  // An offline server says so once instead of listing every stale probe.
  expect(serverLines([server(1, 'TEST_ONLY 离线', { online: false })], [row(1, telecom, [[30, 0]], now - 600_000)], [target('a', 1, 'ip_blocked')], now)[0].problems).toEqual(['服务器离线'])
  // Probes to other networks are named by their task.
  const other = probe(4, 'other', { name: 'TEST_ONLY 主站' })
  expect(lineProblems({ ...far, others: [lineSample(other, samples(other, [[5, 10]]), now)] })).toEqual(['TEST_ONLY 主站：丢包 10.0%'])
})

test('rows sort by problems, name, latency or loss, and filter by name, group or region', () => {
  const telecom = probe(1, 'telecom')
  const lines = serverLines([server(1, 'TEST_ONLY B 香港'), server(2, 'TEST_ONLY A 东京', { asset_settings: { region: '🇯🇵', group_name: 'TEST_ONLY AWS' } }), server(3, 'TEST_ONLY C 新加坡'), server(4, 'TEST_ONLY D 未测')],
    [row(1, telecom, [[20, 0]]), row(2, { ...telecom }, [[60, 0]]), row(3, { ...telecom }, [[90, 30], [91, 30]])], [], now)
  expect(sortLines(lines, 'problems').map(line => line.server.id)).toEqual([3, 2, 1, 4])
  expect(sortLines(lines, 'name').map(line => line.server.id)).toEqual([2, 1, 3, 4])
  expect(sortLines(lines, 'latency').map(line => line.server.id)).toEqual([1, 2, 3, 4])
  expect(sortLines(lines, 'loss').map(line => line.server.id)).toEqual([3, 2, 1, 4])
  expect(matchLines(lines, ' aws ').map(line => line.server.id)).toEqual([2])
  expect(matchLines(lines, '🇯🇵').map(line => line.server.id)).toEqual([2])
  expect(matchLines(lines, '新加坡').map(line => line.server.id)).toEqual([3])
  expect(matchLines(lines, '')).toHaveLength(4)
})

test('carrier figures use the median of the latest readings and count lossy servers', () => {
  const telecom = probe(1, 'telecom')
  const lines = serverLines([1, 2, 3, 4].map(n => server(n, `TEST_ONLY ${n}`)), [row(1, telecom, [[10, 0]]), row(2, telecom, [[30, 0]]), row(3, telecom, [[20, 0]]), row(4, telecom, [[50, 20], [51, 0]])], [], now)
  expect(carrierFigures(lines, 'telecom')).toEqual({ configured: 4, measured: 4, median: 25, fastest: 10, slowest: 50, lossy: 1 })
  expect(carrierFigures(lines.slice(0, 3), 'telecom').median).toBe(20)
  expect(carrierFigures(lines, 'mobile')).toEqual({ configured: 0, measured: 0, median: null, fastest: null, slowest: null, lossy: 0 })
  expect(carrierHeading(lines, 'telecom', '电信')).toBe('电信 · 广东')
  const mixed = serverLines([server(1, 'TEST_ONLY')], [row(1, telecom, [[10, 0]]), row(1, { ...probe(2, 'telecom'), monitor: { ...telecom.monitor!, region: '上海' } }, [[12, 0]])], [], now)
  expect(carrierHeading(mixed, 'telecom', '电信')).toBe('电信')
})

test('reachability figures and the worst state of a server', () => {
  const targets = [target('a', 1, 'reachable'), target('b', 1, 'ip_blocked'), target('c', 2, 'partial'), target('d', 2, 'not_applicable'), target('e', 3, 'unknown'), target('f', 3, 'unconfirmed')]
  expect(reachabilityFigures(targets)).toEqual({ total: 5, reachable: 1, blocked: 2, degraded: 1, unknown: 1 })
  expect(worstState(targets.slice(0, 2))).toBe('ip_blocked')
  expect(worstState([targets[2], targets[3]])).toBe('partial')
  expect(worstState([targets[3]])).toBe('not_applicable')
  expect(worstState([])).toBe('not_applicable')
})
