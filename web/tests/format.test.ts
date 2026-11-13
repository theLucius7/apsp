import { expect, test } from 'bun:test'
import { ago, bytes, clock, currencyDigits, day, money, numberText, shortTime, time, totalBytes } from '../src/format'

test('relative contact time uses coarse, readable units', () => {
  const now = 1_800_000_000
  expect(ago(null, now)).toBe('时间未知')
  expect(ago(now - 30, now)).toBe('刚刚')
  expect(ago(now - 12 * 60, now)).toBe('12 分钟前')
  expect(ago(now - 5 * 3600, now)).toBe('5 小时前')
  expect(ago(now - 3 * 86400, now)).toBe('3 天前')
  // Clock skew never produces a negative age.
  expect(ago(now + 120, now)).toBe('刚刚')
})

test('a malformed traffic counter shows as missing instead of breaking the page', () => {
  expect(totalBytes('1024', '1024')).toBe('2048')
  expect(totalBytes('not-a-number', '1')).toBeUndefined()
  expect(totalBytes('1.5', '1')).toBeUndefined()
  expect(bytes(totalBytes({} as unknown as string, '1'))).toBe('暂无数据')
})

test('every size is scaled the same way and says how it is missing', () => {
  expect(bytes('0', '—')).toBe('0 B')
  expect(bytes(null, '—')).toBe('—')
  expect(bytes(undefined)).toBe('暂无数据')
  // One decimal, never rounded up: a reading never overstates what was used.
  expect(bytes(String(2 * 1024 ** 3 - 1))).toBe('1.9 GiB')
  expect(bytes(1.5 * 1024 ** 2)).toBe('1.5 MiB')
  // A counter that is not a whole number of bytes, or below zero, is missing.
  expect(bytes('1.5', '—')).toBe('—')
  expect(bytes('-5', '—')).toBe('—')
})

test('money carries its currency code, grouping and the currency decimals', () => {
  // The code and the amount are joined by a no-break space so they never wrap apart.
  expect(money(1234.5, 'USD')).toBe('USD\u00a01,234.50')
  expect(money(1234, 'JPY')).toBe('JPY\u00a01,234')
  expect(money(null, 'CNY')).toBe('—')
  // Recorded amounts keep up to four decimals; computed ones round to two.
  expect(money(0.0012, 'USD', 4)).toBe('USD\u00a00.0012')
  expect(money(0.0012, 'USD')).toBe('USD\u00a00.00')
  expect(money(-12.5, 'CNY')).toBe('-CNY\u00a012.50')
  expect(currencyDigits('USD')).toBe(2)
  expect(numberText(1234.5678)).toBe('1,234.568')
})

test('moments read in one order on a 24-hour clock', () => {
  const at = new Date(2026, 10, 14, 8, 30, 5).getTime() / 1000
  expect(time(at)).toBe('2026/11/14 08:30:05')
  expect(clock(at)).toBe('08:30:05')
  expect(day(at)).toBe('2026/11/14')
  expect(shortTime(at)).toBe('11/14 08:30')
  expect(time(null)).toBe('尚未上报')
  expect(time(null, '暂无记录')).toBe('暂无记录')
})
