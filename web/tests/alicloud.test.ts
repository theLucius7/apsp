import { expect, test } from 'bun:test'
import { accountWrite, billUsable, charge, defaultPowerPolicy, message, status, updated } from '../src/resources/cloud/types'
import { bytes } from '../src/format'
import type { Account, Resource } from '../src/resources/cloud/types'
import { POWER_RESULT, ROTATION_RESULTS, activityItems, deliveryText } from '../src/costs/cloud/history'
import type { EipRotation } from '../src/resources/cloud/types'
import { cdtGuardProgress } from '../src/operations/cloud/guardUsage'
import { trafficPoints, readingStale } from '../src/costs/cloud/usageHistory'

test('attached EIP billing is displayed without relabelling dominant traffic as ordinary traffic', () => {
  expect(charge('PayByDominantTraffic')).toBe('按主流量计费')
  expect(charge('PayByTraffic')).toBe('按流量计费')
  expect(charge('PayByBandwidth')).toBe('按带宽计费')
  expect(charge('PayBy95')).toBe('按增强 95 计费')
  expect(charge('TEST_ONLY_UNKNOWN')).toBe('其他计费方式')
})

test('traffic history preserves missing buckets, pool separation, resets and exact large-counter differences', () => {
  const from = Date.parse('2026-10-31T15:00:00Z') / 1000
  const sample = (hour: number, bytes: string) => ({ bucket_start: from + hour * 3600, sampled_at: from + hour * 3600, mainland_bytes: bytes, overseas_bytes: '0' })
  const group = { seconds: 3600, from, to: from + 5 * 3600, samples: [sample(0, '100'), sample(1, '10'), sample(2, '20'), sample(4, '9007199254740992'), sample(5, '9007199254740993')] }
  const delta = trafficPoints(group, 'mainland_bytes', true)
  expect(delta.map(point => point.value)).toEqual([null, null, 10 / 1024 ** 3, null, null, 1 / 1024 ** 3])
  expect(trafficPoints(group, 'overseas_bytes', true)[2].value).toBe(0)
  expect(trafficPoints(group, 'mainland_bytes', false).some(point => point.at === (from + 3600) * 1000 - 1 && point.value === null)).toBeTrue()
  const backward = { ...group, samples: [sample(0, '100'), sample(1, '10'), sample(2, '5')] }
  expect(trafficPoints(backward, 'mainland_bytes', true)[2].value).toBeNull()
})

test('fresh instance timestamps cannot make stale traffic current', () => {
  const now = Date.parse('2026-11-01T00:10:00+08:00') / 1000
  expect(readingStale(now - 30, now, 900, null, true)).toBeFalse()
  expect(readingStale(now - 901, now, 900, null, true)).toBeTrue()
  expect(readingStale(now - 700, now, 900, null, true)).toBeTrue()
  expect(readingStale(now - 1, now, 900, 'network_error')).toBeTrue()
  expect(readingStale(now + 1, now, 900)).toBeTrue()
  expect(readingStale(null, now, 900)).toBeTrue()
})

test('cloud edits do not copy cached billing or write-only credentials back into requests', () => {
  const input = { name: ' 云账号 ', site: 'china' as const, enabled: true, auto_enabled: false, limit_gb: 100, bill: { sensitive: 'cached' } }
  expect(accountWrite(input, ' ', '', 2)).toEqual({ name: '云账号', site: 'china', enabled: true, auto_enabled: false, limit_gb: 100, revision: 2 })
  expect(accountWrite(input, ' TEST_ONLY_ID ', ' TEST_ONLY_SECRET ').access_key_secret).toBeUndefined()
  expect(accountWrite(input, ' TEST_ONLY_ID ', ' TEST_ONLY_SECRET ', undefined, '', true)).toMatchObject({legacy_credentials: true, access_key_id: 'TEST_ONLY_ID', access_key_secret: 'TEST_ONLY_SECRET'})
  expect(accountWrite(input, ' TEST_ONLY_ID ', ' TEST_ONLY_SECRET ', 3, 'TEST_ONLY_CREDENTIAL_REF', true)).toEqual({ name: '云账号', site: 'china', enabled: true, auto_enabled: false, limit_gb: 100, revision: 3, credential_id: 'TEST_ONLY_CREDENTIAL_REF' })
  expect(message('TEST_ONLY_SECRET')).toBe('云接口暂时不可用')
  for (const code of ['constructor', '__proto__', 'toString']) { expect(status(code)).toBe('状态未知'); expect(message(code)).toBe('云接口暂时不可用') }
})
test('billing status rejects previous month, stale, future and incomplete evidence', () => {
  const now = Date.parse('2026-10-01T00:00:00Z') / 1000
  const account = { enabled: true, error_code: null, bill: { month: '2026-10', queried_at: now, usage_micro_gb: 100000000, rows: [] } } as unknown as Account
  expect(billUsable(account, now)).toBeTrue()
  for (const changed of [{ month: '2026-09' }, { queried_at: now - 901 }, { queried_at: now + 1 }, { usage_micro_gb: null }]) expect(billUsable({ ...account, bill: { ...account.bill!, ...changed } }, now)).toBeFalse()
  expect(billUsable({ ...account, error_code: 'network_error' }, now)).toBeFalse()
  expect(billUsable({ ...account, enabled: false }, now)).toBeFalse()
})


