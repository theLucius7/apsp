import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { resolve, extname, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) return response.writeHead(400).end()
  try { response.writeHead(200, { 'Content-Type': ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css' })[extname(file)] ?? 'application/octet-stream' }).end(await readFile(file)) } catch { response.writeHead(404).end() }
})
let browser
try {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  const origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  for (const width of [1440, 390, 320]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } }), errors = [], writes = []
    page.on('pageerror', error => errors.push(error.message))
    let entries = [], allowLogin = false
    const account = "00000000-0000-4000-8000-000000000041"
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname, method = request.method()
      if (method !== 'GET') writes.push({ path, body: request.postDataJSON() })
      let value
      if (path === '/api/me') value = { authenticated: true }
      else if (path === '/api/dashboard/access') value = { authenticated: true, public_dashboard: false }
      else if (path === '/api/plugins/expenses/subscriptions') {
        if (method === 'POST') { const body = request.postDataJSON(); assert.equal(body.amount_units, '123456'); assert.equal(body.cycle, 'month'); assert.equal(body.auto_renew, true); entries = [{ ...body, id: 1, version: 1, anchor_payment: body.next_payment }] }
        value = method === 'GET' ? entries : entries[0]
      } else if (path === '/api/plugins/expenses/labels') value = { entries: [] }
      else if (path === '/api/plugins/expenses/servers') value = { entries: [{ id: 7, name: '测试服务器', amount_units: '125000', currency: 'USD', cycle_days: 30, next_renewal: '2027-01-01', auto_renewal: false }] }
      else if (path === '/api/plugins/expenses/references') value = { entries: [{ kind: 'proxy_source', id: 3, name: '测试机场', expires_at: 1798761600, expires_on: '2027-01-01', used_bytes: 3221225472, total_bytes: 10737418240, reported_at: 1790000000, path: '/plugins/sing-box/nodes?view=sources' }] }
      else if (path === '/api/plugins/expenses/payments') value = { items: [], servers: [{ kind: 'server', id: 'r1', server_id: 7, name: '测试服务器', record_kind: 'renewal', paid_on: '2026-10-01', valid_until: null, amount_minor: '1250', currency: 'USD', reference: 'order-1' }] }
      else if (path === '/api/servers/7/fleet/costs') {
        if (method === 'POST') { const body = request.postDataJSON(); assert.equal(body.amount_minor, 1250); assert.equal(body.currency, 'USD'); assert.equal(body.kind, 'renewal'); value = { id: 'r2' } }
        else value = { records: [{ id: 'r1', kind: 'renewal', amount_minor: 1250, currency: 'USD', occurred_at: 1790000000, valid_until: null, reference: 'order-1' }], annual: [{ year: 2026, currency: 'USD', amount_minor: '1250' }] }
      }
      else if (path === '/api/plugins/expenses/settings') value = { base_currency: 'CNY', budget_units: null, timezone: 'Asia/Shanghai', reminders_enabled: false, version: 1 }
      else if (path.endsWith('/commerce/overview')) value = { members: 43, pending_members: 0, balance_cents: '1234', orders: 3, sales_cents: '2500', open_exceptions: 0, invitations: 1, ledger_entries: 3 }
      else if (path.endsWith('/commerce/offers') || path.endsWith('/package-groups') || path.endsWith('/policy-groups')) value = []
      else if (path.endsWith('/commerce/compatibility')) value = { bridge: null, mappings: [] }
      else if (path.endsWith('/commerce/migration') || path.endsWith('/commerce/migration/records')) value = { entries: [] }
      else if (path.endsWith('/commerce-public/account/login')) return route.fulfill(allowLogin ? { json: { account_id: account } } : { status: 401, json: { error: '邮箱或密码错误' } })
      else if (path === `/api/plugins/sing-box/portal/${account}`) value = { authenticated: true, name: '测试成员', configuration: { enabled: false, reason: '夹具未启用认证器' }, subscription_url: 'https://legacy.example.com/assets/fixture-token', legacy_compatibility: { last_sync_at: 1 }, usage: { uplink: '20', downlink: '30' }, keys: [], subscription_status: { status: 'ready', message: '', entitlement: { package_name: '原套餐', used_bytes: '50', monthly_bytes: '1000', expires_at: null, next_reset: null } } }
      else if (path === `/api/plugins/sing-box/portal/${account}/commerce`) value = { enabled: true, profile: { user_id: 41, balance_cents: 1000, sponsor_required: false, state: 'active', email: 'fixture@example.com', email_verified: false, has_password: true, remind_expire: true, remind_traffic: true, remind_invitation: true }, offers: [], channels: [], content: [{ id: 1, kind: 'guide', category: '客户端', title: '安全指南', body: '<p>保留指南段落</p><a href="javascript:alert(1)">危险链接</a><script>window.guideExecuted = true</script><img src="data:text/html,unsafe" onerror="window.guideExecuted=true">' }], history: { ledger: [], orders: [], invitations: [], topups: [], legacy: [] }, invite_settings: { enabled: false, price_cents: 100 }, friends: [] }
      else value = []
      return route.fulfill({ json: value })
    })
    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/plugins/expenses`)
    await page.getByRole('button', { name: '添加续费项目', exact: true }).click()
    const dialog = page.getByRole('dialog')
    await dialog.getByLabel('名称', { exact: true }).fill('测试线路服务')
    await dialog.getByLabel('每期金额', { exact: true }).fill('12.3456')
    await dialog.getByLabel('下次续费', { exact: true }).fill('2027-01-31')
    await dialog.getByRole('button', { name: '保存', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    await page.getByRole('cell', { name: '测试线路服务', exact: true }).waitFor()
    // Servers come from asset settings; their cost records are edited here (ADR 0097).
    await page.getByRole('link', { name: '测试机场', exact: true }).waitFor()
    await page.getByRole('button', { name: '费用记录', exact: true }).click()
    const costs = page.getByRole('dialog')
    await costs.getByRole('cell', { name: 'order-1', exact: true }).waitFor()
    await costs.getByLabel('币种', { exact: true }).fill('USD')
    await costs.getByLabel(/^金额/).fill('12.5')
    await costs.getByRole('button', { name: '记录费用', exact: true }).click()
    for (let attempt = 0; attempt < 50 && !writes.some(write => write.path === '/api/servers/7/fleet/costs'); attempt++) await page.waitForTimeout(100)
    await costs.getByRole('button', { name: '关闭', exact: true }).click()
    await costs.waitFor({ state: 'hidden' })
    assert.ok(writes.some(write => write.path === '/api/servers/7/fleet/costs'), 'cost record posted')
    await page.getByRole('button', { name: '付款记录', exact: true }).click()
    await page.getByText('2026 年 · USD 12.5', { exact: true }).waitFor()
    await page.getByRole('button', { name: '退订提醒', exact: true }).click()
    await page.getByRole('heading', { name: '计划退订日期提醒' }).waitFor()
    await page.getByRole('button', { name: '配置', exact: true }).click()
    await page.getByRole('heading', { name: '统计与提醒设置' }).waitFor()
    await page.goto(`${origin}/#/plugins/sing-box/commerce`)
    await page.getByRole('heading', { name: '成员运营' }).waitFor()
    await page.getByRole('button', { name: '销售套餐', exact: true }).click()
    await page.getByRole('button', { name: '新增销售套餐', exact: true }).click()
    await page.getByRole('dialog').getByLabel('销售名称', { exact: true }).fill('测试套餐')
    await page.getByRole('dialog').getByRole('button', { name: '取消', exact: true }).click()
    await page.getByRole('button', { name: '迁移', exact: true }).click()
    await page.getByRole('heading', { name: '原订阅兼容通道' }).waitFor()
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `viewport ${width} overflows`)
    await page.goto(`${origin}/#/plugins/sing-box/member`)
    await page.getByLabel('邮箱', { exact: true }).fill('fixture@example.com')
    await page.getByLabel('密码', { exact: true }).fill('wrong-fixture-password')
    await page.getByRole('button', { name: '登录', exact: true }).click()
    await page.getByText('邮箱或密码错误', { exact: true }).waitFor()
    allowLogin = true
    await page.getByRole('button', { name: '登录', exact: true }).click()
    await page.getByRole('heading', { name: '测试成员', exact: true }).waitFor()
    await page.getByRole('button', { name: '公告与指南', exact: true }).click()
    await page.getByText('客户端 · 安全指南', { exact: true }).click()
    await page.getByText('保留指南段落', { exact: true }).waitFor()
    assert.equal(await page.locator('[data-slot=guide-body] script').count(), 0)
    assert.equal(await page.locator('[data-slot=guide-body] img').count(), 0)
    assert.equal(await page.getByText('危险链接', { exact: true }).getAttribute('href'), null)
    assert.equal(await page.evaluate(() => window.guideExecuted), undefined)
    assert.equal(writes.filter(w => w.path.endsWith('/subscriptions')).length, 1)
    assert.deepEqual(errors, [])
    await page.close()
    console.log(`member commerce / expense UI passed at ${width}px`)
  }
} finally { if (browser) await browser.close(); await new Promise(resolve => server.close(resolve)) }
