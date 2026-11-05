import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { resolve, extname, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/expenses-ui.mjs`.
// Renewal items, server cost records (ADR 0097), the payment history and the
// expense settings work at desktop and phone widths.
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
    let entries = []
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
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `viewport ${width} overflows`)
    assert.equal(writes.filter(w => w.path.endsWith('/subscriptions')).length, 1)
    assert.deepEqual(errors, [])
    await page.close()
    console.log(`expense UI passed at ${width}px`)
  }
} finally { if (browser) await browser.close(); await new Promise(resolve => server.close(resolve)) }
