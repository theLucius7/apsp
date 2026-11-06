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

const GiB = 1024 ** 3, DAY = 86400
const now = Math.floor(Date.UTC(2026, 10, 6, 4) / 1000)
const subscription = (id, name, fields = {}) => ({
  id, name, status: 'active', allowed: true, package_name: '标准 · 100 GB', monthly_bytes: String(100 * GiB), used_bytes: String(18 * GiB),
  starts_at: now - 90 * DAY, expires_at: now + 120 * DAY, cycle_start: now - 6 * DAY, next_reset: now + 24 * DAY,
  groups: ['原权限组 · 00'], bridged: true, last_fetch_at: now - 2 * 3600, last_fetch_format: 'renderer', last_fetch_result: 'relayed', fetches_today: 3, ...fields,
})
const board = {
  now, fetches_recorded: true, fetch_retention_days: 7,
  subscriptions: [
    subscription(1, '林间'),
    subscription(2, '晚风', { used_bytes: String(94 * GiB), last_fetch_format: 'clash', last_fetch_result: 'generated', bridged: false, groups: ['家人', '香港优选'] }),
    subscription(3, '山月', { used_bytes: String(52 * GiB), expires_at: now + 4 * DAY }),
    subscription(4, '海盐', { status: 'expired', allowed: false, expires_at: now - 3 * DAY, last_fetch_at: null, last_fetch_format: null, last_fetch_result: null, fetches_today: 0 }),
    subscription(5, '一个名字特别特别长的代理用户用来检查截断', { status: 'unmetered', package_name: null, monthly_bytes: null, used_bytes: String(321 * GiB), expires_at: null, cycle_start: null, next_reset: null, groups: [], bridged: false, last_fetch_format: 'singbox', last_fetch_result: 'generated' }),
    subscription(6, '青柠', { used_bytes: String(100 * GiB), status: 'exhausted', allowed: false, fetches_today: 0, last_fetch_at: now - 2 * DAY, last_fetch_result: 'restricted', last_fetch_format: 'clash' }),
  ],
}
const source = {
  id: 7, name: '备用机场', kind: 'url', host: 'sub.example.com', configured: true, auth_configured: false, settings_revision: 1, identity_epoch: 1, archived: false,
  refresh_interval_secs: 3600, user_agent: null, auto_refresh: true, traffic: { upload: 3 * GiB, download: 61 * GiB, total: 200 * GiB, expire: now + 20 * DAY, updated_at: now },
  changes: { added: 0, updated: 0, missing: 0, unsupported: 0 }, last_attempt_at: now, last_success_at: now - 1800, latest_success: null, active_job: null,
  last_error: null, stale_reason: null, counts: { supported: 18, unsupported: 2, ambiguous: 0, missing: 0 }, dependencies: [],
}
const failing = { ...source, id: 8, name: '旧机场', host: 'old.example.net', traffic: {}, last_success_at: null, last_error: { stage: 'fetch', kind: 'http', message: '来源未返回成功响应，请检查订阅是否有效', http_status: 403 }, counts: { supported: 0, unsupported: 0, ambiguous: 0, missing: 0 } }
const expense = (id, name, fields = {}) => ({
  id, name, amount_units: '200000', currency: 'USD', start_date: null, next_payment: '2026-11-09', anchor_payment: '2026-01-09', cycle: 'month', frequency: 1,
  category_id: 1, payment_method_id: 2, payer_id: null, notes: '', url: '', inactive: false, auto_renew: true, notify: true, notify_days_before: 3, cancellation_date: null, replacement_id: null, version: 1, ...fields,
})
const renewals = [
  expense(11, 'ChatGPT Plus'),
  expense(12, 'example.com 域名', { amount_units: '900000', currency: 'CNY', cycle: 'year', next_payment: '2027-03-02', anchor_payment: '2026-03-02', category_id: 3, auto_renew: false, notes: '在 Cloudflare 续费' }),
  expense(13, '云盘', { amount_units: '80000', currency: 'CNY', next_payment: '2026-11-01', anchor_payment: '2026-01-01', auto_renew: false, category_id: null }),
  expense(14, '旧 VPN 服务', { inactive: true }),
]
const labels = { entries: [{ id: 1, kind: 'category', name: 'AI 服务', enabled: true }, { id: 2, kind: 'payment_method', name: '信用卡', enabled: true }, { id: 3, kind: 'category', name: '域名', enabled: true }] }
const summary = {
  from: '2026-11-06', to: '2027-11-05', active: 3, inactive: 1, totals_units: {}, categories: [], servers: { count: 0, unpriced: 0, totals_units: {} }, upcoming: [],
  base_currency: 'CNY', budget_units: '5000000', converted_yearly: null, converted_yearly_units: null, conversion_stale: false, missing_currencies: [], estimate: true,
  exchange: { base: 'CNY', rates: { USD: 0.14 }, rate_dates: {}, rate_date: '2026-11-05', source: 'frankfurter', source_url: null, fetched_at: now, attempted_at: now, next_refresh_at: now, stale: false, status: 'fresh', error_code: null },
}
const cards = (page, kind) => page.locator(`[data-slot=subscription-card][data-kind=${kind}]`)

