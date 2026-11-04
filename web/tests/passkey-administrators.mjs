import assert from 'node:assert/strict'

// Named administrators register and use their own passkeys (ADR 0107).
export async function administrators({ admin, client, clearLimit, native, request, ok, password, origin, results }) {
  const page = admin.page
  const operatorPassword = 'operator-test-password'
  await clearLimit()
  await ok(page, '/api/control-center/reauth', { password })
  const created = await ok(page, '/api/control-center/administrators', {
    login_name: 'operator', display_name: '运维值班', role: 'viewer', all_servers: true,
    capabilities: ['servers:read'], server_ids: [], password: operatorPassword, enabled: true,
  })
  await ok(page, '/api/control-center/administrators', {
    login_name: 'auditor', display_name: '审计', role: 'viewer', all_servers: true,
    capabilities: ['servers:read'], server_ids: [], password: 'auditor-test-password', enabled: true,
  })

  const operator = await client()
  const op = operator.page
  await op.goto(`${origin}/`)
  await clearLimit()
  await ok(op, '/api/login', { login_name: 'operator', password: operatorPassword })
  // A viewer without security rights still manages the keys of their own account.
  assert.deepEqual((await ok(op, '/api/security/passkeys', undefined, 'GET')).keys, [])
  const start = await ok(op, '/api/security/passkeys/register/start', { name: '值班密钥' })
  assert.equal(start.options.publicKey.user.name, 'operator')
  assert.equal(start.options.publicKey.user.displayName, '运维值班')
  const registered = await native(op, start, true)
  await ok(op, '/api/security/passkeys/register/finish', registered)
  const operatorKeys = (await ok(op, '/api/security/passkeys', undefined, 'GET')).keys
  assert.equal(operatorKeys.length, 1)
  const ownerKeys = (await ok(page, '/api/security/passkeys', undefined, 'GET')).keys
  assert.ok(ownerKeys.length > 0 && !ownerKeys.some(key => key.id === operatorKeys[0].id))
  results.push('named administrator registers a passkey of their own, labelled with their login name')

  // Passkey re-authentication for high-risk actions uses the same account.
  await clearLimit()
  const reauthChallenge = await ok(op, '/api/control-center/reauth/passkey/start')
  const proof = await ok(op, '/api/control-center/reauth/passkey/finish', await native(op, reauthChallenge))
  assert.ok(proof.expires_at > proof.verified_at)
  results.push('named administrator re-authenticates with their own passkey')

  // Sign in through the shipped login page with the login name.
  await ok(op, '/api/logout')
  await op.goto(`${origin}/`)
  await op.getByRole('heading', { name: '欢迎回来', exact: true }).waitFor()
  await op.getByLabel('管理员登录名', { exact: true }).fill('operator')
  await clearLimit()
  await op.getByRole('button', { name: '使用 Passkey 登录', exact: true }).click()
  await op.locator('[data-slot=account-menu]').first().waitFor({ timeout: 15000 })
  const me = await ok(op, '/api/control-center/me', undefined, 'GET')
  assert.equal(me.admin_id, created.id)
  assert.equal(me.login_name, 'operator')
  results.push('named administrator signs in with their passkey from the login page')

  // A key answers only for its own administrator: the owner's challenge
  // lists the owner's keys, and the operator's key cannot satisfy it.
  await clearLimit()
  const ownerChallenge = await ok(op, '/api/login/passkey/start', { login_name: 'admin' })
  assert.ok(!ownerChallenge.options.publicKey.allowCredentials.some(item => item.id === registered.credential.id))
  const forged = await native(op, ownerChallenge, false, registered.credential.id)
  assert.equal((await request(op, '/api/login/passkey/finish', forged)).status, 400)
  results.push('one administrator key cannot sign in as another administrator')

  // Unknown names and names without keys get the same answer.
  await clearLimit()
  const unknown = await request(op, '/api/login/passkey/start', { login_name: 'nobody' })
  const withoutKeys = await request(op, '/api/login/passkey/start', { login_name: 'auditor' })
  assert.equal(unknown.status, 400)
  assert.deepEqual(unknown, withoutKeys)

  // A disabled administrator can no longer start a passkey sign-in.
  await clearLimit()
  await ok(page, '/api/control-center/reauth', { password })
  const listed = await ok(page, '/api/control-center/administrators', undefined, 'GET')
  const row = (listed.administrators ?? listed).find(item => (item.admin_id ?? item.id) === created.id)
  await ok(page, `/api/control-center/administrators/${created.id}`, {
    login_name: 'operator', display_name: '运维值班', role: 'viewer', all_servers: true,
    capabilities: ['servers:read'], server_ids: [], password: null, enabled: false, expected_revision: row.revision,
  }, 'PUT')
  await clearLimit()
  assert.deepEqual(await request(op, '/api/login/passkey/start', { login_name: 'operator' }), unknown)
  results.push('disabled administrators lose passkey sign-in; unknown names are indistinguishable')
}
