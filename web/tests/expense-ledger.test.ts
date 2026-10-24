import { expect, test } from 'bun:test'
import { currencyDigits, minorToUnits, parseMinor } from '../src/plugins/expenses/types'

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