let page
const results = []
try {
  for (const [width, scheme] of [[1440, 'light'], [1440, 'dark'], [768, 'light'], [390, 'light'], [320, 'light']]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 }, colorScheme: scheme })
    page = await context.newPage()
    const errors = [], writes = [], unexpected = []
    page.on('pageerror', error => { errors.push(error.message); console.error('Page error:', error.message) })
    await page.clock.install({ time: now * 1000 })
    let reads = 0
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname
      if (request.method() !== 'GET') writes.push(path)
      if (path === '/api/dashboard/access') return route.fulfill({ json: { authenticated: true, public_dashboard: false } })
      if (path === '/api/plugins/sing-box/subscription-board') { reads++; return route.fulfill({ json: board }) }
      if (path === '/api/plugins/sing-box/source-migration') return route.fulfill({ json: { migrated: true, migrated_at: now - 30 * DAY } })
      if (path === '/api/plugins/sing-box/ordered-subscription-sources') return route.fulfill({ json: [source, failing, { ...source, id: 9, name: '已归档', archived: true }] })
      if (path === '/api/plugins/expenses/subscriptions') return route.fulfill({ json: renewals })
      if (path === '/api/plugins/expenses/labels') return route.fulfill({ json: labels })
      if (path === '/api/plugins/expenses/summary') return route.fulfill({ json: summary })
      // The pages opened from a card read their own data; only the address matters here.
      if (path.startsWith('/api/plugins/')) return route.fulfill({ json: [] })
      unexpected.push(path)
      return route.fulfill({ status: 404, json: { error: 'TEST_ONLY' } })
    })
    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/subscriptions`)
    await page.getByRole('region', { name: '订阅总览', exact: true }).waitFor()
    await cards(page, 'users').first().waitFor()
    await cards(page, 'renewals').first().waitFor()
    await page.locator('[data-slot=source-card]').first().waitFor()

    // It is a tab of the overview, beside the server board.
    const tabs = page.getByRole('navigation', { name: '本节页面', exact: true }).getByRole('link')
    assert.deepEqual((await tabs.allInnerTexts()).map(text => text.trim()), ['概览', '统计报表', '服务器看板', '订阅看板'])
    assert.equal(await tabs.last().getAttribute('aria-current'), 'page')
    assert.equal(await cards(page, 'users').count(), 6)
    assert.equal(await cards(page, 'renewals').count(), 4)
    assert.equal(await page.locator('[data-slot=source-card]').count(), 2, 'Archived sources stay off the board')
    const stat = async label => (await page.locator('[data-slot=overview-item]').filter({ has: page.locator('[data-slot=overview-label]').getByText(label, { exact: true }) }).locator('[data-slot=overview-value]').innerText()).replace(/\s+/g, ' ')
    assert.equal(await stat('可用订阅'), '4 / 6 个')
    assert.equal(await stat('需关注'), '5')
    assert.equal(await stat('24 小时拉取'), '4 / 6 个')
    assert.match(await stat('每月费用'), /158\.36/)
    const kinds = page.getByRole('group', { name: '订阅类型', exact: true }).getByRole('button')
    assert.deepEqual((await kinds.allInnerTexts()).map(text => text.replace(/\s+/g, '')), ['全部12', '代理用户6', '续费项目4', '订阅来源2'])

    // Needs-attention subscriptions come first, the soonest due first; stopped ones last.
    const names = await cards(page, 'users').locator('[data-slot=card-name]').allInnerTexts()
    assert.deepEqual(names.slice(0, 2), ['山月', '晚风'])
    assert.deepEqual(names.slice(-2), ['海盐', '青柠'])
    const user = cards(page, 'users').filter({ hasText: '晚风' })
    assert.equal(await user.getAttribute('href'), '#/plugins/sing-box/users/2')
    assert.match(await user.innerText(), /流量即将用完/)
    assert.equal(await page.locator('[data-slot=cycle-marker]').count(), 3, 'Stopped and unmetered subscriptions draw no cycle marker')
    assert.match(await cards(page, 'users').filter({ hasText: '海盐' }).innerText(), /无记录/)
    // Wallos-style renewal items: price, monthly cost in the ledger currency, next renewal.
    const renewalNames = await cards(page, 'renewals').locator('[data-slot=card-name]').allInnerTexts()
    assert.deepEqual(renewalNames, ['云盘', 'ChatGPT Plus', 'example.com 域名', '旧 VPN 服务'])
    const chat = cards(page, 'renewals').filter({ hasText: 'ChatGPT Plus' })
    assert.equal(await chat.getAttribute('href'), '#/plugins/expenses/items/11')
    const chatText = await chat.innerText()
    for (const phrase of ['USD 20', '即将续费', '3 天后', 'AI 服务', '信用卡', '自动']) assert(chatText.includes(phrase), `Renewal card shows ${phrase}`)
    assert.match(chatText, /142\.86/)
    assert.match(await cards(page, 'renewals').filter({ hasText: '云盘' }).innerText(), /待确认续费[\s\S]*已过 5 天/)
    assert.equal(await cards(page, 'renewals').filter({ hasText: '旧 VPN 服务' }).getAttribute('data-stopped'), 'true')
    assert.match(await page.locator('[data-slot=source-card]').filter({ hasText: '旧机场' }).innerText(), /来源未返回成功响应/)
    // Nothing secret is on the board.
    assert(!(await page.locator('body').innerText()).includes('/sub/'))
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), `No horizontal scroll at ${width}px`)
    if (screenshots) await page.screenshot({ path: resolve(screenshots, `subscription-board-${width}-${scheme}.png`), fullPage: true, animations: 'disabled' })

    if (width === 1440 && scheme === 'light') {
      // One kind at a time, then the status filter and search across kinds.
      await kinds.filter({ hasText: '续费项目' }).click()
      assert.equal(await cards(page, 'users').count(), 0)
      assert.equal(await cards(page, 'renewals').count(), 4)
      await kinds.filter({ hasText: '全部' }).click()
      await page.getByRole('button', { name: '筛选与视图', exact: true }).click()
      await page.getByRole('group', { name: '订阅状态筛选', exact: true }).getByRole('button', { name: '需关注', exact: true }).click()
      assert.equal(await cards(page, 'users').count(), 2)
      assert.equal(await cards(page, 'renewals').count(), 2)
      assert.equal(await page.locator('[data-slot=source-card]').count(), 1)
      await page.getByRole('group', { name: '订阅状态筛选', exact: true }).getByRole('button', { name: '全部', exact: true }).click()
      await page.getByLabel('搜索订阅', { exact: true }).fill('域名')
      assert.equal(await cards(page, 'renewals').count(), 1)
      assert.equal(await page.locator('section[data-kind=users]').count(), 0, 'Sections without a match step aside')
      await page.getByLabel('搜索订阅', { exact: true }).fill('')
      // The table view and the sort are remembered.
      await page.getByLabel('排序', { exact: true }).selectOption('expiry')
      await page.getByRole('button', { name: '表格', exact: true }).click()
      await page.locator('section[data-kind=users] tbody tr').first().waitFor()
      assert.equal(await page.locator('section[data-kind=users] tbody tr').count(), 6)
      assert.equal(await page.locator('section[data-kind=renewals] tbody tr').count(), 4)
      assert.equal(await page.locator('section[data-kind=sources] tbody tr').count(), 2)
      assert.match(await page.locator('section[data-kind=users] tbody tr').first().innerText(), /海盐/)
      if (screenshots) await page.screenshot({ path: resolve(screenshots, `subscription-board-table-${width}.png`), fullPage: true, animations: 'disabled' })
      await page.reload()
      await page.locator('section[data-kind=users] tbody tr').first().waitFor()
      await page.getByRole('button', { name: '筛选与视图', exact: true }).click()
      assert.equal(await page.getByLabel('排序', { exact: true }).inputValue(), 'expiry')
      await page.getByLabel('排序', { exact: true }).selectOption('attention')
      await page.getByRole('button', { name: '卡片', exact: true }).click()
      // Polling keeps the board current without a manual refresh.
      const before = reads
      await page.clock.fastForward(31_000)
      await new Promise(done => setTimeout(done, 300))
      assert(reads > before, 'The board polls')
      // A card opens that proxy user.
      await cards(page, 'users').filter({ hasText: '晚风' }).click()
      await page.waitForURL(/#\/plugins\/sing-box\/users\/2$/)
    }
    assert.deepEqual(writes, [], 'The board never writes')
    assert.deepEqual(errors, [])
    assert.deepEqual(unexpected.filter(path => !path.startsWith('/api/control-center/')), [], `Unexpected API reads: ${unexpected.join(', ')}`)
    results.push(`${width}px ${scheme}`)
    await context.close()
  }

  // An administrator who may read costs but not the proxy service sees the renewal items only, and nothing else is read.
  {
    const context = await browser.newContext({ viewport: { width: 1280, height: 900 } })
    page = await context.newPage()
    const errors = [], proxyReads = []
    page.on('pageerror', error => errors.push(error.message))
    await page.clock.install({ time: now * 1000 })
    await page.route('**/api/**', async route => {
      const path = new URL(route.request().url()).pathname
      if (path === '/api/dashboard/access') return route.fulfill({ json: { authenticated: true, public_dashboard: false } })
      if (path.startsWith('/api/plugins/sing-box/')) { proxyReads.push(path); return route.fulfill({ status: 403, json: { error: '没有权限' } }) }
      if (path === '/api/plugins/expenses/subscriptions') return route.fulfill({ json: renewals })
      if (path === '/api/plugins/expenses/labels') return route.fulfill({ json: labels })
      if (path === '/api/plugins/expenses/summary') return route.fulfill({ json: summary })
      return route.fulfill({ status: 404, json: { error: 'TEST_ONLY' } })
    })
    await installControlCenterFixtures(page)
    await page.route('**/api/control-center/me', route => route.fulfill({ json: { id: 2, role: 'operator', display_name: 'TEST_ONLY cost reader', all_servers: true, capabilities: ['cloud:read'], token_capabilities: null, token_servers: null } }))
    await page.goto(`${origin}/#/subscriptions`)
    await cards(page, 'renewals').first().waitFor()
    assert.equal(await cards(page, 'renewals').count(), 4)
    assert.equal(await cards(page, 'users').count(), 0)
    assert.equal(await page.getByRole('group', { name: '订阅类型', exact: true }).count(), 0, 'One kind needs no switch')
    assert.deepEqual(proxyReads, [], 'Proxy data is not requested without the proxy capability')
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
