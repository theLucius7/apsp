import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { resolve, extname, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/member-accounts.mjs`.
// Proxy user accounts without member commerce: the former 成员运营 address opens
// 代理用户, whose 门户设置 dialog and per-user 邀请 tab manage invitation quota;
// a user signs in with email and password, generates a one-time invitation code
// in 我的账号 (a retried request keeps its identity), reads sanitised guides and
// keeps reminders and notices; a visitor redeems a code on the join page.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) return response.writeHead(400).end()
  try { response.writeHead(200, { 'Content-Type': ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css' })[extname(file)] ?? 'application/octet-stream' }).end(await readFile(file)) } catch { response.writeHead(404).end() }
})
const root = '/api/plugins/sing-box'
const account = '00000000-0000-4000-8000-000000000041'
const code = 'ABCD-EFGH-JKMN-PQRS'
const user = { id: 1, name: 'TEST_ONLY 成员', subscription_token: 'TEST_ONLY', subscription_url: 'http://127.0.0.1/s/TEST_ONLY' }
const policies = [{ id: 1, name: 'TEST_ONLY 策略', node_ids: [], chain_ids: [], member_count: 1 }]
const packages = [{ id: 2, name: 'TEST_ONLY 套餐', monthly_bytes: '1073741824', reset_day: 1, reset_hour: 0, reset_minute: 0, timezone: 'UTC', duration_days: 30 }]
const diagnosis = { account: { name: user.name, portal_created: true, keys: 1, active_sessions: 0, activation_expires_at: null },
  subscription: { status: 'empty', message: 'TEST_ONLY 未验证设备状态。', granted_nodes: 0, ready_managed_nodes: 0, ready_external_nodes: 0 },
  legacy: null, permissions: [], ledger: [], rotations: [], events: [], limitations: {} }
const template = { template: null, definition_redacted: false, credential_access_reason: 'TEST_ONLY 完整模板未读取。', limitations: 'TEST_ONLY 没有保存的模板。' }
const guide = { id: 1, kind: 'guide', category: '客户端', title: '安全指南', published: true, version: 1, body: '<p>保留指南段落</p><a href="javascript:alert(1)">危险链接</a><script>window.guideExecuted = true</script><img src="data:text/html,unsafe" onerror="window.guideExecuted=true">' }
const fits = page => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)

let browser
try {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  const origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  for (const width of [1440, 390, 320]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } }), errors = [], writes = [], unexpected = []
    page.on('pageerror', error => errors.push(error.message))
    const dialogs = []
    page.on('dialog', dialog => { dialogs.push(dialog.message()); void dialog.dismiss() })
    let allowLogin = false, dropNextInvitation = true, remaining = 3, quota = 3
    let settings = { enabled: false, package_group_id: null, policy_group_ids: [], default_quota: 0, version: 1 }, guides = [guide]
    const issued = [{ id: 'inv-1', hint: 'WXYZ', origin: 'quota', state: 'claimed', created_at: 1790000000, claimed_at: 1790000100, revoked_at: null, recipient: { id: 42, name: 'TEST_ONLY 受邀人' } }]
    const messages = [{ id: 1, message: 'TEST_ONLY 套餐即将到期', created_at: 1790000000, read_at: null }]
    await page.route('**/api/**', async route => {
      const request = route.request(), url = new URL(request.url()), path = url.pathname, method = request.method()
      const body = method === 'GET' || method === 'DELETE' ? undefined : request.postDataJSON()
      if (method !== 'GET') writes.push({ method, path, body })
      let value
      if (path === '/api/me') value = { authenticated: true }
      else if (path === '/api/dashboard/access') value = { authenticated: true, public_dashboard: false }
      // Administrator: 代理用户 with the portal settings dialog and the per-user invitation tab.
      else if (method === 'GET' && path === `${root}/users`) value = [user]
      else if (method === 'GET' && path === `${root}/usage`) value = { uplink: '0', downlink: '0', total: '0', by_user: [], by_node: [] }
      else if (method === 'GET' && path === `${root}/policy-groups`) value = policies
      else if (method === 'GET' && path === `${root}/package-groups`) value = packages
      else if (method === 'GET' && path === `${root}/users/1/entitlement`) value = { user_id: 1, package_group_id: null, package_name: null, monthly_bytes: null, starts_at: null, expires_at: null, cycle_start: null, next_reset: null, used_bytes: '0', status: 'unmetered', allowed: true }
      else if (method === 'GET' && path === `${root}/users/1/policy-groups`) value = { group_ids: [1] }
      else if (method === 'GET' && path === `${root}/users/1/portal`) value = { configuration: { enabled: true, reason: null, origin }, keys: 1, url: `${origin}/#/account/${account}`, activation_expires_at: null }
      else if (method === 'GET' && !url.search && path === `${root}/users/1/diagnosis`) value = diagnosis
      else if (method === 'GET' && !url.search && path === `${root}/users/1/client-template`) value = template
      else if (method === 'GET' && path === `${root}/users/1/invitations`) value = { remaining: quota, issued, invited_by: { id: 9, name: 'TEST_ONLY 邀请人' }, events: [{ id: 1, delta: 3, remaining: 3, reason: 'balance_conversion', detail: { balance_cents: 300, price_cents: 100 }, created_at: 1790000000 }] }
      else if (method === 'POST' && path === `${root}/users/1/invitations/quota`) { quota += body.delta; value = { remaining: quota } }
      else if (path === `${root}/invitation-settings`) {
        if (method === 'PUT') { assert.equal(body.version, settings.version); settings = { ...body, version: settings.version + 1 } }
        value = settings
      }
      else if (method === 'GET' && path === `${root}/account-mail`) value = { enabled: false, reminders_enabled: false, host: '', port: 465, security: 'tls', username: '', from_address: '', credential_id: null, blocked_domains: [], version: 1 }
      else if (method === 'GET' && path === `${root}/portal-guides`) value = { entries: guides }
      else if (method === 'DELETE' && path === `${root}/portal-guides/${guide.id}`) { guides = []; return route.fulfill({ status: 204, body: '' }) }
      // The user's own portal.
      else if (method === 'POST' && path === `${root}/portal-public/account/login`) return route.fulfill(allowLogin ? { json: { account_id: account } } : { status: 401, json: { error: '邮箱或密码错误' } })
      else if (method === 'GET' && path === `${root}/portal/${account}`) value = { authenticated: true, name: '测试成员', configuration: { enabled: false, reason: '夹具未启用认证器' }, subscription_url: 'https://legacy.example.com/assets/fixture-token', legacy_compatibility: { last_sync_at: 1 }, usage: { uplink: '20', downlink: '30' }, keys: [], subscription_status: { status: 'ready', message: '', entitlement: { package_name: '原套餐', used_bytes: '50', monthly_bytes: '1000', expires_at: null, next_reset: null } } }
      else if (method === 'GET' && path === `${root}/portal/${account}/lines`) value = { generated_at: 1, package: '原套餐', lines: [], hosts: [] }
      else if (method === 'GET' && path === `${root}/portal/${account}/account`) value = { profile: { email: 'fixture@example.com', email_verified: false, has_password: true, remind_expire: true, remind_traffic: true }, guides: [guide], invitations: { enabled: true, remaining, issued } }
      else if (method === 'POST' && path === `${root}/portal/${account}/invitations`) {
        // The first answer is lost; the retry must carry the same request identity.
        if (dropNextInvitation) { dropNextInvitation = false; return route.fulfill({ status: 503, json: { error: 'TEST_ONLY 响应丢失，请重试' } }) }
        remaining -= body.quantity
        issued.push({ id: 'inv-2', hint: code.slice(-4), origin: 'quota', state: 'available', created_at: 1790000200, claimed_at: null, revoked_at: null, recipient: null })
        value = { codes: [code], links: [`${origin}/#/join/${code}`] }
      }
      else if (method === 'POST' && path === `${root}/portal/${account}/invitations/inv-2/revoke`) { Object.assign(issued.find(entry => entry.id === 'inv-2'), { state: 'revoked', revoked_at: 1790000300 }); remaining += 1; value = {} }
      else if (method === 'PUT' && path === `${root}/portal/${account}/account/reminders`) value = {}
      else if (method === 'GET' && path === `${root}/portal/${account}/account/messages`) value = { entries: messages }
      else if (method === 'POST' && path === `${root}/portal/${account}/account/messages/read`) { for (const message of messages) if (body.ids.includes(message.id)) message.read_at = 1790000400; value = {} }
      // A visitor with an invitation code.
      else if (method === 'POST' && path === `${root}/portal-public/invitation`) {
        if (body.token !== code) return route.fulfill({ status: 404, json: { error: '邀请不存在' } })
        value = { available: true, package_name: 'TEST_ONLY 套餐', email_verification_required: false, configuration: { enabled: true } }
      }
      else if (method === 'POST' && path === `${root}/portal-public/claim`) value = { url: `${origin}/#/account/${account}?activate=${'A'.repeat(43)}` }
      else if (method === 'GET' && /^\/api\/plugins\/sing-box\/users\/\d+\/limits$/.test(path)) value = { user_id: Number(path.split('/')[5]), revision: 0, upload_mbps: 0, download_mbps: 0, max_connections: 0, servers: [] } // Per-user limits (ADR 0124) are read on the user page; none are set here.
      else { unexpected.push(`${method} ${path}`); return route.fulfill({ status: 404, json: { error: 'TEST_ONLY 未知请求' } }) }
      return route.fulfill({ json: value })
    })
    await installControlCenterFixtures(page)

    // The former member operations address opens 代理用户; no 成员运营 entry remains.
    await page.goto(`${origin}/#/users`)
    await page.locator('[data-slot=page-header]').getByRole('heading', { name: '代理用户', exact: true }).waitFor()
    assert.equal(await page.getByRole('link', { name: '成员运营', includeHidden: true }).count(), 0)
    await page.locator('[data-slot=user-heading]').getByRole('heading', { name: user.name, exact: true }).waitFor()
    await page.getByRole('button', { name: '门户设置', exact: true }).click()
    const portal = page.getByRole('dialog', { name: '门户设置' })
    const portalTabs = portal.getByRole('navigation', { name: '门户设置' })
    await portalTabs.getByRole('button', { name: '公告与指南', exact: true }).waitFor()
    // The owner also sets the users' own addresses here; it used to be a separate button.
    assert.deepEqual(await portalTabs.getByRole('button').allTextContents(), ['邀请', '登录邮件', '公告与指南', '专属地址'])
    await portal.getByText('允许代理用户用邀请名额生成邀请码', { exact: true }).click()
    await portal.getByLabel('受邀账号的套餐').selectOption('2')
    await portal.getByRole('checkbox', { name: 'TEST_ONLY 策略', exact: true }).check()
    await portal.getByLabel('受邀账号自带的邀请名额').fill('2')
    await portal.getByRole('button', { name: '保存邀请设置', exact: true }).click()
    for (let attempt = 0; attempt < 50 && settings.version === 1; attempt++) await page.waitForTimeout(50)
    assert.deepEqual(writes.filter(write => write.path === `${root}/invitation-settings`).map(write => write.body), [{ enabled: true, package_group_id: 2, policy_group_ids: [1], default_quota: 2, version: 1 }])
    await portal.getByRole('button', { name: '登录邮件', exact: true }).click()
    await portal.getByLabel('SMTP 主机').waitFor()
    await portal.getByRole('button', { name: '公告与指南', exact: true }).click()
    await portal.getByText('安全指南').first().waitFor()
    assert.ok(await portal.evaluate(element => element.scrollWidth <= element.clientWidth + 1), `portal settings fit ${width}px`)
    // An outdated guide is deleted after a confirmation; the list follows.
    await portal.getByRole('row').filter({ hasText: '安全指南' }).getByRole('button', { name: '删除', exact: true }).click()
    const confirm = page.getByRole('alertdialog', { name: '删除「安全指南」？' })
    await confirm.getByRole('button', { name: '确认删除', exact: true }).click()
    await confirm.waitFor({ state: 'hidden' })
    await portal.getByText('安全指南', { exact: true }).waitFor({ state: 'hidden' })
    assert.deepEqual(writes.filter(write => write.method === 'DELETE').map(write => write.path), [`${root}/portal-guides/${guide.id}`])
    await page.keyboard.press('Escape')
    await portal.waitFor({ state: 'hidden' })
    // One user's quota, the codes they issued and who invited them.
    await page.getByRole('navigation', { name: '更多用户信息' }).getByRole('button', { name: '邀请', exact: true }).click()
    await page.getByText('剩余名额 3 · 由 TEST_ONLY 邀请人 邀请', { exact: true }).waitFor()
    await page.getByRole('cell', { name: '····-WXYZ', exact: true }).waitFor()
    await page.getByLabel('增减名额').fill('2')
    await page.getByLabel('备注').fill('TEST_ONLY 补发')
    await page.getByRole('button', { name: '调整', exact: true }).click()
    await page.getByText('剩余名额 5 · 由 TEST_ONLY 邀请人 邀请', { exact: true }).waitFor()
    assert.deepEqual(writes.filter(write => write.path === `${root}/users/1/invitations/quota`).map(write => write.body), [{ delta: 2, reason: 'TEST_ONLY 补发' }])
    assert.ok(await fits(page), `proxy users fit ${width}px`)

    // Email and password sign-in.
    await page.goto(`${origin}/#/member`)
    await page.getByLabel('邮箱', { exact: true }).fill('fixture@example.com')
    await page.getByLabel('密码', { exact: true }).fill('wrong-fixture-password')
    await page.getByRole('button', { name: '登录', exact: true }).click()
    await page.getByText('邮箱或密码错误', { exact: true }).waitFor()
    allowLogin = true
    await page.getByRole('button', { name: '登录', exact: true }).click()
    await page.getByRole('heading', { name: '测试成员', exact: true }).waitFor()
    // Without an imported catalogue the line panel leaves once its answer arrives.
    await page.locator('[data-slot=member-lines]').waitFor({ state: 'detached' })

    // 我的账号: a one-time invitation code; a lost answer is retried with the same identity.
    await page.getByText('剩余邀请名额 3', { exact: true }).waitFor()
    const tabs = page.getByRole('navigation', { name: '我的账号' })
    await tabs.getByRole('button', { name: '通知', exact: true }).waitFor()
    assert.deepEqual(await tabs.getByRole('button').allTextContents(), ['邀请', '公告与指南', '账号与提醒', '通知'])
    await page.getByRole('cell', { name: '····-WXYZ', exact: true }).waitFor()
    await page.getByText('TEST_ONLY 受邀人', { exact: true }).waitFor()
    await page.getByRole('button', { name: '生成邀请码', exact: true }).click()
    await page.getByText('TEST_ONLY 响应丢失，请重试', { exact: true }).waitFor()
    await page.getByRole('button', { name: '生成邀请码', exact: true }).click()
    await page.getByText('邀请码只显示这一次，请复制后发给对方。', { exact: true }).waitFor()
    await page.getByText(`邀请码 ${code}`, { exact: true }).waitFor()
    assert.equal(await page.locator('[data-slot=copy-field] code').filter({ hasText: `/#/join/${code}` }).count(), 1)
    const created = writes.filter(write => write.path === `${root}/portal/${account}/invitations`)
    assert.equal(created.length, 2)
    assert.equal(created[0].body.quantity, 1)
    assert.match(created[0].body.request_id, /^[0-9a-f-]{36}$/)
    assert.deepEqual(created[1].body, created[0].body, 'a retry keeps the same request identity')
    await page.getByText('剩余邀请名额 2', { exact: true }).waitFor()
    const fresh = page.getByRole('row').filter({ hasText: '····-PQRS' })
    await fresh.getByText('待使用', { exact: true }).waitFor()
    await fresh.getByRole('button', { name: '撤销', exact: true }).click()
    await page.getByText('邀请码已撤销，名额已退回。', { exact: true }).waitFor()
    await fresh.getByText('已撤销', { exact: true }).waitFor()
    await page.getByText('剩余邀请名额 3', { exact: true }).waitFor()
    assert.ok(await fits(page), `account panel fits ${width}px`)

    // Guides are sanitised.
    await tabs.getByRole('button', { name: '公告与指南', exact: true }).click()
    await page.getByText('客户端 · 安全指南', { exact: true }).click()
    await page.getByText('保留指南段落', { exact: true }).waitFor()
    assert.equal(await page.locator('[data-slot=guide-body] script').count(), 0)
    assert.equal(await page.locator('[data-slot=guide-body] img').count(), 0)
    assert.equal(await page.getByText('危险链接', { exact: true }).getAttribute('href'), null)
    assert.equal(await page.evaluate(() => window.guideExecuted), undefined)

    // Sign-in settings and reminders, then notices.
    await tabs.getByRole('button', { name: '账号与提醒', exact: true }).click()
    await page.getByText('联系邮箱：fixture@example.com · 未验证', { exact: true }).waitFor()
    await page.getByRole('checkbox', { name: '流量达到 80% 提醒', exact: true }).uncheck()
    await page.getByRole('button', { name: '保存提醒偏好', exact: true }).click()
    await page.getByText('提醒设置已保存。', { exact: true }).waitFor()
    assert.deepEqual(writes.filter(write => write.path.endsWith('/account/reminders')).map(write => write.body), [{ remind_expire: true, remind_traffic: false }])
    await tabs.getByRole('button', { name: '通知', exact: true }).click()
    const notices = page.locator('[data-slot=member-message]').filter({ hasText: 'TEST_ONLY 套餐即将到期' })
    await notices.and(page.locator('[data-unread]')).waitFor()
    await page.getByText('1 条未读', { exact: true }).waitFor()
    await page.getByRole('button', { name: '全部标为已读', exact: true }).click()
    await notices.and(page.locator(':not([data-unread])')).waitFor()
    await page.getByText('没有未读通知', { exact: true }).waitFor()
    assert.deepEqual(writes.filter(write => write.path.endsWith('/account/messages/read')).map(write => write.body), [{ ids: [1] }])

    // A visitor types a code: an unknown one is explained and the input stays.
    await page.goto(`${origin}/#/join`)
    await page.getByRole('heading', { name: '用邀请码创建账号', exact: true }).waitFor()
    const input = page.getByLabel('邀请码', { exact: true })
    assert.equal(await input.getAttribute('placeholder'), 'XXXX-XXXX-XXXX-XXXX')
    await input.fill('ZZZZ-ZZZZ-ZZZZ-ZZZZ')
    await page.getByRole('button', { name: '下一步', exact: true }).click()
    await page.getByText('邀请码不存在，请核对后重新输入。', { exact: true }).waitFor()
    assert.equal(await input.inputValue(), 'ZZZZ-ZZZZ-ZZZZ-ZZZZ')
    await input.fill(` ${code} `)
    await page.getByRole('button', { name: '下一步', exact: true }).click()
    await page.getByText('创建后即可使用「TEST_ONLY 套餐」。', { exact: true }).waitFor()
    assert.ok(await fits(page), `join page fits ${width}px`)
    await page.getByLabel('名称', { exact: true }).fill('TEST_ONLY 新成员')
    await page.getByLabel('联系邮箱', { exact: true }).fill('new@example.com')
    await page.getByRole('button', { name: '创建账号', exact: true }).click()
    // The portal opens with the one-time activation and then drops it from the address.
    await page.waitForFunction(expected => location.hash === expected, `#/account/${account}`)
    const claim = writes.find(write => write.path === `${root}/portal-public/claim`)
    assert.equal(claim.body.token, code); assert.equal(claim.body.name, 'TEST_ONLY 新成员'); assert.equal(claim.body.email, 'new@example.com')
    assert.equal(claim.body.challenge_id, null); assert.match(claim.body.request_id, /^[0-9a-f-]{36}$/)
    // A link carrying the code checks it at once.
    await page.goto(`${origin}/#/join/${code}`)
    await page.getByText('创建后即可使用「TEST_ONLY 套餐」。', { exact: true }).waitFor()
    assert.deepEqual(writes.filter(write => write.path === `${root}/portal-public/invitation`).map(write => write.body.token), ['ZZZZ-ZZZZ-ZZZZ-ZZZZ', code, code])

    // Links sent before the page addresses were renamed open the same page under the current address.
    await page.goto(`${origin}/#/join/${code}`)
    await page.getByText('创建后即可使用「TEST_ONLY 套餐」。', { exact: true }).waitFor()
    assert.equal(new URL(page.url()).hash, `#/join/${code}`)
    await page.goto(`${origin}/#/member`)
    await page.getByLabel('邮箱', { exact: true }).waitFor()
    assert.equal(new URL(page.url()).hash, '#/member')

    assert.deepEqual(unexpected, [])
    assert.deepEqual(dialogs, [])
    assert.deepEqual(errors, [])
    await page.close()
    console.log(`member accounts passed at ${width}px`)
  }
} finally { if (browser) await browser.close(); await new Promise(resolve => server.close(resolve)) }
