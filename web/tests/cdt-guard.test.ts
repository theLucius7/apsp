import { expect, test } from 'bun:test'
import { cdtGuardExceeded } from '../src/plugins/alicloud/guardUsage'
import type { Account, Resource } from '../src/plugins/alicloud/types'

const now = Date.parse('2026-10-01T00:00:00Z') / 1000, gib = 1024 ** 3
const account = { enabled: true, revision: 2, error_code: 'billing_incomplete' } as Account
const resource = { region: 'cn-hongkong', cloud_id: 'i-test', power_policy: {
  enabled: true, cdt_guard: { threshold_bytes: 150 * gib, jump_bytes: 20 * gib, auto_start: true },
}, cdt_guard_state: { billing_cycle: '2026-10', region: 'cn-hongkong', cloud_id: 'i-test', account_revision: 2,
  queried_at: now, max_usage_bytes: 110 * gib, max_jump_bytes: 45 * gib } } as Resource

test('CDT uses a separate pool meter with remembered backlog, independent of BSS', () => {
  expect(cdtGuardExceeded(account, resource, now)).toBeTrue()
  expect(cdtGuardExceeded(account, { ...resource, cdt_guard_state: { ...resource.cdt_guard_state!, max_jump_bytes: 10 * gib } }, now)).toBeFalse()
})
test('imported, stale, future, wrong-cycle and failed readings cannot authorize a start', () => {
  for (const change of [{ queried_at: 0 }, { queried_at: now - 121 }, { queried_at: now + 1 },
    { billing_cycle: '2026-09' }, { account_revision: 1 }, { cloud_id: 'i-other' }, { max_usage_bytes: NaN }]) {
    expect(cdtGuardExceeded(account, { ...resource, cdt_guard_state: { ...resource.cdt_guard_state!, ...change } }, now)).toBeNull()
  }
  expect(cdtGuardExceeded(account, { ...resource, cdt_guard_error: 'cdt_reset_pending' }, now)).toBeNull()
  expect(cdtGuardExceeded({ ...account, enabled: false }, resource, now)).toBeNull()
})
