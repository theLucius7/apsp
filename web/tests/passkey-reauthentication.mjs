import assert from 'node:assert/strict'
import { createHash } from 'node:crypto'

// Uses only the isolated database and virtual authenticator owned by passkeys.mjs.
export async function reauthentication({ admin, sql, clearLimit, native, request, ok, password, origin, results }) {
  const { page, context } = admin
  const base = '/api/control-center/reauth/passkey'
  const session = async () => (await context.cookies()).find(cookie => cookie.name === 'apsp_session').value
  const sessionHash = async () => createHash('sha256').update(await session()).digest('hex')
  const expireProof = () => sql`UPDATE administrator_reauth SET expires_at=0`
  const noProof = async () => {
    const hash = await sessionHash()
    const [row] = await sql`SELECT count(*)::int AS count FROM administrator_reauth WHERE session_hash=${hash} AND expires_at>extract(epoch from now())`
    assert.equal(row.count, 0)
  }
  const tokenCount = async name => Number((await sql`SELECT count(*) AS count FROM management_api_tokens WHERE name=${name}`)[0].count)
  await clearLimit()
  await expireProof()
  const originalSession = await session()
  const hash = await sessionHash()
  const [sessionBefore] = await sql`SELECT expires_at FROM sessions WHERE token_hash=${hash}`
  // Passkey is an independent factor even when password verification would need TOTP.
  await sql`UPDATE admins SET totp_secret=decode('0102030405060708090001020304050607080900','hex') WHERE id=1`
  await page.goto(`${origin}/#/system/control-center`)
  await page.getByRole('button', { name: 'API 令牌', exact: true }).click()
  await page.getByLabel('名称', { exact: true }).fill('passkey-reauth-ui')
  const issued = page.waitForResponse(response => response.url().endsWith('/api/control-center/tokens') && response.request().method() === 'POST' && response.status() === 200)
  await page.getByRole('button', { name: '创建令牌', exact: true }).click()
  const dialog = page.getByRole('dialog', { name: '再次验证身份' })
  await dialog.getByRole('button', { name: '使用 Passkey 验证', exact: true }).click()
  const token = (await (await issued).json()).token
  await dialog.waitFor({ state: 'hidden' })
  await page.getByRole('button', { name: '我已保存，隐藏令牌', exact: true }).click()
  assert.equal(await tokenCount('passkey-reauth-ui'), 1)
  assert.equal(await session(), originalSession)
  const [proof] = await sql`SELECT verified_at,expires_at FROM administrator_reauth WHERE session_hash=${hash}`
  assert.equal(Number(proof.expires_at) - Number(proof.verified_at), 300)
  const [sessionAfter] = await sql`SELECT expires_at FROM sessions WHERE token_hash=${hash}`
  assert.equal(sessionAfter.expires_at, sessionBefore.expires_at)
  const [audit] = await sql`SELECT request_diff FROM management_audit WHERE object_path=${`${base}/finish`} ORDER BY id DESC LIMIT 1`
  assert.equal(audit.request_diff.requested.credential, '[已脱敏]')
  results.push('reauthentication UI resumes the protected action exactly once, with TOTP enabled, a five-minute proof and unchanged session')

  for (const bearer of [null, token]) {
    const status = await page.evaluate(async ({ base, bearer }) => (await fetch(`${base}/start`, {
      method: 'POST', credentials: 'omit', headers: bearer ? { Authorization: `Bearer ${bearer}` } : {},
    })).status, { base, bearer })
    assert.equal(status, bearer ? 403 : 401)
  }
  results.push('anonymous requests and management API tokens cannot obtain a reauthentication challenge')

  await sql`UPDATE admins SET totp_secret=NULL,totp_last_step=NULL WHERE id=1`
  await clearLimit()
  await expireProof()
  await page.getByLabel('名称', { exact: true }).fill('passkey-reauth-fallback')
  await page.getByRole('button', { name: '创建令牌', exact: true }).click()
  await dialog.waitFor()
  await page.evaluate(() => {
    window.originalCredentialGet = navigator.credentials.get
    navigator.credentials.get = () => Promise.reject(new DOMException('Cancelled by user', 'NotAllowedError'))
  })
  await dialog.getByRole('button', { name: '使用 Passkey 验证', exact: true }).click()
  await dialog.getByRole('alert').filter({ hasText: '已取消或超时' }).waitFor()
  assert.equal(await tokenCount('passkey-reauth-fallback'), 0)
  await noProof()
  await page.evaluate(() => { navigator.credentials.get = window.originalCredentialGet; delete window.originalCredentialGet })
  await dialog.getByLabel('当前管理员密码', { exact: true }).fill(password)
  await dialog.getByRole('button', { name: '验证并继续', exact: true }).click()
  await dialog.waitFor({ state: 'hidden' })
  await page.getByRole('button', { name: '我已保存，隐藏令牌', exact: true }).click()
  assert.equal(await tokenCount('passkey-reauth-fallback'), 1)
  results.push('cancelled Passkey keeps the action pending and the password fallback resumes it')

  await clearLimit()
  const replay = await native(page, await ok(page, `${base}/start`))
  const replies = await Promise.all([request(page, `${base}/finish`, replay), request(page, `${base}/finish`, replay)])
  assert.deepEqual(replies.map(reply => reply.status).sort(), [200, 400])
  assert.equal((await request(page, `${base}/finish`, replay)).status, 400)
  results.push('reauthentication assertions are consumed once, including concurrent submissions')

  for (const mutation of ['signature', 'expired', 'purpose', 'session', 'generation', 'revoked_session']) {
    await clearLimit()
    await expireProof()
    const challenge = await ok(page, mutation === 'purpose' ? '/api/login/passkey/start' : `${base}/start`)
    const body = await native(page, challenge)
    if (mutation === 'signature') {
      const bytes = Buffer.from(body.credential.response.signature, 'base64url'); bytes[bytes.length - 1] ^= 1
      body.credential.response.signature = bytes.toString('base64url')
    } else if (mutation === 'expired') {
      await sql`UPDATE passkey_ceremonies SET expires_at=0 WHERE id=${challenge.challenge_id}`
    } else if (mutation === 'purpose') {
      const binding = (await context.cookies()).find(cookie => cookie.name === 'apsp_admin_passkey')
      await context.addCookies([{ ...binding, name: 'apsp_admin_reauth' }])
    } else if (mutation === 'session') {
      await ok(page, '/api/login', { password })
      await expireProof()
    } else if (mutation === 'generation') {
      await sql`UPDATE passkey_accounts SET generation=generation+1 WHERE id='00000000-0000-0000-0000-000000000001'`
    } else if (mutation === 'revoked_session') {
      const current = await sessionHash()
      await sql`UPDATE sessions SET expires_at=0 WHERE token_hash=${current}`
    }
    const response = await request(page, `${base}/finish`, body)
    assert.equal(response.status, mutation === 'revoked_session' ? 401 : 400, `${mutation}: ${JSON.stringify(response)}`)
    await noProof()
    if (mutation === 'revoked_session') await ok(page, '/api/login', { password })
    results.push(`reauthentication rejects ${mutation} without granting a proof`)
  }
  await clearLimit()
  await page.goto(`${origin}/#/system/administrator`)
  await page.getByRole('heading', { name: '账号安全', exact: true }).waitFor()
}