test('BSS opt-ins are independent and disabled billing cannot authorize control from cached values', () => {
  const input = { name: '云账号', site: 'china' as const, enabled: true, auto_enabled: false, limit_gb: 100, billing_enabled: false, balance_enabled: true }
  expect(accountWrite(input, '', '')).toMatchObject({ billing_enabled: false, balance_enabled: true })
  const now = Date.parse('2026-10-01T00:00:00Z') / 1000
  const account = { ...input, bill: { month: '2026-10', queried_at: now, usage_micro_gb: 100, rows: [] } } as unknown as Account
  expect(billUsable(account, now)).toBeFalse()
})

test('one history folds each power result notice into its job and keeps other notices', () => {
  const state = { cloud_id: 'i-testonly', region: 'cn-hongkong', status: 'Running', stopped_mode: null, charge_type: 'PostPaid', network_type: 'vpc', spot_strategy: 'NoSpot', interruption_behavior: null, public_ips: [], locked: false }
  const job = (id: string, created_at: number, status: string) => ({ id, resource_id: 'r1', action: 'start', stop_mode: 'KeepCharging', source: 'cdt_guard', before_state: state, status, created_at, expires_at: created_at + 300, error_code: null, request_id: null })
  const event = (id: number, title: string, created_at: number, resource_id = 'r1') => ({ id, resource_id, title, message: 'TEST_ONLY', created_at, deliveries: [] })
  const snapshot = { bandwidth_mbps: 10, charge_type: 'PayByTraffic', kind: 'ecs', cloud_id: 'i-testonly', region: 'cn-hongkong', public_ip: '192.0.2.1', resource_charge_type: 'PostPaid', status: 'Running' }
  const operation = { id: 'o1', resource_id: 'r1', before_state: snapshot, target: { bandwidth_mbps: 1, charge_type: 'PayByTraffic' as const }, source: 'manual', billing_cycle: null, status: 'uncertain', created_at: 50, updated_at: 50, expires_at: 350, error_code: null, request_id: null }
  const items = activityItems({
    operations: [operation],
    power_jobs: [job('old', 100, 'succeeded'), job('new', 200, 'succeeded')],
    events: [event(1, POWER_RESULT, 105), event(2, POWER_RESULT, 150), event(3, POWER_RESULT, 205), event(4, POWER_RESULT, 230), event(5, 'CDT 流量阈值提醒', 220), event(6, POWER_RESULT, 90)],
  })
  // The unfinished bandwidth change leads; then newest first. Notice 6 predates every job, so it stays visible.
  expect(items.map(item => item.key)).toEqual(['operation:o1', 'event:5', 'power:new', 'power:old', 'event:6'])
  const folded = (key: string) => { const item = items.find(entry => entry.key === key); return item?.kind === 'power' ? item.event?.id : undefined }
  expect(folded('power:old')).toBe(2)
  expect(folded('power:new')).toBe(4)
  expect(deliveryText(event(7, POWER_RESULT, 1))).toBe('')
  expect(deliveryText({ ...event(8, POWER_RESULT, 1), deliveries: [{ channel: 'telegram', status: 'sent', attempts: 1, last_error: null }, { channel: 'webhook', status: 'failed', attempts: 3, last_error: 'TEST_ONLY 503' }] }))
    .toBe('通知：Telegram 已送达；Webhook 投递失败（尝试 3 次） · TEST_ONLY 503')
})

