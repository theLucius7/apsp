import { expect, test } from 'bun:test'
import { accountError, bandwidthError, checked, credentialError, operationError, powerError, registrationError, resourceError } from '../src/plugins/alicloud/guards'
import { billUsable, defaultPowerPolicy, exactUsage } from '../src/plugins/alicloud/types'
import type { Account, Operation, Overview, PowerJob, Resource } from '../src/plugins/alicloud/types'
import { editorError, ruleError } from '../src/plugins/ddns/guards'
import type { DdnsConfig, DdnsRule } from '../src/plugins/ddns/types'

const now = Date.parse('2026-10-02T00:00:00Z') / 1000
const account = { id: 'TEST_ONLY account', enabled: true, revision: 2, error_code: null, bill: { month: '2026-10', queried_at: now, usage_micro_gb: 0, rows: [] } } as Account
const resource = { id: 'TEST_ONLY resource', account_id: account.id, revision: 3, kind: 'ecs', cloud_id: 'i-testonly', region: 'cn-hangzhou', power_policy: { ...defaultPowerPolicy } } as Resource
const base = (): Overview => ({ accounts: [structuredClone(account)], resources: [structuredClone(resource)], operations: [], power_jobs: [] })
const operation = { id: 'TEST_ONLY operation', resource_id: resource.id, account_revision: 2, resource_revision: 3, status: 'preview', source: 'manual', expires_at: now + 300, before_state: { cloud_id: resource.cloud_id, region: resource.region }, target: { bandwidth_mbps: 1, charge_type: 'PayByTraffic' } } as Operation

test('cloud drafts never write from pending, vanished or changed account/resource revisions', () => {
  let data: Overview | undefined = base(), writes = 0
  const current = () => data
  const attempt = () => checked(() => resourceError(current, resource, { managed: true, accountRevision: 2 }), async () => ++writes)
  for (const altered of [undefined, { ...base(), resources: [] }, { ...base(), accounts: [] }]) {
    data = altered; expect(attempt).toThrow(); expect(writes).toBe(0)
  }
  for (const revision of [undefined, 0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, 4]) {
    data = base(); data.resources[0].revision = revision as number
    expect(attempt).toThrow(); expect(writes).toBe(0)
  }
  data = base(); data.accounts[0].revision++
  expect(attempt).toThrow(); expect(accountError(current, account)).not.toBe('')
  expect(registrationError(current, account.id, account.revision)).not.toBe('')
  data = base(); data.accounts[0].enabled = false; expect(attempt).toThrow()
  data = base(); expect(resourceError(current, resource, { accountRevision: undefined })).not.toBe('')
  expect(writes).toBe(0)
})

test('bandwidth and power confirmations require the original current intent, revisions and expiry', () => {
  const job = { ...operation, action: 'stop', stop_mode: 'KeepCharging' } as unknown as PowerJob
  for (const power of [false, true]) {
    const draft = power ? job : operation, data = base()
    if (power) data.power_jobs = [structuredClone(job)]; else data.operations = [structuredClone(operation)]
    const guard = () => operationError(() => data, draft, power, ['preview'], true, now)
    expect(guard()).toBe('')
    const latest = power ? data.power_jobs![0] : data.operations[0]
    latest.expires_at = now; expect(guard()).not.toBe(''); latest.expires_at = now + 300
    latest.account_revision = undefined; expect(guard()).not.toBe(''); latest.account_revision = 2
    latest.status = 'running'; expect(guard()).not.toBe(''); latest.status = 'preview'
    data.accounts[0].enabled = false; expect(guard()).not.toBe(''); data.accounts[0].enabled = true
    data.operations.push({ ...operation, id: 'TEST_ONLY other', status: 'uncertain' }); expect(guard()).not.toBe(''); data.operations.pop()
    if (power) data.power_jobs = []; else data.operations = []
    expect(guard()).not.toBe('')
  }
})

