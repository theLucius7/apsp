import { expect, test } from 'bun:test'
import { boardCounts, orderedSourceCard, renewalView, savedSort, selectEntries, sourceView, userStatus, userView, validRenewalSummary, validRenewals, validUserBoard } from '../src/subscriptions/board'
import type { Entry, UserSubscription } from '../src/subscriptions/board'
import type { SubscriptionSource } from '../src/plugins/singbox/orderedSourceTypes'
import type { Expense, Label } from '../src/plugins/expenses/types'
import { daysUntil, monthlyAmount } from '../src/plugins/expenses/types'
import type { ExchangeRates } from '../src/display/currency'

const DAY = 86_400
const GIB = 1024 ** 3
// 2026-11-06 04:00 UTC, the same day as the ledger's `today` below.
const now = Date.UTC(2026, 10, 6, 4) / 1000
const user: UserSubscription = {
  id: 1, name: '测试用户', status: 'active', allowed: true, package_name: '基础', monthly_bytes: String(100 * GIB), used_bytes: String(10 * GIB),
  starts_at: now - 40 * DAY, expires_at: now + 60 * DAY, cycle_start: now - 15 * DAY, next_reset: now + 15 * DAY,
  groups: ['原权限组 · 00'], bridged: false, last_fetch_at: now - 3 * 3600, last_fetch_format: 'clash', last_fetch_result: 'generated', fetches_today: 2,
}
const view = (item: Partial<UserSubscription>, recorded = true) => userView({ ...user, ...item }, now, recorded, 7)
const renewal: Expense = {
  id: 5, name: 'ChatGPT Plus', amount_units: '200000', currency: 'USD', start_date: null, next_payment: '2026-11-20', anchor_payment: '2026-01-20', cycle: 'month', frequency: 1,
  category_id: 1, payment_method_id: 2, payer_id: null, notes: '', url: '', inactive: false, auto_renew: true, notify: true, notify_days_before: 3, cancellation_date: null, replacement_id: null, version: 1,
}
const labels: Label[] = [{ id: 1, kind: 'category', name: 'AI 服务', enabled: true }, { id: 2, kind: 'payment_method', name: '信用卡', enabled: true }]
const rates = { base: 'CNY', rates: { USD: 0.14, EUR: 0.13 }, rate_dates: {}, rate_date: '2026-11-05', source: 'frankfurter', source_url: null, fetched_at: now, attempted_at: now, next_refresh_at: now, stale: false, status: 'fresh', error_code: null } satisfies ExchangeRates
const today = '2026-11-06'

test('board answers are validated before anything is drawn', () => {
  expect(validUserBoard({ now, fetches_recorded: true, fetch_retention_days: 7, subscriptions: [user] })).toBe(true)
  expect(validUserBoard({ now, fetches_recorded: true, fetch_retention_days: 7, subscriptions: [{ ...user, used_bytes: 12 }] })).toBe(false)
  expect(validUserBoard({ now, fetches_recorded: true, fetch_retention_days: 7, subscriptions: [{ ...user, groups: [1] }] })).toBe(false)
  expect(validUserBoard({ now, subscriptions: [] })).toBe(false)
  expect(validRenewals([renewal])).toBe(true)
  expect(validRenewals([{ ...renewal, cycle: 'decade' }])).toBe(false)
  expect(validRenewals([{ ...renewal, amount_units: 2 }])).toBe(false)
  expect(validRenewalSummary({ from: today, base_currency: 'CNY', budget_units: null, exchange: rates })).toBe(true)
  expect(validRenewalSummary({ from: '11/06', base_currency: 'CNY', budget_units: null, exchange: rates })).toBe(false)
})

test('a proxy user subscription reads usage, cycle progress, expiry and the last fetch', () => {
  const result = view({})
  expect(result.status).toEqual({ label: '使用中', tone: 'good' })
  expect(result.usage).toBe(10)
  expect(result.elapsed).toBe(50)
  expect(result.runsOutAt).toBeNull()
  expect(result.fetch.label).toBe('3 小时前')
  expect(result.fetch.format).toBe('Clash')
  expect(result.reasons).toEqual([])
  expect(result.usable).toBe(true)
})

test('a subscription that runs out before its reset or expires soon needs attention', () => {
  // 60 GiB in half a cycle runs out at 100 GiB before the reset.
  const fast = view({ used_bytes: String(60 * GIB) })
  expect(fast.runsOutAt!).toBeLessThan(user.next_reset!)
  expect(fast.reasons[0]).toStartWith('按当前速度')
  expect(view({ used_bytes: String(95 * GIB) }).reasons).toEqual(['流量即将用完'])
  const soon = view({ expires_at: now + 3 * DAY })
  expect(soon.expiry).toMatchObject({ label: '3 天后', tone: 'warning', days: 3 })
  expect(soon.reasons).toContain('3 天后到期')
  // Early in a cycle a busy start says little about the month.
  expect(view({ cycle_start: now - DAY, next_reset: now + 29 * DAY, used_bytes: String(20 * GIB) }).runsOutAt).toBeNull()
})

