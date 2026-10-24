import { expect, test } from 'bun:test'
import { parseMoney, paymentFee, RequestIdentity } from '../src/plugins/singbox/commerce/types'
import { parseAmount, decimal } from '../src/plugins/expenses/types'

test('money parsing preserves minor units and rejects extra precision', () => {
  expect(parseMoney('0.29')).toBe(29)
  expect(parseMoney('-5.01')).toBe(-501)
  expect(() => parseMoney('1.001')).toThrow()
  expect(() => parseMoney('1e3')).toThrow()
  expect(paymentFee(1234, { fixed_fee_cents: 10, fee_basis_points: 125 })).toBe(25)
  expect(parseAmount('899999999999.9999')).toBe('8999999999999999')
  expect(decimal('8999999999999999')).toBe('899999999999.9999')
  expect(() => parseAmount('900000000000.0001')).toThrow()
})
test('retry identity changes only with the intended operation', () => {
  const request = new RequestIdentity()
  const first = request.for({ user: 41, amount: 29 })
  expect(request.for({ user: 41, amount: 29 })).toBe(first)
  expect(request.for({ user: 41, amount: 30 })).not.toBe(first)
})
