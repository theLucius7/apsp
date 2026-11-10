import { expect, test } from 'bun:test'
import { probeRevisionMatches, probeWriteError, familyLabel, authorizationMatches, bindProbeAuthorization, changeProbe, lossLabel, probeSlots, probeState, probeTone, probeValue } from '../src/observability/probes'
import type { Probe, ProbeResult } from '../src/observability/probes'

const probe: Probe = { id: 'probe', name: '回环', kind: 'icmp', target: '127.0.0.1', port: null, interval_secs: 10, carrier: '', enabled: true, monitor: { region: '', address_family: 'any', authorization: { kind: 'owned', source: 'TEST_ONLY owner', scope: 'TEST_ONLY owned loopback measurement', enabled: true, expires_at: null, identity: { kind: 'icmp', target: '127.0.0.1', port: null, address_family: 'any' } } } }
const point: ProbeResult = { id: 'point', probe_id: probe.id, sampled_at: 100_000, latency_ms: 0, loss_percent: 0, error: null }

test('probes distinguish zero, total loss and unavailable legacy measurements', () => {
  expect(probeValue(point, 'latency_ms')).toBe(0)
  expect(probeValue({ ...point, latency_ms: null, loss_percent: 100 }, 'loss_percent')).toBe(100)
  expect(probeValue({ ...point, loss_percent: 100, error: 'permission denied' }, 'loss_percent')).toBeNull()
  expect(probeValue({ ...point, error: '' }, 'latency_ms')).toBeNull()
  for (const invalid of [-1, NaN, Infinity, 101]) expect(probeValue({ ...point, loss_percent: invalid }, 'loss_percent')).toBeNull()
  expect(lossLabel(probe)).toBe('丢包率')
  expect(lossLabel({ kind: 'tcp' })).toBe('连接失败率')
  expect(probeTone(100, 'loss_percent')).toBe('danger')
  expect(probeTone(0, 'latency_ms')).toBe('good')
  expect(probeTone(null, 'latency_ms')).toBe('empty')
})

test('paused, offline, stale and failed reads never expose live quality', () => {
  expect(probeState(probe, point, 101_000)).toBe('最近采样')
  expect(probeState(probe, point, 161_000)).toBe('采样已过期')
  expect(probeState(probe, point, 101_000, true)).toBe('状态未知')
  expect(probeState({ ...probe, enabled: false }, point, 101_000)).toBe('已暂停')
  expect(probeState(probe, { ...point, error: 'failed' }, 101_000)).toBe('检测不可用')
  expect(probeState(probe, undefined, 101_000)).toBe('等待采样')
})

test('quality bars keep time gaps and unavailable samples instead of carrying forward success', () => {
  const samples = [point, { ...point, sampled_at: 105_000, loss_percent: 100, latency_ms: null }, { ...point, sampled_at: 80_000, error: 'failed' }, { ...point, sampled_at: 140_000 }, { ...point, probe_id: 'other', sampled_at: 120_000 }]
  const slots = probeSlots(samples, probe, 125_000, 5)
  expect(slots.map(value => value?.sampled_at)).toEqual([80_000, undefined, 105_000, undefined, undefined])
  expect(probeValue(slots[0], 'loss_percent')).toBeNull()
  expect(probeValue(slots[2], 'loss_percent')).toBe(100)
})

test('revoked and expired authorization cannot display current successful telemetry', () => {
  const authorized: Probe = { ...probe, target: '::1', execution_authorized: true, monitor: { region: '华东', address_family: 'ipv6', authorization: {
    kind: 'owned', source: 'TEST_ONLY-owned-target', scope: 'ICMP on this loopback only', enabled: true, expires_at: 102,
    identity: { kind: 'icmp', target: '::1', port: null, address_family: 'ipv6' },
  } } }
  expect(probeState(authorized, point, 103_000)).toBe('授权已过期')
  expect(probeState({ ...probe, execution_authorized: false }, point, 101_000)).toBe('未取得执行授权')
  const revoked = { ...authorized, monitor: { ...authorized.monitor!, authorization: { ...authorized.monitor!.authorization!, enabled: false } } }
  expect(probeState(revoked, point, 101_000)).toBe('授权已撤销')
  const payload = bindProbeAuthorization({ ...authorized, target: '::1' })
  expect(payload.execution_authorized).toBeUndefined()
  expect(payload.monitor!.authorization!.identity).toEqual({ kind: 'icmp', target: '::1', port: null, address_family: 'ipv6' })
  expect(bindProbeAuthorization({ ...probe, monitor: null }).monitor).toBeNull()
})

