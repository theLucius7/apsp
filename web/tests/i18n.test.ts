import { expect, test } from 'bun:test'
import { translate } from '../src/i18n'

test('translates shared interface labels to English', () => {
  expect(translate('服务器看板', 'en-US')).toBe('Server dashboard')
  expect(translate('Agent 版本', 'en-US')).toBe('Agent versions')
  expect(translate('未配置服务', 'en-US')).toBe('Service not configured')
  expect(translate('刷新', 'en-US')).toBe('Refresh')
  expect(translate('刷新', 'zh-CN')).toBe('刷新')
})

test('keeps runtime values that are not in the static catalog', () => {
  const runtimeValue = '服务器自定义名称-01'
  expect(translate(runtimeValue, 'en-US')).toBe(runtimeValue)
})
