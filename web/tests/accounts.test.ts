import { expect, test } from 'bun:test'
import { RequestIdentity } from '../src/plugins/singbox/accounts/types'
import { parseAmount, decimal } from '../src/plugins/expenses/types'
import { resolveRoute } from '../src/app/routes'

test('expense amounts keep four decimal places', () => {
  expect(parseAmount('899999999999.9999')).toBe('8999999999999999')
  expect(decimal('8999999999999999')).toBe('899999999999.9999')
  expect(() => parseAmount('900000000000.0001')).toThrow()
})
test('retry identity changes only with the intended operation', () => {
  const request = new RequestIdentity()
  const first = request.for({ quantity: 1 })
  expect(request.for({ quantity: 1 })).toBe(first)
  expect(request.for({ quantity: 2 })).not.toBe(first)
})
test('invitation links accept codes and the links of paid invitations', () => {
  expect(resolveRoute('/plugins/sing-box/join')).toEqual({ page: 'member-join' })
  expect(resolveRoute('/plugins/sing-box/join/ABCD-EFGH-JKMN-PQRS')).toEqual({ page: 'member-join', token: 'ABCD-EFGH-JKMN-PQRS' })
  expect(resolveRoute('/plugins/sing-box/join/ABCDEFGHJKMNPQRS')).toEqual({ page: 'member-join', token: 'ABCDEFGHJKMNPQRS' })
  const legacy = 'a'.repeat(43)
  expect(resolveRoute(`/plugins/sing-box/join/${legacy}`)).toEqual({ page: 'member-join', token: legacy })
  expect(resolveRoute('/plugins/sing-box/join/ABCD').page).not.toBe('member-join')
})
