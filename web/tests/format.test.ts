import { expect, test } from 'bun:test'
import { ago } from '../src/format'

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