test('stopped subscriptions are labelled but never flagged, and missing records stay honest', () => {
  const expired = view({ status: 'expired', allowed: false, expires_at: now - DAY })
  expect(expired.status).toEqual({ label: '已到期', tone: 'danger' })
  expect(expired.expiry).toMatchObject({ label: '已到期', tone: 'danger' })
  expect(expired.reasons).toEqual([])
  expect(userStatus({ status: 'no_package', allowed: false })).toEqual({ label: '未分配套餐', tone: 'muted' })
  expect(userStatus({ status: 'something-new', allowed: true }).label).toBe('可用')
  const unmetered = view({ status: 'unmetered', monthly_bytes: null, cycle_start: null, next_reset: null, expires_at: null })
  expect([unmetered.usage, unmetered.elapsed, unmetered.reset.label, unmetered.expiry.label]).toEqual([null, null, '—', '长期有效'])
  // The bridge reports 0 for a cycle start it does not know.
  expect(view({ cycle_start: 0 }).elapsed).toBeNull()
  expect(view({ last_fetch_at: null, last_fetch_format: null, last_fetch_result: null }).fetch.label).toBe('无记录')
  expect(view({}, false).fetch.label).toBe('未记录')
  expect(view({ last_fetch_format: 'renderer', last_fetch_result: 'relayed' }).fetch.format).toBe('原系统生成')
  expect(view({ last_fetch_result: 'unavailable' }).reasons).toContain('最近一次拉取失败')
})

test('ledger helpers count days in the ledger calendar and spread cycles over months', () => {
  expect(daysUntil('2026-11-20', today)).toBe(14)
  expect(daysUntil('2026-11-01', today)).toBe(-5)
  expect(daysUntil('not a date', today)).toBeNull()
  expect(monthlyAmount(renewal)).toBe(20)
  expect(monthlyAmount({ amount_units: '1200000', cycle: 'year', frequency: 1 })).toBe(10)
  expect(monthlyAmount({ amount_units: '300000', cycle: 'month', frequency: 3 })).toBe(10)
  expect(monthlyAmount({ amount_units: '70000', cycle: 'week', frequency: 1 })).toBeCloseTo(30.44, 1)
})

test('a renewal item shows its price, monthly cost in the ledger currency and the next renewal', () => {
  const result = renewalView(renewal, today, 'CNY', rates, labels)
  expect(result.status).toEqual({ label: '自动续费', tone: 'good' })
  expect(result.price).toBe('USD 20')
  expect(result.cycle).toBe('每月')
  expect(result.monthly).toBeCloseTo(20 / 0.14, 6)
  expect(result.days).toBe(14)
  expect(result.next.label).toBe('11/20')
  expect(result.progress).toBeGreaterThan(50)
  expect(result.category).toBe('AI 服务')
  expect(result.payment).toBe('信用卡')
  expect(result.reasons).toEqual([])
  // Same currency needs no rate; a missing one is reported, not guessed.
  expect(renewalView({ ...renewal, currency: 'CNY' }, today, 'CNY', undefined, labels).monthly).toBe(20)
  expect(renewalView({ ...renewal, currency: 'JPY' }, today, 'CNY', rates, labels).monthly).toBeNull()
  expect(renewalView({ ...renewal, cycle: 'year', frequency: 2 }, today, 'CNY', rates, labels).cycle).toBe('每 2 年')
})

test('renewals due soon or overdue need attention; inactive and cancelled ones do not', () => {
  const soon = renewalView({ ...renewal, next_payment: '2026-11-09' }, today, 'CNY', rates, labels)
  expect(soon.status).toEqual({ label: '即将续费', tone: 'warning' })
  expect(soon.reasons).toEqual(['3 天后续费'])
  expect(renewalView({ ...renewal, next_payment: today }, today, 'CNY', rates, labels).status.label).toBe('今天续费')
  const overdue = renewalView({ ...renewal, next_payment: '2026-11-01', auto_renew: false }, today, 'CNY', rates, labels)
  expect(overdue.status).toEqual({ label: '待确认续费', tone: 'danger' })
  expect(overdue.next.label).toBe('已过 5 天')
  expect(overdue.reasons).toEqual(['续费日已过，等待确认'])
  const inactive = renewalView({ ...renewal, inactive: true, next_payment: '2026-11-08' }, today, 'CNY', rates, labels)
  expect([inactive.usable, inactive.reasons, inactive.progress, inactive.due]).toEqual([false, [], null, null])
  const ending = renewalView({ ...renewal, next_payment: '2026-11-08', cancellation_date: '2026-11-08' }, today, 'CNY', rates, labels)
  expect(ending.status.label).toBe('到期不续')
  expect(ending.reasons).toEqual([])
})

