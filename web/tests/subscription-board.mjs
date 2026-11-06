import { installControlCenterFixtures } from './control-center-fixtures.mjs'
// Built-page regression for the subscription board with isolated API fixtures; never connects to a deployed panel.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile, mkdir } from 'node:fs/promises'
import { extname, resolve, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')

const root = fileURLToPath(new URL('../dist/', import.meta.url))
const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.webp': 'image/webp', '.png': 'image/png', '.txt': 'text/plain' }
const host = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(root, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(root.endsWith(sep) ? root : `${root}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': types[extname(file)] || 'application/octet-stream' }).end(body) }
  catch { response.writeHead(404).end() }
})
await new Promise(done => host.listen(0, '127.0.0.1', done))
const origin = `http://127.0.0.1:${host.address().port}`
const browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
const screenshots = process.env.APSP_UI_SCREENSHOT_DIR
if (screenshots) await mkdir(screenshots, { recursive: true })

const now = Math.floor(Date.UTC(2026, 10, 6, 4) / 1000)
const expense = (id, name, fields = {}) => ({
  id, name, amount_units: '200000', currency: 'USD', start_date: null, next_payment: '2026-11-09', anchor_payment: '2026-01-09', cycle: 'month', frequency: 1,
  category_id: 1, payment_method_id: 2, payer_id: null, notes: '', url: '', inactive: false, auto_renew: true, notify: true, notify_days_before: 3, cancellation_date: null, replacement_id: null, version: 1, ...fields,
})
const subscriptions = [
  expense(11, 'ChatGPT Plus'),
  expense(12, 'example.com 域名', { amount_units: '900000', currency: 'CNY', cycle: 'year', next_payment: '2027-03-02', anchor_payment: '2026-03-02', category_id: 3, auto_renew: false, notes: '在 Cloudflare 续费' }),
  expense(13, '云盘', { amount_units: '80000', currency: 'CNY', next_payment: '2026-11-01', anchor_payment: '2026-01-01', auto_renew: false, category_id: null }),
  expense(14, '旧 VPN 服务', { inactive: true }),
  expense(15, 'Claude Pro', { next_payment: '2026-11-25', anchor_payment: '2026-01-25' }),
]
const labels = { entries: [{ id: 1, kind: 'category', name: 'AI 服务', enabled: true }, { id: 2, kind: 'payment_method', name: '信用卡', enabled: true }, { id: 3, kind: 'category', name: '域名', enabled: true }] }
const summary = {
  from: '2026-11-06', to: '2027-11-05', active: 4, inactive: 1, totals_units: {}, categories: [], servers: { count: 0, unpriced: 0, totals_units: {} }, upcoming: [],
  base_currency: 'CNY', budget_units: '5000000', converted_yearly: null, converted_yearly_units: null, conversion_stale: false, missing_currencies: [], estimate: true,
  exchange: { base: 'CNY', rates: { USD: 0.14 }, rate_dates: {}, rate_date: '2026-11-05', source: 'frankfurter', source_url: null, fetched_at: now, attempted_at: now, next_refresh_at: now, stale: false, status: 'fresh', error_code: null },
}
const cards = page => page.locator('[data-slot=subscription-card]')

let page
const results = []
try {
  for (const [width, scheme] of [[1440, 'light'], [1440, 'dark'], [768, 'light'], [390, 'light'], [320, 'light']]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 }, colorScheme: scheme })
    page = await context.newPage()
    const errors = [], writes = [], unexpected = [], proxyReads = []
    page.on('pageerror', error => { errors.push(error.message); console.error('Page error:', error.message) })
    await page.clock.install({ time: now * 1000 })
    let reads = 0
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname
      if (request.method() !== 'GET') writes.push(path)
      if (path === '/api/dashboard/access') return route.fulfill({ json: { authenticated: true, public_dashboard: false } })
      if (path === '/api/plugins/expenses/subscriptions') { reads++; return route.fulfill({ json: subscriptions }) }
      if (path === '/api/plugins/expenses/labels') return route.fulfill({ json: labels })
      if (path === '/api/plugins/expenses/summary') return route.fulfill({ json: summary })
      // Other people's subscriptions are never read here.
      if (path.startsWith('/api/plugins/sing-box/') && path !== '/api/plugins/sing-box/users/overview') proxyReads.push(path)
      if (path.startsWith('/api/plugins/')) return route.fulfill({ json: [] })
      unexpected.push(path)
      return route.fulfill({ status: 404, json: { error: 'TEST_ONLY' } })
    })
    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/subscriptions`)
    await page.getByRole('region', { name: '订阅总览', exact: true }).waitFor()
    await cards(page).first().waitFor()

    // It is a tab of the overview, beside the server board.
    const tabs = page.getByRole('navigation', { name: '本节页面', exact: true }).getByRole('link')
    assert.deepEqual((await tabs.allInnerTexts()).map(text => text.trim()), ['概览', '统计报表', '服务器看板', '订阅看板'])
    assert.equal(await tabs.last().getAttribute('aria-current'), 'page')
    const stat = async label => (await page.locator('[data-slot=overview-item]').filter({ has: page.locator('[data-slot=overview-label]').getByText(label, { exact: true }) }).locator('[data-slot=overview-value]').innerText()).replace(/\s+/g, ' ')
    assert.equal(await stat('使用中'), '4 / 5 个')
    assert.equal(await stat('需关注'), '2')
    assert.match(await stat('每月费用'), /301\.21/)
    assert.match(await stat('30 天内续费'), /285\.71/)

    // Only the administrator's own subscriptions: the soonest renewal first, stopped last.
    assert.equal(await cards(page).count(), 5)
    assert.deepEqual(await cards(page).locator('[data-slot=card-name]').allInnerTexts(), ['云盘', 'ChatGPT Plus', 'Claude Pro', 'example.com 域名', '旧 VPN 服务'])
    const chat = cards(page).filter({ hasText: 'ChatGPT Plus' })
    assert.equal(await chat.getAttribute('href'), '#/expenses/items/11')
    const chatText = await chat.innerText()
    for (const phrase of ['USD 20', '即将续费', '3 天后', 'AI 服务', '信用卡', '自动']) assert(chatText.includes(phrase), `The card shows ${phrase}`)
    assert.match(chatText, /142\.86/)
    assert.match(await cards(page).filter({ hasText: '云盘' }).innerText(), /待确认续费[\s\S]*已过 5 天/)
    assert.equal(await cards(page).filter({ hasText: '旧 VPN 服务' }).getAttribute('data-stopped'), 'true')
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `No horizontal scroll at ${width}px`)
    if (screenshots) await page.screenshot({ path: resolve(screenshots, `subscription-board-${width}-${scheme}.png`), fullPage: true, animations: 'disabled' })

    if (width === 1440 && scheme === 'light') {
      const status = page.getByRole('group', { name: '订阅状态筛选', exact: true })
      await status.getByRole('button', { name: /^需关注/ }).click()
      assert.equal(await cards(page).count(), 2)
      await status.getByRole('button', { name: /^已停用/ }).click()
      assert.equal(await cards(page).count(), 1)
      await status.getByRole('button', { name: /^全部/ }).click()
      await page.getByLabel('搜索订阅', { exact: true }).fill('cloudflare')
      assert.equal(await cards(page).count(), 1)
      await page.getByLabel('搜索订阅', { exact: true }).fill('')
      // The category, the sort and the table view; the last two are remembered.
      await page.getByRole('button', { name: '筛选与视图', exact: true }).click()
      await page.getByLabel('分类', { exact: true }).selectOption({ label: '域名' })
      assert.equal(await cards(page).count(), 1)
      await page.getByLabel('分类', { exact: true }).selectOption('')
      await page.getByLabel('排序', { exact: true }).selectOption('cost')
      await page.getByRole('button', { name: '表格', exact: true }).click()
      await page.locator('tbody tr').first().waitFor()
      assert.equal(await page.locator('tbody tr').count(), 5)
      assert.match(await page.locator('tbody tr').first().innerText(), /ChatGPT Plus|Claude Pro/)
      if (screenshots) await page.screenshot({ path: resolve(screenshots, `subscription-board-table-${width}.png`), fullPage: true, animations: 'disabled' })
      await page.reload()
      await page.locator('tbody tr').first().waitFor()
      await page.getByRole('button', { name: '筛选与视图', exact: true }).click()
      assert.equal(await page.getByLabel('排序', { exact: true }).inputValue(), 'cost')
      await page.getByLabel('排序', { exact: true }).selectOption('next')
      await page.getByRole('button', { name: '卡片', exact: true }).click()
      // Polling keeps the board current without a manual refresh.
      const before = reads
      await page.clock.fastForward(61_000)
      await new Promise(done => setTimeout(done, 300))
      assert(reads > before, 'The board polls')
      // A card opens that subscription in the ledger.
      await cards(page).filter({ hasText: 'ChatGPT Plus' }).click()
      await page.waitForURL(/#\/expenses\/items\/11$/)
    }
    assert.deepEqual(proxyReads, [], 'No proxy user data is read for the board')
    assert.deepEqual(writes, [], 'The board never writes')
    assert.deepEqual(errors, [])
    assert.deepEqual(unexpected.filter(path => !path.startsWith('/api/control-center/')), [], `Unexpected API reads: ${unexpected.join(', ')}`)
    results.push(`${width}px ${scheme}`)
    await context.close()
  }

  // An administrator who may only read costs gets the same board.
  {
    const context = await browser.newContext({ viewport: { width: 1280, height: 900 } })
    page = await context.newPage()
    const errors = []
    page.on('pageerror', error => errors.push(error.message))
    await page.clock.install({ time: now * 1000 })
    await page.route('**/api/**', async route => {
      const path = new URL(route.request().url()).pathname
      if (path === '/api/dashboard/access') return route.fulfill({ json: { authenticated: true, public_dashboard: false } })
      if (path === '/api/plugins/expenses/subscriptions') return route.fulfill({ json: subscriptions })
      if (path === '/api/plugins/expenses/labels') return route.fulfill({ json: labels })
      if (path === '/api/plugins/expenses/summary') return route.fulfill({ json: summary })
      return route.fulfill({ status: 403, json: { error: '没有权限' } })
    })
    await installControlCenterFixtures(page)
    await page.route('**/api/control-center/me', route => route.fulfill({ json: { id: 2, role: 'operator', display_name: 'TEST_ONLY cost reader', all_servers: true, capabilities: ['cloud:read'], token_capabilities: null, token_servers: null } }))
    await page.goto(`${origin}/#/subscriptions`)
    await cards(page).first().waitFor()
    assert.equal(await cards(page).count(), 5)
    assert.deepEqual(errors, [])
    results.push('cost reader')
    await context.close()
  }
  console.log(`subscription board passed: ${results.join(', ')}`)
} catch (error) {
  if (screenshots && page) await page.screenshot({ path: resolve(screenshots, 'subscription-board-failure.png'), fullPage: true }).catch(() => {})
  throw error
} finally {
  await browser.close()
  host.close()
}