test('unknown capabilities and current traffic protection block power starts', () => {
  const data = base(); delete data.power_jobs
  expect(powerError(() => data, resource, 'start', 2)).not.toBe('')
  data.power_jobs = []; data.resources[0].power_policy = { ...defaultPowerPolicy, enabled: true, threshold_action: 'stop' }
  data.resources[0].threshold_hold = true
  expect(powerError(() => data, resource, 'start', 2)).not.toBe('')
  expect(powerError(() => data, resource, 'stop', 2)).toBe('')
})

test('a newly attached EIP invalidates bandwidth drafts and confirmations without blocking ECS power', () => {
  const data = base(), current = () => data
  data.operations = [structuredClone(operation)]
  expect(bandwidthError(current, resource)).toBe('')
  expect(operationError(current, operation, false, ['preview'], true, now)).toBe('')
  data.resources[0].snapshot = { ...operation.before_state, attached_eip_id: 'eip-testonly', bandwidth_mbps: 2000, charge_type: 'PayByDominantTraffic' }
  let writes = 0
  expect(() => checked(() => bandwidthError(current, resource), async () => ++writes)).toThrow('绑定 EIP')
  expect(operationError(current, operation, false, ['preview'], true, now)).toContain('绑定 EIP')
  expect(powerError(current, resource, 'start', 2)).toBe('')
  expect(writes).toBe(0)
})

test('billing display preserves exact zero and never rounds unsafe, negative or unknown authority', () => {
  expect(exactUsage(0)).toBe(0); expect(billUsable(account, now)).toBeTrue()
  for (const value of [null, undefined, -1, 1.5, Number.MAX_SAFE_INTEGER + 1, NaN, Infinity]) {
    expect(exactUsage(value)).toBeNull()
    expect(billUsable({ ...account, bill: { ...account.bill!, usage_micro_gb: value as number } }, now)).toBeFalse()
  }
  expect(credentialError('TEST_ONLY_KEY', '')).not.toBe('')
  expect(credentialError(' ', ' ', true)).not.toBe('')
  expect(credentialError('', '')).toBe('')
})

test('legacy Cloudflare drafts stay readable, but stale revisions and vanished server choices cannot write', () => {
  const config: DdnsConfig = { name: 'TEST_ONLY legacy', server_id: 1, zone_id: '0'.repeat(32), record_name: 'node.example.com', record_type: 'A', ttl: 1, proxied: false, enabled: true, interval_secs: 300, adopt_existing: false }
  const rule = { id: 'TEST_ONLY rule', config, revision: 2, busy: false, plugin_enabled: true } as DdnsRule
  const data = { rules: [rule], servers: [{ id: 1, name: 'TEST_ONLY server', online: false, enabled: true }] }
  expect(editorError(() => data, config, rule)).toBe('')
  expect(editorError(() => data, { ...config, provider: 'cloudflare' }, rule)).toBe('')
  expect(editorError(() => data, { ...config, provider: 'aliyun' }, rule)).not.toBe('')
  expect(ruleError(() => ({ ...data, rules: [{ ...rule, revision: 3 }] }), rule)).not.toBe('')
  expect(editorError(() => ({ ...data, servers: [] }), config, rule)).not.toBe('')
  expect(ruleError(() => ({ ...data, rules: [{ ...rule, revision: undefined as unknown as number }] }), rule)).not.toBe('')
})

test('dual-stack creation reserves two slots from the current snapshot', () => {
  const config: DdnsConfig = { name: 'TEST_ONLY dual', server_id: 1, zone_id: '0'.repeat(32), record_name: 'dual.example.com', record_type: 'A', ttl: 1, proxied: false, enabled: true, interval_secs: 300, adopt_existing: false }
  const data = { rules: Array.from({ length: 31 }, (_, index) => ({ id: `TEST_ONLY ${index}`, config, revision: 1, busy: false, plugin_enabled: true } as DdnsRule)), servers: [{ id: 1, name: 'TEST_ONLY server', online: false, enabled: true }] }
  expect(editorError(() => data, config)).toBe('')
  expect(editorError(() => data, config, undefined, 2)).toContain('两条')
  expect(editorError(() => undefined, config, undefined, 2)).not.toBe('')
  data.rules.pop()
  expect(editorError(() => data, config, undefined, 2)).toBe('')
})
