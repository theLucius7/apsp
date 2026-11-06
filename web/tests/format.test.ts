import { expect, test } from 'bun:test'
import { ago, bytes, totalBytes } from '../src/format'

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