const source = {
  id: 9, name: '机场', kind: 'url', host: 'sub.example.com', configured: true, auth_configured: false, settings_revision: 1, identity_epoch: 1, archived: false,
  refresh_interval_secs: 3600, user_agent: null, auto_refresh: true, traffic: { upload: GIB, download: 3 * GIB, total: 100 * GIB, expire: now + 5 * DAY, updated_at: now },
  changes: { added: 0, updated: 0, missing: 0, unsupported: 0 }, last_attempt_at: now, last_success_at: now - 600, latest_success: null, active_job: null,
  last_error: null, stale_reason: null, counts: { supported: 12, unsupported: 0, ambiguous: 0, missing: 0 }, dependencies: [],
} satisfies SubscriptionSource

test('subscription sources show provider usage, expiry and failures', () => {
  const card = orderedSourceCard(source)
  expect(card).toMatchObject({ label: '正常', tone: 'good', used: 4 * GIB, total: 100 * GIB, supported: 12, host: 'sub.example.com' })
  const result = sourceView(card, now)
  expect(result.expiry).toEqual({ label: '5 天后到期', tone: 'warning' })
  expect(result.reasons).toEqual(['5 天后到期'])
  const failed = orderedSourceCard({ ...source, last_error: { stage: 'fetch', kind: 'http', message: '来源未返回成功响应', http_status: 403 } })
  expect(failed).toMatchObject({ label: '导入失败', tone: 'danger', error: '来源未返回成功响应' })
  expect(sourceView(failed, now).usable).toBe(false)
  expect(orderedSourceCard({ ...source, traffic: {} })).toMatchObject({ used: null, total: null, expire: null })
})

test('one board counts, filters and sorts every kind together', () => {
  const users: UserSubscription[] = [
    { ...user, id: 1, name: '乙' },
    { ...user, id: 2, name: '甲', used_bytes: String(95 * GIB), fetches_today: 0 },
    { ...user, id: 3, name: '丙', status: 'expired', allowed: false, expires_at: now - DAY, last_fetch_at: null, fetches_today: 0 },
    { ...user, id: 4, name: '丁', expires_at: now + 2 * DAY, groups: ['家人'] },
  ]
  const entries: Entry[] = [
    ...users.map(item => ({ kind: 'users' as const, key: `user-${item.id}`, name: item.name, search: [item.name, ...item.groups], item, view: userView(item, now, true, 7) })),
    ...[renewal, { ...renewal, id: 6, name: '域名', next_payment: '2026-11-08', currency: 'CNY', amount_units: '900000', cycle: 'year' as const }, { ...renewal, id: 7, name: '旧服务', inactive: true }]
      .map(item => ({ kind: 'renewals' as const, key: `renewal-${item.id}`, name: item.name, search: [item.name], item, view: renewalView(item, today, 'CNY', rates, labels) })),
    { kind: 'sources' as const, key: 'ordered-9', name: source.name, search: [source.name], item: orderedSourceCard(source), view: sourceView(orderedSourceCard(source), now) },
  ]
  const counts = boardCounts(entries)
  expect(counts).toMatchObject({ all: 8, usable: 6, attention: 4, stopped: 2 })
  expect(counts.users).toMatchObject({ all: 4, usable: 3, stopped: 1, expiring: 1, runningOut: 1, fetched: 2, fetches: 4, metered: 3 })
  expect(counts.users.used).toBe(BigInt(125) * BigInt(GIB))
  expect(counts.renewals).toMatchObject({ all: 3, active: 2, attention: 1, unconverted: 0, month: 2 })
  expect(counts.renewals.monthly).toBeCloseTo(20 / 0.14 + 7.5, 6)
  expect(counts.sources).toEqual({ all: 1, attention: 1 })
  expect(selectEntries(entries, 'users', '', 'all', 'attention').map(entry => entry.key)).toEqual(['user-4', 'user-2', 'user-1', 'user-3'])
  expect(selectEntries(entries, 'all', '', 'attention', 'attention').map(entry => entry.key)).toEqual(['renewal-6', 'user-4', 'ordered-9', 'user-2'])
  expect(selectEntries(entries, 'renewals', '', 'stopped', 'attention').map(entry => entry.key)).toEqual(['renewal-7'])
  expect(selectEntries(entries, 'all', '家人', 'all', 'attention').map(entry => entry.key)).toEqual(['user-4'])
  expect(selectEntries(entries, 'renewals', '', 'all', 'usage').map(entry => entry.key)).toEqual(['renewal-5', 'renewal-7', 'renewal-6'])
  expect(selectEntries(entries, 'users', '', 'all', 'fetch').map(entry => entry.key).at(-1)).toBe('user-3')
  expect(savedSort('usage')).toBe('usage')
  expect(savedSort('nonsense')).toBe('attention')
})