test('editing destination identity requires a fresh confirmation and submitting never transfers the old grant', () => {
  const authorized: Probe = { ...probe, kind: 'tcp', target: 'probe.example.com', port: 443, monitor: { region: '', address_family: 'ipv4', authorization: {
    kind: 'consent', source: 'TEST_ONLY explicit consent', scope: 'Only this target, method, port and family', enabled: true, expires_at: null,
    identity: { kind: 'tcp', target: 'probe.example.com', port: 443, address_family: 'ipv4' },
  } } }
  for (const part of [{ target: 'other.example.com' }, { kind: 'icmp' as const, port: null }, { port: 8443 }, { monitor: { ...authorized.monitor!, address_family: 'ipv6' as const } }]) {
    const changed = changeProbe(authorized, part)
    expect(changed.monitor!.authorization!.enabled).toBe(false)
    expect(changed.monitor!.authorization!.identity).toEqual(authorized.monitor!.authorization!.identity)
    expect(authorizationMatches(changed)).toBe(false)
    const submitted = bindProbeAuthorization({ ...authorized, ...part })
    expect(submitted.monitor!.authorization!.enabled).toBe(false)
    expect(submitted.monitor!.authorization!.identity).toEqual(authorized.monitor!.authorization!.identity)
    expect(changeProbe(changed, authorized).monitor!.authorization!.enabled).toBe(false)
  }
  expect(changeProbe(authorized, { name: 'renamed', interval_secs: 60 }).monitor!.authorization!.enabled).toBe(true)
})


test('unconfirmed targets are inert and actual four failed connections keep measured loss', () => {
  expect(probeState({ ...probe, monitor: null }, point, 101_000)).toBe('未取得执行授权')
  const failed: ProbeResult = { ...point, latency_ms: null, loss_percent: 100, error: 'TEST_ONLY connection refused', attempts: 4, address_family: 'ipv6' }
  expect(probeValue(failed, 'latency_ms')).toBeNull()
  expect(probeValue(failed, 'loss_percent')).toBe(100)
  expect(probeState(probe, failed, 101_000)).toBe('最近采样')
  expect(familyLabel(probe, failed)).toBe('IPv6')
  expect(probeValue({ ...failed, attempts: undefined }, 'loss_percent')).toBeNull()
})


test('probe edits and deletion require the same positive safe persisted revision', () => {
  const current: Probe = { ...probe, revision: 4 }
  expect(probeWriteError(current, { ...current })).toBe('')
  for (const revision of [undefined, null, 0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1])
    expect(probeWriteError({ ...current, revision }, current)).toContain('草稿已保留')
  for (const revision of [undefined, null, 0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1])
    expect(probeRevisionMatches({ revision }, { revision })).toBe(false)
  expect(probeWriteError(current, undefined)).toContain('草稿已保留')
  expect(probeWriteError(current, { ...current, revision: 5 })).toContain('草稿已保留')
})

test('admin revision survives binding while public presentation cannot confer a grant', () => {
  const current: Probe = { ...probe, revision: 7, execution_authorized: true, monitor: null }
  const payload = bindProbeAuthorization(current)
  expect(payload.revision).toBe(7)
  expect(payload.enabled).toBe(false)
  expect(payload.execution_authorized).toBeUndefined()
  expect(payload.monitor).toBeNull()
})
