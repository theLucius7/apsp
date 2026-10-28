import { expect, test } from 'bun:test'
import { resolveRoute } from '../src/app/routes'
import { api } from '../src/api'

test('only explicit plugin account routes bypass administrator page routing', () => {
  const account = '00000000-0000-4000-8000-000000000002'
  const token = 'T'.repeat(43)
  expect(resolveRoute(`/plugins/sing-box/account/${account}`)).toEqual({ page: 'proxy-portal', account, activation: undefined })
  expect(resolveRoute(`/plugins/sing-box/account/${account}?activate=${token}`)).toEqual({ page: 'proxy-portal', account, activation: token })
  for (const path of ['/users', '/account', '/plugins/sing-box/account/1', `/plugins/sing-box/account/${account}/administrator`, `/plugins/sing-box/account/${account}?activate=<script>`, `/plugins/sing-box/account/${account}?activate=${token}&admin=true`]) {
    expect(resolveRoute(path).page).toBe('not-found')
  }
  expect(resolveRoute('/plugins/sing-box/users').page).toBe('proxy-users')
})

test('a proxy authentication error never ends an administrator session', async () => {
  const originalFetch = globalThis.fetch
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'window')
  const events: string[] = []
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { dispatchEvent: (event: Event) => events.push(event.type) } })
  globalThis.fetch = (async () => new Response(JSON.stringify({ error: '请登录' }), { status: 401 })) as typeof fetch
  try {
    await expect(api('/api/plugins/sing-box/portal/test', 'GET', undefined, undefined, false)).rejects.toThrow('请登录')
    await expect(api('/api/login/passkey/finish', 'POST', {})).rejects.toThrow('请登录')
    expect(events).toEqual([])
    await expect(api('/api/security/passkeys')).rejects.toThrow('请登录')
    expect(events).toEqual(['apsp:unauthorized'])
  } finally {
    globalThis.fetch = originalFetch
    if (descriptor) Object.defineProperty(globalThis, 'window', descriptor)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})

test('both reauthentication messages resume a protected request at most once', async () => {
  const originalFetch = globalThis.fetch
  const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'window')
  let prompts = 0
  Object.defineProperty(globalThis, 'window', { configurable: true, value: { dispatchEvent: (event: CustomEvent) => {
    expect(event.type).toBe('apsp:reauthentication')
    prompts++; event.detail.resolve()
  } } })
  try {
    for (const message of ['请先再次验证管理员身份，证明有效期为五分钟', '请先再次验证管理员密码及二步验证码，证明有效期为五分钟']) {
      let attempts = 0
      globalThis.fetch = (async () => {
        attempts++
        return new Response(JSON.stringify({ error: message }), { status: 403 })
      }) as typeof fetch
      await expect(api('/api/control-center/tokens', 'POST', {})).rejects.toThrow(message)
      expect(attempts).toBe(2)
    }
    expect(prompts).toBe(2)
  } finally {
    globalThis.fetch = originalFetch
    if (descriptor) Object.defineProperty(globalThis, 'window', descriptor)
    else Reflect.deleteProperty(globalThis, 'window')
  }
})
