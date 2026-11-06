import { expect, test } from 'bun:test'
import { currencyDigits, minorToUnits, parseMinor } from '../src/plugins/expenses/types'
import { dueState, itemRow, ledgerTotals, monthGroups, monthlyByCategory, paymentYears, selectRows, serverMonthly, serverRow, signedUnits, validItems, validServers } from '../src/plugins/expenses/model'
import type { Payment, ServerRenewal } from '../src/plugins/expenses/model'
import { itemInput } from '../src/plugins/expenses/types'
import type { Expense, Label } from '../src/plugins/expenses/types'
import type { ExchangeRates } from '../src/display/currency'

test('server cost records use each currency’s own minor unit', () => {
  expect(currencyDigits('USD')).toBe(2)
  expect(currencyDigits('JPY')).toBe(0)
  expect(currencyDigits('not-a-code')).toBe(2)
  expect(minorToUnits('1250', 'USD')).toBe('125000')
  expect(minorToUnits('1250', 'JPY')).toBe('12500000')
})

test('cost amounts parse exactly and reject extra precision', () => {
  expect(parseMinor('12.5', 'USD')).toBe(1250)
  expect(parseMinor('0.01', 'CNY')).toBe(1)
  expect(parseMinor('100', 'JPY')).toBe(100)
  for (const [value, code] of [['1.5', 'JPY'], ['1.234', 'USD'], ['-1', 'USD'], ['1e3', 'USD'], ['', 'USD']]) expect(() => parseMinor(value, code)).toThrow()
})

const today = '2026-11-06'
const rates = { base: 'CNY', rates: { USD: 0.14 }, rate_dates: {}, rate_date: today, source: 'frankfurter', source_url: null, fetched_at: 0, attempted_at: 0, next_refresh_at: 0, stale: false, status: 'fresh', error_code: null } satisfies ExchangeRates
const labels: Label[] = [{ id: 1, kind: 'category', name: 'AI 服务', enabled: true }, { id: 2, kind: 'payment_method', name: '信用卡', enabled: true }]
const expense = (id: number, fields: Partial<Expense> = {}): Expense => ({
  id, name: `项目 ${id}`, amount_units: '200000', currency: 'USD', start_date: null, next_payment: '2026-11-20', anchor_payment: '2026-01-20', cycle: 'month', frequency: 1,
  category_id: 1, payment_method_id: 2, payer_id: null, notes: '', url: '', inactive: false, auto_renew: true, notify: false, notify_days_before: 3, cancellation_date: null, replacement_id: null, version: 4, ...fields,
})
const server = (id: number, fields: Partial<ServerRenewal> = {}): ServerRenewal => ({ id, name: `服务器 ${id}`, amount_units: '300000', currency: 'CNY', cycle_days: 30, next_renewal: '2026-11-08', auto_renewal: false, ...fields })
const context = { today, base: 'CNY', rates, labels }

test('items renew and servers expire, each with its own words for the same dates', () => {
  expect(dueState({ next: '2026-11-01', today, inactive: false, auto: false })).toMatchObject({ days: -5, status: { label: '待确认续费', tone: 'danger' }, next: { label: '已过 5 天' }, reasons: ['续费日已过，等待确认'] })
  expect(dueState({ next: '2026-11-01', today, inactive: false, auto: true }).status).toEqual({ label: '等待顺延', tone: 'warning' })
  expect(dueState({ next: today, today, inactive: false, auto: true })).toMatchObject({ status: { label: '今天续费' }, reasons: ['今天续费'] })
  expect(dueState({ kind: 'server', next: '2026-11-08', today, inactive: false, auto: false })).toMatchObject({ status: { label: '即将到期', tone: 'warning' }, reasons: ['2 天后到期'] })
  expect(dueState({ kind: 'server', next: '2026-11-01', today, inactive: false, auto: false })).toMatchObject({ status: { label: '已到期', tone: 'danger' }, reasons: ['到期日已过，请登记续费'] })
  expect(dueState({ kind: 'server', next: '2027-06-30', today, inactive: false, auto: false, recurring: false }).status.label).toBe('按到期日')
  expect(dueState({ next: '2026-11-09', today, inactive: false, auto: true, cancelled: true })).toMatchObject({ status: { label: '到期不续', tone: 'info' }, reasons: [] })
  expect(dueState({ next: '2026-11-01', today, inactive: true, auto: false })).toMatchObject({ status: { label: '已停用' }, next: { label: '—' }, reasons: [] })
})

test('rows carry the price, the monthly cost in the ledger currency and the label names', () => {
  const item = itemRow(expense(1), context)
  expect(item).toMatchObject({ kind: 'item', price: 'USD 20', cycle: '每月', ownMonthly: 20, category: 'AI 服务', payment: '信用卡', payer: '' })
  expect(item.monthly).toBeCloseTo(20 / 0.14, 6)
  expect(itemRow(expense(2, { currency: 'JPY' }), context).monthly).toBeNull()
  const host = serverRow(server(7), context)
  expect(host).toMatchObject({ kind: 'server', price: 'CNY 30', cycle: '每 30 天', inactive: false })
  expect(host.monthly).toBeCloseTo(30 / 30 * 365.25 / 12, 6)
  expect(serverMonthly(server(8, { cycle_days: 0 }))).toBeNull()
  expect(serverRow(server(9, { amount_units: null }), context)).toMatchObject({ price: null, monthly: null })
})

