import { expect, test } from 'bun:test'
import { amount, ratio, utcDay } from '../src/observability/statistics/data'

test('large byte totals remain integers until chart geometry is calculated', () => {
  const maximum = 18446744073709551617n
  expect(amount(maximum.toString())).toBe(maximum)
  expect(ratio(maximum.toString(), maximum)).toBe(100)
  expect(ratio((maximum / 2n).toString(), maximum)).toBe(49.99)
  expect(ratio((maximum * 2n).toString(), maximum)).toBe(100)
})

test('unknown observations are distinct from measured zero and dates use UTC', () => {
  for (const value of [undefined, null, '', '-1', 'NaN', '1.2']) expect(amount(value)).toBeNull()
  expect(amount('0')).toBe(0n)
  expect(ratio(null, 10n)).toBe(0)
  expect(ratio('0', 0n)).toBe(0)
  expect(utcDay(Date.UTC(2026, 9, 1) / 1000)).toBe('10/01')
})
