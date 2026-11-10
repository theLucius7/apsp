import { expect, test } from 'bun:test'
import { portalAccessError, validPortalAccess } from '../src/access/portalAccess'

test('portal zero-key readiness and immutable invitation identity fail closed', () => {
  const original = { configuration: { enabled: true, reason: null, origin: 'https://panel.example.com' }, keys: 0, url: null, activation_expires_at: null }
  expect(validPortalAccess(original)).toBeTrue()
  expect(portalAccessError(original, original)).toBe('')
  for (const value of [undefined, {}, { ...original, keys: -1 }, { ...original, keys: Number.MAX_SAFE_INTEGER + 1 }]) expect(portalAccessError(value, original)).not.toBe('')
  for (const value of [{ ...original, keys: 1 }, { ...original, url: 'https://panel.example.com/changed' }, { ...original, activation_expires_at: 123 }, { ...original, configuration: { ...original.configuration, origin: 'https://changed.example.com' } }, { ...original, configuration: { ...original.configuration, enabled: false } }]) expect(portalAccessError(value, original)).not.toBe('')
  expect(portalAccessError(structuredClone(original), original)).toBe('')
})
