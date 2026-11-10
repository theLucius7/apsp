import { expect, test } from 'bun:test'
import { convert, costSummary, money, price, quoteDate, quoteState, remainingCost } from '../src/costs/currency/currency'
import type { ExchangeRates } from '../src/costs/currency/currency'
import type { Server } from '../src/types'
import { defaultAssets } from '../src/resources/server-assets'

const quote: ExchangeRates = { base: 'CNY', rates: { CNY: 1, USD: .125, EUR: .1 }, rate_dates: { USD: '2026-09-30', EUR: '2026-09-29' }, rate_date: '2026-09-29', source: 'frankfurter', source_url: 'https://frankfurter.dev', fetched_at: 1, attempted_at: 1, next_refresh_at: 2, stale: true, status: 'stale', error_code: 'fetch_failed' }

test('conversion uses CNY quotes, keeps real stale data and never fabricates missing prices', () => {
  expect(convert(10, 'USD', 'CNY', quote)).toBe(80)
  expect(convert(10, 'USD', 'EUR', quote)).toBe(8)
  expect(convert(10, 'USD', 'USD', undefined)).toBe(10)
  expect(convert(10, 'USD', 'CNY', undefined)).toBeNull()
  expect(convert(10, 'GBP', 'CNY', quote)).toBeNull()
  expect(convert(10, 'USD', 'EUR', { ...quote, rates: { USD: 0, EUR: .1 } })).toBeNull()
  for (const value of [null, undefined, '', '-1', 'NaN', '1e3']) expect(price(value)).toBeNull()
  expect(price('0')).toBe(0)
  expect(money(null, 'CNY')).toBe('—')
  expect(quoteDate(quote, 'USD', 'EUR')).toBe('2026-09-29')
  expect(quoteDate(quote, 'GBP', 'CNY')).toBeNull()
  expect(convert(0, 'GBP', 'CNY', undefined)).toBe(0)
  expect(convert(null, 'GBP', 'CNY', undefined)).toBeNull()
})

test('a failed cache read qualifies retained quotes, and unknown status cannot become fresh', () => {
  const fresh: ExchangeRates = { ...quote, status: 'fresh', stale: false, error_code: null }
  expect(quoteState(fresh)).toBe('fresh')
  expect(quoteState(fresh, 'cache read failed')).toBe('read-error')
  expect(convert(10, 'USD', 'CNY', fresh)).toBe(80)
  expect(quoteState(fresh)).toBe('fresh')
  expect(quoteState(quote)).toBe('stale')
  expect(quoteState({ ...fresh, error_code: 'fetch_failed' })).toBe('stale')
  expect(quoteState({ ...fresh, stale: true })).toBe('stale')
  expect(quoteState({ ...fresh, status: 'unavailable' })).toBe('unavailable')
  expect(quoteState(undefined)).toBe('unavailable')
  expect(quoteState(undefined, 'cache read failed')).toBe('read-error')
  expect(quoteState({ ...fresh, status: undefined } as unknown as ExchangeRates)).toBe('unknown')
  expect(quoteState({ ...fresh, stale: undefined } as unknown as ExchangeRates)).toBe('unknown')
})

test('cost totals exclude public and hidden resources, identify partial coverage and separate billing periods', () => {
  const now = Date.UTC(2026, 9, 1)
  const item = (price: string | null, currency = 'CNY', billing_cycle = 30) => ({ id: 1, public_view: false, asset_settings: { ...defaultAssets, price, currency, billing_cycle, expires_at: (now + 15 * 86_400_000) / 1000 } }) as Server
  const rows = [item('10', 'USD'), item('20', 'CNY', 0), item(null), item('30', 'GBP'), { ...item('1000'), public_view: true }, { ...item('1000'), asset_settings: { ...item('1000').asset_settings!, hidden: true } }]
  expect(costSummary(rows, 'CNY', quote, now)).toEqual({ total: 100, recurring: 80, remaining: 60, priced: 3, converted: 2, recurringCount: 1, remainingCount: 2, missingRates: 1, missingPrices: 1 })
  expect(costSummary([item('3', 'GBP')], 'CNY', quote, now).total).toBeNull()
  expect(remainingCost(item('10').asset_settings, now)).toBe(5)
  expect(remainingCost(item('10', 'CNY', 0).asset_settings, now)).toBe(10)
  expect(remainingCost({ ...item('10').asset_settings!, expires_at: null }, now)).toBeNull()
  expect(remainingCost(item('10').asset_settings, now + 60 * 86_400_000)).toBe(0)
})

test('remaining value includes prepaid years and prorated partial billing periods', () => {
  const now = Date.UTC(2026, 9, 2)
  const asset = (price: string, billing_cycle: number, days: number) => ({ ...defaultAssets, price, billing_cycle, expires_at: now / 1000 + days * 86_400 })
  expect(remainingCost(asset('36', 30, 3254), now)).toBeCloseTo(3904.8, 8)
  expect(remainingCost(asset('36', 30, 3650), now)).toBeCloseTo(4380, 8)
  expect(remainingCost(asset('36', 30, 15), now)).toBe(18)
  expect(remainingCost(asset('36', 30, 45), now)).toBe(54)
  expect(remainingCost(asset('120', 365, 730), now)).toBe(240)
  expect(remainingCost(asset('0', 30, 3650), now)).toBe(0)
  expect(remainingCost(asset('36', 0, 3650), now)).toBe(36)
  for (const cycle of [0, 30]) for (const days of [0, -1]) expect(remainingCost(asset('36', cycle, days), now)).toBe(0)
  expect(remainingCost({ ...asset('36', 30, 30), expires_at: null }, now)).toBeNull()
  for (const cycle of [NaN, Infinity, -1]) expect(remainingCost(asset('36', cycle, 30), now)).toBeNull()
  const servers = [{ id: 1, asset_settings: asset('36', 30, 3254) }, { id: 2, asset_settings: asset('12', 0, 3650) }] as Server[]
  expect(costSummary(servers, 'CNY', undefined, now)).toMatchObject({ remaining: 3916.8, recurring: 36, remainingCount: 2, recurringCount: 1 })
})