test('the list puts what is due first, stopped entries last, and searches labels and notes', () => {
  const rows = [
    itemRow(expense(1, { name: '乙', next_payment: '2026-12-01' }), context),
    itemRow(expense(2, { name: '甲', next_payment: '2026-11-01', auto_renew: false, notes: '在 Cloudflare 续费' }), context),
    itemRow(expense(3, { name: '丙', inactive: true }), context),
    serverRow(server(7), context),
  ]
  expect(selectRows(rows, {}).map(row => row.key)).toEqual(['item-2', 'server-7', 'item-1'])
  expect(selectRows(rows, { inactive: true }).map(row => row.key).at(-1)).toBe('item-3')
  expect(selectRows(rows, { kind: 'server' }).map(row => row.key)).toEqual(['server-7'])
  expect(selectRows(rows, { query: 'cloudflare' }).map(row => row.key)).toEqual(['item-2'])
  expect(selectRows(rows, { category: '1' }).map(row => row.key)).toEqual(['item-2', 'item-1'])
  const byCost = selectRows(rows, { sort: 'cost' })
  expect(byCost.map(row => row.key)).toEqual(['item-2', 'item-1', 'server-7'])
  const totals = ledgerTotals(rows)
  expect(totals).toMatchObject({ items: 2, servers: 1, inactive: 1, attention: 2, overdue: 1, soon: 1, unconverted: 0 })
  expect(totals.monthly).toBeCloseTo(2 * 20 / 0.14 + 30 * 365.25 / 12 / 30, 6)
  const groups = monthlyByCategory(rows)
  expect(groups.map(group => group.name)).toEqual(['AI 服务', '服务器'])
  expect(groups[0].count).toBe(2)
  expect(Math.round(groups.reduce((sum, group) => sum + group.share, 0))).toBe(100)
})

test('renewals group by month with their total in the ledger currency', () => {
  const groups = monthGroups([
    { kind: 'item', id: 1, name: 'B', date: '2026-11-20', amount_units: '200000', currency: 'USD', auto_renew: true },
    { kind: 'server', id: 7, name: 'A', date: '2026-11-08', amount_units: '300000', currency: 'CNY', auto_renew: false },
    { kind: 'reference', id: 3, name: '机场', date: '2026-12-01', amount_units: null, currency: '', auto_renew: false },
    { kind: 'item', id: 2, name: 'C', date: '2026-12-02', amount_units: '100', currency: 'JPY', auto_renew: true },
  ], 'CNY', rates)
  expect(groups.map(group => [group.label, group.count, group.unconverted])).toEqual([['2026 年 11 月', 2, 0], ['2026 年 12 月', 2, 1]])
  expect(groups[0].entries.map(entry => entry.name)).toEqual(['A', 'B'])
  expect(groups[0].total).toBeCloseTo(30 + 20 / 0.14, 6)
})

test('payments count by year: refunds subtract, price changes and cancellations do not count', () => {
  const payments: Payment[] = [
    { kind: 'item', id: 'a', subscription_id: 1, name: 'A', paid_on: '2026-10-01', period_start: '2026-10-01', amount_units: '80000', currency: 'CNY' },
    { kind: 'server', id: 'b', server_id: 7, name: 'S', record_kind: 'renewal', paid_on: '2026-09-01', valid_until: null, amount_minor: '1250', currency: 'USD', reference: '' },
    { kind: 'server', id: 'c', server_id: 7, name: 'S', record_kind: 'refund', paid_on: '2026-09-15', valid_until: null, amount_minor: '250', currency: 'USD', reference: '' },
    { kind: 'server', id: 'd', server_id: 7, name: 'S', record_kind: 'cancellation', paid_on: '2026-11-01', valid_until: null, amount_minor: '0', currency: 'USD', reference: '' },
    { kind: 'item', id: 'e', subscription_id: 1, name: 'A', paid_on: '2025-10-01', period_start: '2025-10-01', amount_units: '80000', currency: 'CNY' },
  ]
  expect(signedUnits(payments[2])).toBe(-25000n)
  expect(signedUnits(payments[3])).toBe(0n)
  const years = paymentYears(payments, 'CNY', rates)
  expect(years.map(year => [year.year, year.payments.length])).toEqual([['2026', 4], ['2025', 1]])
  expect(years[0].payments[0].id).toBe('d')
  expect(years[0].totals).toEqual([{ currency: 'CNY', units: 80000n }, { currency: 'USD', units: 100000n }])
  expect(years[0].converted).toBeCloseTo(8 + 10 / 0.14, 6)
  expect(paymentYears(payments, 'CNY', undefined)[0].converted).toBeNull()
})

test('answers are validated, and an update sends every field the API takes', () => {
  expect(validItems([expense(1)])).toBe(true)
  expect(validItems([{ ...expense(1), amount_units: 2 }])).toBe(false)
  expect(validItems([{ ...expense(1), next_payment: '11/20' }])).toBe(false)
  expect(validServers({ entries: [server(7)] })).toBe(true)
  expect(validServers({ entries: [{ ...server(7), cycle_days: '30' }] })).toBe(false)
  const body = itemInput(expense(1), { inactive: true })
  expect(body.inactive).toBe(true)
  expect(body.version).toBe(4)
  expect(Object.keys(body).sort()).toEqual(['amount_units', 'auto_renew', 'cancellation_date', 'category_id', 'currency', 'cycle', 'frequency', 'inactive', 'name', 'next_payment', 'notes', 'notify', 'notify_days_before', 'payer_id', 'payment_method_id', 'replacement_id', 'start_date', 'url', 'version'])
})
