import { expect, test } from 'bun:test'
import { carrierSummary, detailLines, intervalError, pendingNote, riskLabel, runErrorText, runLine, sortTargets, stateLabels, stateTones } from '../src/observability/monitoring/reachability'
import type { Probe, ReachabilityState, ReachabilityTarget } from '../src/observability/monitoring/reachability'

const probe = (carrier: string, connected: boolean): Probe => ({ carrier, network: 'fixture', city: 'fixture', connected, avg_ms: connected ? 30 : null })
const target = (key: string, state: ReachabilityState, pending: ReachabilityState | null = null): ReachabilityTarget => ({
  key, host: 'node.example.com', port: 443, tcp: true, state, blocked: false, pending_state: pending, failing_streak: pending ? 1 : 0,
  observed_state: state, resolved_ip: null, detail: {}, checked_at: null, state_changed_at: null, deferred_at: null, nodes: [], risks: [],
})

test('every state has a label and a tone, and blocked states read as problems', () => {
  for (const state of Object.keys(stateLabels) as ReachabilityState[]) expect(stateTones[state]).toBeDefined()
  for (const state of ['ip_blocked', 'port_blocked', 'unconfirmed'] as const) expect(stateTones[state]).toBe('bad')
  expect(stateTones.reachable).toBe('good')
  expect(stateTones.unknown).toBe('neutral')
})

test('carrier summary counts connections per carrier in a fixed order', () => {
  expect(carrierSummary([probe('mobile', false), probe('telecom', true), probe('telecom', false), probe('unicom', true)])).toBe('电信 1/2 · 联通 1/1 · 移动 0/1')
  expect(carrierSummary([probe('someone', true)])).toBe('其他 1/1')
  expect(carrierSummary([])).toBe('')
})

test('detail lines separate mainland, overseas and the control comparison', () => {
  expect(detailLines({
    china: { probes: [probe('telecom', false)], error: null },
    overseas: { probes: [probe('other', true), probe('other', false)], error: null },
    control: { status: 'checked', port: 22, probes: [probe('telecom', false)] },
  })).toEqual(['中国：电信 0/1', '海外：1/2', '对照端口 22：电信 0/1'])
  expect(detailLines({
    china: { probes: [], error: 'timeout' }, overseas: { probes: [], error: 'http_500' }, control: { status: 'not_needed' },
  })).toEqual(['中国：测量失败', '海外：测量失败'])
  expect(detailLines({ control: { status: 'no_port' } })[0]).toContain('无法对照')
  expect(detailLines({ control: { status: 'deferred' } })[0]).toContain('延后')
  expect(detailLines({ reason: 'udp_only' })[0]).toContain('UDP')
  expect(detailLines({ reason: 'no_public_address' })[0]).toContain('公网地址')
  expect(detailLines({})).toEqual([])
})

test('risk flags read as warnings and carry their value', () => {
  expect(riskLabel({ code: 'port_not_443', value: '8443' })).toContain('8443')
  expect(riskLabel({ code: 'name_risky_tld', value: 'example.ru' })).toContain('example.ru')
  expect(riskLabel({ code: 'name_risky_brand', value: 'www.apple.com' })).toContain('www.apple.com')
  expect(riskLabel({ code: 'future_flag', value: 'kept' })).toBe('kept')
})

test('problems sort first, then unconfirmed failures, then by endpoint', () => {
  const sorted = sortTargets([target('a', 'reachable'), target('b', 'reachable', 'ip_blocked'), target('c', 'ip_blocked'), target('e', 'partial'), target('d', 'not_applicable')])
  expect(sorted.map(row => row.key)).toEqual(['c', 'e', 'b', 'a', 'd'])
})

test('a pending block says it still needs its second run', () => {
  expect(pendingNote(target('a', 'reachable'))).toBe('')
  expect(pendingNote(target('a', 'reachable', 'port_blocked'))).toContain('需连续 2 次')
})

test('the interval follows the limits the server reports', () => {
  const limits = { minimum_interval_minutes: 15, maximum_interval_minutes: 1440 }
  expect(intervalError('30', limits)).toBe('')
  expect(intervalError('15', limits)).toBe('')
  expect(intervalError('14', limits)).not.toBe('')
  expect(intervalError('1441', limits)).not.toBe('')
  expect(intervalError('15.5', limits)).not.toBe('')
  expect(intervalError('', limits)).not.toBe('')
})

test('the last run summary reports deferrals and never claims a run that did not happen', () => {
  expect(runLine({})).toBeUndefined()
  const line = runLine({ started_at: 1, measured: 3, deferred: 2, unresolved: 1, tests_used: 18, remaining: 120, limit: 250 })
  expect(line).toContain('检测 3 组')
  expect(line).toContain('因额度延后 2 组')
  expect(line).toContain('1 个地址未解析')
  expect(line).toContain('120/250')
  expect(runErrorText('limits_http_500')).toContain('没有发送检测')
  expect(runErrorText('token_unavailable')).toContain('没有发送检测')
  expect(runErrorText('other')).toContain('other')
})
