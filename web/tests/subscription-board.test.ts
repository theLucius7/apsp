import { expect, test } from 'bun:test'
import { boardCounts, renewalView, savedSort, selectEntries, validRenewalSummary, validRenewals } from '../src/costs/subscriptions/board'
import type { Entry } from '../src/costs/subscriptions/board'
import type { Expense, Label } from '../src/costs/expenses/types'
import type { ExchangeRates } from '../src/costs/currency/currency'

const today = '2026-11-06'
const rates = { base: 'CNY', rates: { USD: 0.14 }, rate_dates: {}, rate_date: '2026-11-05', source: 'frankfurter', source_url: null, fetched_at: 0, attempted_at: 0, next_refresh_at: 0, stale: false, status: 'fresh', error_code: null } satisfies ExchangeRates
const labels: Label[] = [{ id: 1, kind: 'category', name: 'AI 服务', enabled: true }, { id: 2, kind: 'payment_method', name: '信用卡', enabled: true }, { id: 3, kind: 'category', name: '域名', enabled: true }]
const subscription = (id: number, fields: Partial<Expense> = {}): Expense => ({
  id, name: `订阅 ${id}`, amount_units: '200000', currency: 'USD', start_date: null, next_payment: '2026-11-20', anchor_payment: '2026-01-20', cycle: 'month', frequency: 1,
  category_id: 1, payment_method_id: 2, payer_id: null, notes: '', url: '', inactive: false, auto_renew: true, notify: true, notify_days_before: 3, cancellation_date: null, replacement_id: null, version: 1, ...fields,
})
const view = (fields: Partial<Expense> = {}) => renewalView(subscription(5, fields), today, 'CNY', rates, labels)
const entries = (items: Expense[]): Entry[] => items.map(item => ({ key: `item-${item.id}`, item, view: renewalView(item, today, 'CNY', rates, labels) }))

test('answers are validated before anything is drawn', () => {
  expect(validRenewals([subscription(1)])).toBe(true)
  expect(validRenewals([{ ...subscription(1), cycle: 'decade' }])).toBe(false)
  expect(validRenewals([{ ...subscription(1), amount_units: 2 }])).toBe(false)
  expect(validRenewalSummary({ from: today, base_currency: 'CNY', budget_units: null, exchange: rates })).toBe(true)
  expect(validRenewalSummary({ from: '11/06', base_currency: 'CNY', budget_units: null, exchange: rates })).toBe(false)
})

test('a subscription shows its price, monthly cost in the ledger currency and the next renewal', () => {
  const result = view()
  expect(result.status).toEqual({ label: '自动续费', tone: 'good' })
  expect([result.price, result.cycle, result.days, result.next.label, result.category, result.payment, result.renewal]).toEqual(['USD 20', '每月', 14, '11/20', 'AI 服务', '信用卡', '自动'])
  expect(result.monthly).toBeCloseTo(20 / 0.14, 6)
  expect(result.nextAmount).toBeCloseTo(20 / 0.14, 6)
  expect(result.progress).toBeGreaterThan(50)
  expect(result.reasons).toEqual([])
  // Same currency needs no rate; a missing one is reported, not guessed.
  expect(renewalView(subscription(6, { currency: 'CNY' }), today, 'CNY', undefined, labels).monthly).toBe(20)
  expect(renewalView(subscription(7, { currency: 'JPY' }), today, 'CNY', rates, labels).monthly).toBeNull()
  expect(view({ cycle: 'year', frequency: 2 }).cycle).toBe('每 2 年')
})

test('renewals due soon or overdue need attention; stopped and cancelled ones do not', () => {
  expect(view({ next_payment: '2026-11-09' })).toMatchObject({ status: { label: '即将续费', tone: 'warning' }, reasons: ['3 天后续费'] })
  expect(view({ next_payment: today }).status.label).toBe('今天续费')
  expect(view({ next_payment: '2026-11-01', auto_renew: false })).toMatchObject({ status: { label: '待确认续费', tone: 'danger' }, next: { label: '已过 5 天' }, reasons: ['续费日已过，等待确认'] })
  const stopped = view({ inactive: true, next_payment: '2026-11-08' })
  expect([stopped.usable, stopped.reasons, stopped.progress]).toEqual([false, [], null])
  expect(view({ next_payment: '2026-11-08', cancellation_date: '2026-11-08' })).toMatchObject({ status: { label: '到期不续' }, reasons: [] })
})

test('counts, filters and sorting put the soonest renewal first and stopped ones last', () => {
  const list = entries([
    subscription(1, { name: 'ChatGPT Plus' }),
    subscription(2, { name: 'example.com', amount_units: '900000', currency: 'CNY', cycle: 'year', next_payment: '2027-03-02', category_id: 3, auto_renew: false, notes: '在 Cloudflare 续费' }),
    subscription(3, { name: '云盘', amount_units: '80000', currency: 'CNY', next_payment: '2026-11-01', auto_renew: false, category_id: null }),
    subscription(4, { name: '旧服务', inactive: true }),
    subscription(5, { name: 'Claude', next_payment: '2026-11-09' }),
  ])
  const counts = boardCounts(list)
  expect(counts).toMatchObject({ all: 5, active: 4, stopped: 1, attention: 2, overdue: 1, soon: 1, unconverted: 0, month: 2 })
  expect(counts.monthly).toBeCloseTo(2 * 20 / 0.14 + 7.5 + 8, 6)
  expect(counts.monthTotal).toBeCloseTo(2 * 20 / 0.14, 6)
  expect(selectEntries(list, {}).map(entry => entry.item.id)).toEqual([3, 5, 1, 2, 4])
  expect(selectEntries(list, { filter: 'attention' }).map(entry => entry.item.id)).toEqual([3, 5])
  expect(selectEntries(list, { filter: 'stopped' }).map(entry => entry.item.id)).toEqual([4])
  expect(selectEntries(list, { filter: 'active' }).map(entry => entry.item.id)).toEqual([3, 5, 1, 2])
  expect(selectEntries(list, { query: 'cloudflare' }).map(entry => entry.item.id)).toEqual([2])
  expect(selectEntries(list, { category: '3' }).map(entry => entry.item.id)).toEqual([2])
  expect(selectEntries(list, { sort: 'cost' }).map(entry => entry.item.id).slice(0, 2)).toEqual([1, 5])
  expect(savedSort('cost')).toBe('cost')
  expect(savedSort('attention')).toBe('next')
})
