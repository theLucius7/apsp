import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile, mkdir } from 'node:fs/promises'
import { resolve, extname, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/expenses-ui.mjs`.
// The costs and renewals ledger (ADR 0118): one list of services and servers, the
// timeline, spending, entry details with their own addresses, the item form and
// the settings dialog, at desktop and phone widths. Never connects to a panel.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png' }
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) return response.writeHead(400).end()
  try { response.writeHead(200, { 'Content-Type': types[extname(file)] ?? 'application/octet-stream' }).end(await readFile(file)) } catch { response.writeHead(404).end() }
})
const screenshots = process.env.APSP_UI_SCREENSHOT_DIR
if (screenshots) await mkdir(screenshots, { recursive: true })

const now = Math.floor(Date.UTC(2026, 10, 6, 4) / 1000)
const day = date => Math.floor(Date.parse(`${date}T00:00:00Z`) / 1000)
const rates = { base: 'CNY', rates: { USD: 0.14 }, rate_dates: {}, rate_date: '2026-11-05', source: 'frankfurter', source_url: null, fetched_at: now, attempted_at: now, next_refresh_at: now, stale: false, status: 'fresh', error_code: null }
const expense = (id, name, fields = {}) => ({ id, name, amount_units: '200000', currency: 'USD', start_date: null, next_payment: '2026-11-09', anchor_payment: '2026-01-09', cycle: 'month', frequency: 1, category_id: 1, payment_method_id: 2, payer_id: null, notes: '', url: '', inactive: false, auto_renew: true, notify: true, notify_days_before: 3, cancellation_date: null, replacement_id: null, version: 1, ...fields })

function fixtures() {
  return {
    items: [
      expense(11, 'ChatGPT Plus', { url: 'https://chat.example/' }),
      expense(12, 'example.com', { amount_units: '900000', currency: 'CNY', cycle: 'year', next_payment: '2027-03-02', anchor_payment: '2026-03-02', category_id: 3, auto_renew: false, notes: '在 Cloudflare 续费' }),
      expense(13, '云盘 2TB', { amount_units: '80000', currency: 'CNY', next_payment: '2026-11-01', anchor_payment: '2026-01-01', auto_renew: false, category_id: null }),
      expense(15, '旧 VPN 服务', { inactive: true }),
    ],
    labels: [{ id: 1, kind: 'category', name: 'AI 服务', enabled: true }, { id: 3, kind: 'category', name: '域名', enabled: true }, { id: 2, kind: 'payment_method', name: '信用卡', enabled: true }],
    servers: [
      { id: 7, name: 'hk1 · 阿里云香港', amount_units: '340000', currency: 'CNY', cycle_days: 30, next_renewal: '2026-11-20', auto_renewal: true },
      { id: 8, name: 'tw0 · 住宅', amount_units: '125000', currency: 'USD', cycle_days: 30, next_renewal: '2026-11-08', auto_renewal: false },
    ],
    costs: { 8: { records: [
      { id: 'r2', kind: 'purchase', amount_minor: 1250, currency: 'USD', occurred_at: day('2026-09-08'), valid_until: null, reference: 'order-1' },
      { id: 'r3', kind: 'cancellation', amount_minor: 0, currency: 'USD', occurred_at: day('2026-11-01'), valid_until: day('2026-12-08'), reference: '到期不续' },
    ], annual: [{ year: 2026, currency: 'USD', amount_minor: '1250' }] }, 7: { records: [], annual: [] } },
    reminders: [{ record_id: 'r3', server_id: 8, server_name: 'tw0 · 住宅', valid_until: day('2026-12-08'), reference: '到期不续', enabled: false, lead_secs: 604800, notified_at: null, last_error: null }],
    payments: { items: [{ kind: 'item', id: 'p1', subscription_id: 12, name: 'example.com', paid_on: '2026-03-02', period_start: '2026-03-02', amount_units: '900000', currency: 'CNY' }],
      servers: [{ kind: 'server', id: 'r2', server_id: 8, name: 'tw0 · 住宅', record_kind: 'purchase', paid_on: '2026-09-08', valid_until: null, amount_minor: '1250', currency: 'USD', reference: 'order-1' }] },
    settings: { base_currency: 'CNY', budget_units: '5000000', timezone: 'Asia/Shanghai', reminders_enabled: true, version: 3 },
  }
}

let browser, page
const results = []
try {
  await new Promise(done => server.listen(0, '127.0.0.1', done))
  const origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  for (const width of [1440, 390, 320]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 }, acceptDownloads: true })
    page = await context.newPage()
    const errors = [], writes = [], unexpected = [], reads = []
    page.on('pageerror', error => errors.push(error.message))
    await page.clock.install({ time: now * 1000 })
    const state = fixtures()
    await page.route('**/api/**', async route => {
      const request = route.request(), url = new URL(request.url()), path = url.pathname, method = request.method()
      if (method === 'GET') reads.push(path); else writes.push({ path, method, body: request.postDataJSON() })
      const json = value => route.fulfill({ json: value })
      if (path === '/api/dashboard/access') return json({ authenticated: true, public_dashboard: false })
      if (path === '/api/plugins/expenses/subscriptions' && method === 'GET') return json(state.items)
      if (path === '/api/plugins/expenses/subscriptions' && method === 'POST') { const created = { ...request.postDataJSON(), id: 40, anchor_payment: request.postDataJSON().next_payment, version: 1 }; delete created.version; state.items.push({ ...created, version: 1 }); return json({ ...created, version: 1 }) }
      const item = path.match(/^\/api\/plugins\/expenses\/subscriptions\/(\d+)(?:\/(renew|clone|payments))?$/)
      if (item) {
        const target = state.items.find(entry => entry.id === Number(item[1]))
        if (item[2] === 'payments') return json({ entries: state.payments.items.filter(payment => payment.subscription_id === target?.id).map(({ id, period_start, paid_on, amount_units, currency }) => ({ id, period_start, paid_on, amount_units, currency, created_at: now })) })
        if (item[2] === 'renew') { Object.assign(target, { next_payment: '2026-12-01', version: target.version + 1 }); return json(target) }
        if (item[2] === 'clone') { const copy = { ...target, id: 41, name: `${target.name}（副本）`, inactive: true, notify: false, version: 1 }; state.items.push(copy); return json(copy) }
        if (method === 'DELETE') { state.items = state.items.filter(entry => entry !== target); return json({}) }
        if (method === 'PUT') { Object.assign(target, request.postDataJSON(), { version: target.version + 1 }); return json(target) }
      }
      if (path === '/api/plugins/expenses/labels' && method === 'GET') return json({ entries: state.labels })
      if (path === '/api/plugins/expenses/labels' && method === 'POST') { const label = { id: 50 + state.labels.length, ...request.postDataJSON() }; state.labels.push(label); return json(label) }
      if (path === '/api/plugins/expenses/servers') return json({ entries: state.servers })
      if (path === '/api/plugins/expenses/summary') return json({ from: '2026-11-06', base_currency: state.settings.base_currency, budget_units: state.settings.budget_units, exchange: rates,
        upcoming: [{ kind: 'server', id: 8, name: 'tw0 · 住宅', date: '2026-11-08', amount_units: '125000', currency: 'USD', auto_renew: false }, { kind: 'item', id: 11, name: 'ChatGPT Plus', date: '2026-11-09', amount_units: '200000', currency: 'USD', auto_renew: true }] })
      if (path === '/api/plugins/expenses/calendar') {
        assert.equal(url.searchParams.get('from'), '2026-11-06')
        return json({ entries: [
          { kind: 'server', id: 8, name: 'tw0 · 住宅', date: '2026-11-08', amount_units: '125000', currency: 'USD', auto_renew: false },
          { kind: 'item', id: 11, name: 'ChatGPT Plus', date: '2026-11-09', amount_units: '200000', currency: 'USD', auto_renew: true },
          { kind: 'reference', id: 3, name: '备用机场', date: '2026-11-20', amount_units: null, currency: '', auto_renew: false, path: '/plugins/sing-box/nodes?view=sources' },
          { kind: 'server', id: 7, name: 'hk1 · 阿里云香港', date: '2026-11-20', amount_units: '340000', currency: 'CNY', auto_renew: true },
          { kind: 'item', id: 11, name: 'ChatGPT Plus', date: '2026-12-09', amount_units: '200000', currency: 'USD', auto_renew: true },
        ] })
      }
      if (path === '/api/plugins/expenses/calendar.ics') return route.fulfill({ status: 200, contentType: 'text/calendar', body: 'BEGIN:VCALENDAR\r\nEND:VCALENDAR\r\n' })
      if (path === '/api/plugins/expenses/payments') return json(state.payments)
      if (path === '/api/plugins/expenses/settings' && method === 'GET') return json(state.settings)
      if (path === '/api/plugins/expenses/settings' && method === 'PUT') { Object.assign(state.settings, request.postDataJSON(), { version: state.settings.version + 1 }); return json(state.settings) }
      if (path === '/api/plugins/expenses/notifications') return json({ entries: [{ id: 'n1', kind: 'renewal_reminder', created_at: now - 3600, detail: { name: 'ChatGPT Plus', due: '2026-11-09' }, deliveries: [{ channel: 'telegram', state: 'sent', attempts: 1, last_error: null }] }] })
      if (path === '/api/plugins/expenses/import/preview') return json({ digest: 'a'.repeat(64), already_imported: false, subscriptions: 2, labels: 1, warnings: [] })
      if (path === '/api/plugins/expenses/import') return json({ imported: 2 })
      if (path === '/api/operations/cancellation-reminders' && method === 'GET') return json(state.reminders)
      if (path === '/api/operations/cancellation-reminders' && method === 'POST') { Object.assign(state.reminders[0], request.postDataJSON()); return json({}) }
      const costs = path.match(/^\/api\/servers\/(\d+)\/fleet\/costs$/)
      if (costs && method === 'GET') return json(state.costs[costs[1]] ?? { records: [], annual: [] })
      if (costs && method === 'POST') { state.costs[costs[1]].records.push({ id: 'r9', ...request.postDataJSON() }); return json({ id: 'r9' }) }
      unexpected.push(`${method} ${path}`)
      return route.fulfill({ status: 404, json: { error: 'TEST_ONLY' } })
    })
    await installControlCenterFixtures(page)
    const rows = page.locator('[data-slot=ledger-row]')
    const overflow = async label => assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `${label} overflows at ${width}px`)
    await page.goto(`${origin}/#/plugins/expenses`)
    await rows.first().waitFor()

    // One list: services and servers together, stopped ones hidden, what is due first.
    const stat = async label => (await page.locator('[data-slot=overview-item]').filter({ has: page.locator('[data-slot=overview-label]').getByText(label, { exact: true }) }).locator('[data-slot=overview-value]').innerText()).replace(/\s+/g, ' ')
    assert.match(await stat('每月费用'), /CNY/)
    assert.equal(await stat('待处理'), '3')
    assert.equal(await rows.count(), 5)
    assert.match(await rows.first().innerText(), /云盘 2TB[\s\S]*已过 5 天/)
    assert.equal(await rows.first().getByRole('button', { name: '确认续期', exact: true }).count(), 1, 'A manual renewal that is due has the action at hand')
    assert.equal(await rows.filter({ hasText: 'ChatGPT Plus' }).getByRole('button', { name: '确认续期', exact: true }).count(), 0, 'Automatic renewals do not ask')
    assert.equal(await rows.filter({ hasText: '旧 VPN 服务' }).count(), 0)
    await overflow('list')
    if (screenshots) await page.screenshot({ path: resolve(screenshots, `expenses-${width}.png`), fullPage: true, animations: 'disabled' })

    // An entry's detail has its own address and Back... or closing returns to the list.
    await rows.filter({ hasText: 'tw0 · 住宅' }).getByRole('button', { name: '费用记录', exact: true }).click()
    await page.waitForURL(/#\/plugins\/expenses\/servers\/8$/)
    const detail = page.locator('[data-dialog=ledger-detail]')
    await detail.locator('[data-slot=cost-record]').first().waitFor()
    assert.equal(await detail.locator('[data-slot=cost-record]').count(), 2)
    assert.equal(await detail.locator('[data-slot=cancellation-reminder]').count(), 1, 'A cancellation sets its reminder where it is recorded')
    await overflow('server detail')
    await detail.getByRole('button', { name: '关闭', exact: true }).first().click()
    await page.waitForURL(/#\/plugins\/expenses$/)

    if (width === 1440) {
      // Server detail: record a cost and switch on the cancellation reminder.
      await page.goto(`${origin}/#/plugins/expenses/servers/8`)
      await detail.locator('[data-slot=cancellation-reminder]').waitFor()
      await detail.getByRole('switch', { name: '退订提醒' }).click()
      await detail.locator('[data-slot=cancellation-reminder]').getByRole('button', { name: '保存', exact: true }).click()
      for (let attempt = 0; attempt < 40 && !writes.some(write => write.path === '/api/operations/cancellation-reminders'); attempt++) await page.waitForTimeout(50)
      assert.deepEqual(writes.find(write => write.path === '/api/operations/cancellation-reminders').body, { record_id: 'r3', enabled: true, lead_secs: 604800 })
      await detail.getByRole('button', { name: '登记费用', exact: true }).click()
      const costForm = detail.locator('[data-slot=cost-form]')
      await costForm.getByLabel(/^金额/).fill('12.5')
      await costForm.getByLabel('币种', { exact: true }).fill('USD')
      await costForm.getByRole('button', { name: '登记', exact: true }).click()
      await costForm.waitFor({ state: 'detached' })
      const cost = writes.find(write => write.path === '/api/servers/8/fleet/costs')
      assert.equal(cost.body.amount_minor, 1250)
      assert.equal(cost.body.currency, 'USD')
      assert.equal(cost.body.kind, 'renewal')
      await page.goBack()
      await page.waitForURL(/#\/plugins\/expenses$/)
      await detail.waitFor({ state: 'detached' })

      // Adding an item, with a category created on the spot.
      await page.getByRole('button', { name: '添加项目', exact: true }).click()
      const form = page.locator('[data-dialog=expense-form]')
      await form.getByLabel('名称', { exact: true }).fill('测试线路服务')
      await form.getByLabel('每期金额', { exact: true }).fill('12.3456')
      await form.getByLabel(/^下次续费/).fill('2027-01-31')
      await form.getByRole('button', { name: '新建分类', exact: true }).click()
      await form.getByLabel('新的分类名称', { exact: true }).fill('测试分类')
      await form.getByLabel('新的分类名称', { exact: true }).press('Enter')
      await form.getByRole('combobox', { name: '分类', exact: true }).locator('option:checked', { hasText: '测试分类' }).waitFor({ state: 'attached' })
      await form.getByRole('button', { name: '添加', exact: true }).last().click()
      await form.waitFor({ state: 'detached' })
      const created = writes.find(write => write.path === '/api/plugins/expenses/subscriptions' && write.method === 'POST').body
      assert.equal(created.amount_units, '123456')
      assert.equal(created.cycle, 'month')
      assert.equal(created.auto_renew, true)
      assert.equal(created.category_id, state.labels.find(label => label.name === '测试分类').id)
      assert.equal(writes.filter(write => write.path === '/api/plugins/expenses/labels').length, 1)
      await rows.filter({ hasText: '测试线路服务' }).waitFor()

      // Confirming a manual renewal records a payment once.
      await rows.first().getByRole('button', { name: '确认续期', exact: true }).click()
      const confirm = page.getByRole('alertdialog')
      await confirm.getByRole('button', { name: '确认续期', exact: true }).click()
      await confirm.waitFor({ state: 'detached' })
      const renew = writes.find(write => write.path === '/api/plugins/expenses/subscriptions/13/renew')
      assert.equal(renew.body.version, 1)
      assert.match(renew.body.request_id, /^[0-9a-f-]{36}$/)
      await page.getByText('「云盘 2TB」已记录付款，下次续费改为 2026-12-01。').waitFor()

      // An item opened by its address: the facts, its payments, and deleting asks first.
      await page.goto(`${origin}/#/plugins/expenses/items/12`)
      await detail.getByText('在 Cloudflare 续费').waitFor()
      await detail.getByText('覆盖 2026-03-02 起的一期').waitFor()
      assert.match(await detail.innerText(), /CNY 90 · 每年[\s\S]*手动续费[\s\S]*2026-03-02/)
      await detail.getByRole('button', { name: 'example.com 的更多操作', exact: true }).click()
      await page.getByRole('menuitem', { name: '删除', exact: true }).click()
      await detail.locator('[data-slot=confirm]').waitFor()
      assert.equal(writes.filter(write => write.method === 'DELETE').length, 0, 'Nothing is deleted before it is confirmed')
      await detail.getByRole('button', { name: '确认删除', exact: true }).click()
      await page.waitForURL(/#\/plugins\/expenses$/)
      assert.equal(writes.filter(write => write.method === 'DELETE' && write.path.endsWith('/subscriptions/12')).length, 1)

      // The timeline: past due first, then each month, and the calendar export.
      await page.getByRole('button', { name: '即将续费', exact: true }).click()
      await page.locator('[data-slot=month]').first().waitFor()
      assert.equal(await page.locator('[data-slot=month]').count(), 2)
      assert.match(await page.locator('[data-slot=month]').first().innerText(), /2026 年 11 月[\s\S]*4 笔/)
      assert.equal(await page.locator('[data-slot=due-row][data-kind=reference]').getAttribute('data-kind'), 'reference')
      const download = page.waitForEvent('download')
      await page.getByRole('button', { name: '导出日历', exact: true }).click()
      assert.equal((await download).suggestedFilename(), 'apsp-renewals.ics')
      if (screenshots) await page.screenshot({ path: resolve(screenshots, `expenses-upcoming-${width}.png`), fullPage: true, animations: 'disabled' })

      // Spending: the run rate by category against the budget, and what was paid.
      await page.getByRole('button', { name: '支出', exact: true }).click()
      await page.locator('[data-slot=budget]').waitFor()
      assert.equal(await page.locator('[data-slot=payment]').count(), 2)
      if (screenshots) await page.screenshot({ path: resolve(screenshots, `expenses-spending-${width}.png`), fullPage: true, animations: 'disabled' })

      // Settings in one place: budget, labels, reminder history and import.
      await page.getByRole('button', { name: '设置', exact: true }).click()
      const settings = page.locator('[data-dialog=ledger-settings]')
      await settings.getByLabel(/^每月预算/).fill('600')
      await settings.getByRole('button', { name: '保存', exact: true }).click()
      await settings.getByText('已保存', { exact: true }).waitFor()
      assert.equal(writes.find(write => write.path === '/api/plugins/expenses/settings').body.budget_units, '6000000')
      await settings.getByRole('button', { name: '分类与付款', exact: true }).click()
      await settings.getByLabel('新的付款方式', { exact: true }).fill('支付宝')
      await settings.getByLabel('新的付款方式', { exact: true }).press('Enter')
      await settings.getByText('支付宝', { exact: true }).waitFor()
      await settings.getByRole('button', { name: '提醒记录', exact: true }).click()
      await settings.getByText('Telegram · 已发送').waitFor()
      await settings.getByRole('button', { name: '导入导出', exact: true }).click()
      await settings.getByLabel('选择配置文件').setInputFiles({ name: 'wallos.json', mimeType: 'application/json', buffer: Buffer.from('{"TEST_ONLY":true}') })
      await settings.locator('[data-slot=import-preview]').getByText('将添加 2 个项目').waitFor()
      await settings.getByRole('checkbox').click()
      await settings.getByRole('button', { name: '导入', exact: true }).click()
      await settings.getByText('导入完成').waitFor()
      assert.equal(writes.find(write => write.path === '/api/plugins/expenses/import').body.confirmed_digest, 'a'.repeat(64))
      await settings.getByRole('button', { name: '完成', exact: true }).click()
    } else {
      // Phones get the same detail, form and settings without sideways scrolling.
      await page.goto(`${origin}/#/plugins/expenses/items/11`)
      await detail.getByRole('button', { name: '编辑', exact: true }).waitFor()
      await overflow('item detail')
      await detail.getByRole('button', { name: '编辑', exact: true }).click()
      await page.locator('[data-dialog=expense-form]').waitFor()
      await overflow('form')
      await page.locator('[data-dialog=expense-form]').getByRole('button', { name: '取消', exact: true }).click()
      await page.waitForURL(/#\/plugins\/expenses\/items\/11$/)
      await detail.getByRole('button', { name: '编辑', exact: true }).waitFor()
      await page.goto(`${origin}/#/plugins/expenses`)
      for (const view of ['即将续费', '支出']) { await page.getByRole('button', { name: view, exact: true }).click(); await page.waitForTimeout(200); await overflow(view) }
      await page.getByRole('button', { name: '设置', exact: true }).click()
      await page.locator('[data-dialog=ledger-settings]').waitFor()
      await overflow('settings')
    }
    assert.deepEqual(errors, [])
    assert.deepEqual(unexpected, [], `Unexpected API calls: ${unexpected.join(', ')}`)
    results.push(`${width}px`)
    await context.close()
  }
  console.log(`expense UI passed at ${results.join(', ')}`)
} catch (error) {
  if (screenshots && page) await page.screenshot({ path: resolve(screenshots, 'expenses-failure.png'), fullPage: true }).catch(() => {})
  throw error
} finally { if (browser) await browser.close(); await new Promise(done => server.close(done)) }