test('a public IP rotation is one entry: its result notice folds into it, and unfinished ones lead', () => {
  const before = { allocation_id: 'eip-testonly', public_ip: '192.0.2.1', package_id: null, isp: 'BGP', internet_charge_type: 'PayByTraffic', bandwidth: 200, instance_id: 'i-testonly', region: 'cn-hongkong', server_ids: [1] }
  const rotation = (id: string, created_at: number, status: string, resource_id = 'r1') => ({ id, resource_id, requested_by: 1, probe_port: 443, max_attempts: 3, snapshot_digest: 'TEST_ONLY', before, status, phase: 'allocate', attempt: 1, attempts: [], new_ip: null, error_code: null, created_at, expires_at: created_at + 300, updated_at: created_at }) as EipRotation
  const event = (id: number, title: string, created_at: number, resource_id = 'r1') => ({ id, resource_id, title, message: 'TEST_ONLY', created_at, deliveries: [] })
  const [succeeded, restored, halted] = ROTATION_RESULTS
  const items = activityItems({
    operations: [],
    power_jobs: [],
    eip_rotations: [rotation('first', 100, 'succeeded'), rotation('second', 300, 'restored'), rotation('halted', 400, 'unknown', 'r2'), rotation('waiting', 500, 'preview')],
    events: [event(1, succeeded, 160), event(2, restored, 320), event(3, halted, 410, 'r2'), event(4, POWER_RESULT, 600), event(5, '公网 IP 已更换', 50)],
  })
  // The halted one and the preview still need the administrator; a notice older than any rotation stays visible.
  expect(items.map(item => item.key)).toEqual(['rotation:waiting', 'rotation:halted', 'event:4', 'rotation:second', 'rotation:first', 'event:5'])
  const folded = (key: string) => { const item = items.find(entry => entry.key === key); return item?.kind === 'rotation' ? item.event?.id : undefined }
  expect(folded('rotation:first')).toBe(1)
  expect(folded('rotation:second')).toBe(2)
  expect(folded('rotation:halted')).toBe(3)
  expect(folded('rotation:waiting')).toBeUndefined()
})

test('the guard summary measures the month high plus a counted jump against the stop line', () => {
  const GIB = 1024 ** 3, now = 1_000_000
  const resource = { power_policy: { ...defaultPowerPolicy, enabled: true, cdt_guard: { threshold_bytes: 150 * GIB, jump_bytes: 20 * GIB, auto_start: true } },
    cdt_guard_state: { billing_cycle: '2026-11', region: 'cn-hongkong', cloud_id: 'i-testonly', account_revision: 1, max_usage_bytes: 100 * GIB, max_jump_bytes: 25 * GIB, queried_at: now - 30 } } as unknown as Resource
  expect(cdtGuardProgress(resource, now)).toMatchObject({ backlog: 25 * GIB, used: 125 * GIB, left: 25 * GIB, stale: false })
  expect(cdtGuardProgress({ ...resource, cdt_guard_state: { ...resource.cdt_guard_state!, max_jump_bytes: 3 * GIB, queried_at: now - 300 } }, now)).toMatchObject({ backlog: 0, left: 50 * GIB, stale: true })
  expect(cdtGuardProgress({ ...resource, cdt_guard_state: { ...resource.cdt_guard_state!, max_usage_bytes: 200 * GIB } }, now)?.left).toBe(0)
  expect(cdtGuardProgress({ ...resource, cdt_guard_state: null }, now)).toBeNull()
})

test('each reading shows its clock today and its date otherwise', () => {
  const now = new Date(2026, 10, 6, 12, 0, 0)
  expect(updated(Math.floor(new Date(2026, 10, 6, 1, 42, 16).getTime() / 1000), now)).toBe('01:42:16')
  expect(updated(Math.floor(new Date(2026, 10, 5, 1, 42, 16).getTime() / 1000), now)).toContain('2026')
  expect(updated(null, now)).toBe('尚未查询')
  // CDT readings are binary gigabytes and read like every other size.
  expect(bytes(Math.round(16.363 * 1024 ** 3))).toBe('16.3 GiB')
})
