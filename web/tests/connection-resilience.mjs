import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { extname, resolve, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/connection-resilience.mjs`.
// What a lost connection must and must not do (ADR 0102): a brief loss is invisible,
// a real one is reported once without moving the page, a hung read cannot freeze
// polling, and no connection problem ever looks like being signed out.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const root = fileURLToPath(new URL('../dist/', import.meta.url))
const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }
const http = createServer(async (request, response) => {
  const pathname = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(root, pathname === '/' ? 'index.html' : `.${pathname}`)
  if (!file.startsWith(root.endsWith(sep) ? root : `${root}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': mime[extname(file)] ?? 'application/octet-stream' }); response.end(body) }
  catch { response.writeHead(404).end() }
})
await new Promise(done => http.listen(0, '127.0.0.1', done))
const origin = `http://127.0.0.1:${http.address().port}`
const browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
const until = async (condition, message, limit = 10_000) => { const end = Date.now() + limit; while (!await condition()) { assert(Date.now() < end, message); await new Promise(done => setTimeout(done, 25)) } }
const server = id => ({ id, name: `TEST_ONLY 服务器 ${id}`, online: true, enabled: true, metrics_stale: false, device_public_key: 'TEST_ONLY', registered: true, manifest_rev: 1, last_seen: Date.now() / 1000, last_heartbeat_at: Date.now() / 1000, metrics_sampled_at: Date.now(), static_info: { memory_total: 1024 ** 3 }, latest_metrics: { cpu_percent: 10, memory_used: 1024 ** 2 } })

/** A signed-in panel; `state` decides how reads answer so each scenario can break the connection. */
async function open(state = {}, path = '/servers', contextOptions = {}) {
  const context = await browser.newContext({ viewport: { width: 1280, height: 800 }, colorScheme: 'light', ...contextOptions })
  const page = await context.newPage()
  const errors = [], reads = new Map()
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/api/**', async route => {
    const url = new URL(route.request().url()), api = url.pathname
    reads.set(api, (reads.get(api) ?? 0) + 1)
    if (state.down) return route.abort('connectionfailed')
    if (state.gateway) return route.fulfill({ status: 502, contentType: 'text/html', body: '<html>502 Bad Gateway</html>' })
    if (state.hold === api) { state.reached = (state.reached ?? 0) + 1; return }
    if (state.routes?.[api]) return state.routes[api](route)
    if (api === '/api/dashboard/access') return route.fulfill({ json: { authenticated: state.signedOut !== true, public_dashboard: state.publicDashboard === true } })
    if (api === '/api/servers' || api === '/api/dashboard/servers') return route.fulfill({ json: [server(1), server(2)] })
    if (api === '/api/dashboard/live') return route.fulfill({ json: { served_at: Date.now(), public_view: state.publicDashboard === true, servers: [server(1), server(2)].map(({ id, online, last_seen, last_heartbeat_at, metrics_stale, metrics_sampled_at, latest_metrics }) => ({ id, online, last_seen, last_heartbeat_at, metrics_stale, metrics_sampled_at, metrics_received_at: null, metrics_persisted_at: null, latest_metrics })) } })
    return route.fulfill({ status: 404, json: { error: 'TEST_ONLY 此夹具未提供该数据。' } })
  })
  await installControlCenterFixtures(page)
  await page.goto(`${origin}/#${path}`)
  return { context, page, errors, reads }
}
const summary = []

try {
  // One lost read: nothing on screen changes.
  {
    const state = {}
    const { context, page, errors, reads } = await open(state)
    await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).waitFor()
    await page.evaluate(() => {
      window.testAlerts = []
      new MutationObserver(records => { for (const record of records) for (const node of record.addedNodes) if (node instanceof Element && (node.matches('[data-slot=alert], [data-slot=connection-notice]') || node.querySelector('[data-slot=alert], [data-slot=connection-notice]'))) window.testAlerts.push(node.textContent) }).observe(document.body, { childList: true, subtree: true })
    })
    state.down = true
    const before = reads.get('/api/servers')
    await until(() => reads.get('/api/servers') > before, 'The next poll runs into the outage')
    state.down = false
    await until(async () => reads.get('/api/dashboard/access') > 1, 'A quiet probe confirms the panel is reachable')
    await page.waitForTimeout(600)
    assert.deepEqual(await page.evaluate(() => window.testAlerts), [], 'A brief loss is invisible')
    assert.equal(await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).count(), 1)
    assert.deepEqual(errors, [])
    summary.push('brief-loss')
    await context.close()
  }

  // A real outage is said once, over the page; recovery says so and refreshes.
  {
    const state = {}
    const { context, page, errors, reads } = await open(state)
    await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).waitFor()
    const heading = await page.getByRole('heading', { name: '服务器列表', exact: true }).boundingBox()
    state.down = true
    const notice = page.locator('[data-slot=connection-notice]')
    await notice.filter({ hasText: '连接中断' }).waitFor({ timeout: 15_000 })
    assert.equal(await page.locator('[data-slot=alert]').count(), 0, 'The notice replaces an alert in every section')
    assert.equal(await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).count(), 1, 'What was read stays on screen')
    assert.deepEqual(await page.getByRole('heading', { name: '服务器列表', exact: true }).boundingBox(), heading, 'The notice moves nothing')
    const before = reads.get('/api/servers')
    state.down = false
    await page.locator('[data-slot=connection-notice][data-state=recovered]').waitFor({ timeout: 20_000 })
    await until(() => reads.get('/api/servers') > before, 'Pages read again as soon as the panel is back', 2000)
    await notice.waitFor({ state: 'hidden', timeout: 5000 })
    assert.deepEqual(errors, [])
    summary.push('outage')
    await context.close()
  }

  // A read that never answers ends by timeout, and polling continues.
  {
    const state = {}
    const { context, page, errors, reads } = await open(state)
    await page.clock.install()
    await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).waitFor()
    state.hold = '/api/servers'
    await page.clock.runFor(5100)
    await until(() => state.reached > 0, 'The poll hangs')
    state.hold = undefined
    const before = reads.get('/api/servers')
    await page.clock.runFor(26_000)
    await until(() => reads.get('/api/servers') > before, 'Polling resumes after the hung read times out')
    assert.deepEqual(errors, [])
    summary.push('hung-read')
    await context.close()
  }

  // A gateway error at startup is not a sign-out, and recovery goes straight in.
  {
    const state = { gateway: true }
    const { context, page, errors } = await open(state)
    await page.getByText('暂时连不上面板', { exact: true }).waitFor()
    assert.equal(await page.locator('input[type=password]').count(), 0, 'A connection problem never shows the sign-in form')
    state.gateway = false
    await page.getByRole('button', { name: '立即重试', exact: true }).click()
    await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
    assert.equal(await page.locator('input[type=password]').count(), 0)
    assert.deepEqual(errors, [])
    summary.push('boot-gateway')
    await context.close()
  }

  // The first paint of a reload already has the saved theme and the full navigation.
  {
    const { context, page, errors } = await open()
    await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).waitFor()
    await page.evaluate(() => localStorage.setItem('apsp-appearance', JSON.stringify({ theme: 'dark', density: 'comfortable' })))
    await page.route('**/api/control-center/preferences/appearance', route => route.fulfill({ json: { value: { theme: 'dark', density: 'comfortable' }, revision: 1, updated_at: 0 } }))
    await page.addInitScript(() => {
      window.testFirst = { themes: [], links: [] }
      const sample = () => {
        const theme = document.documentElement?.dataset.theme
        if (theme && window.testFirst.themes.at(-1) !== theme) window.testFirst.themes.push(theme)
        const links = document.querySelectorAll('[data-slot=sidebar] nav a').length
        if (links && window.testFirst.links.at(-1) !== links) window.testFirst.links.push(links)
        requestAnimationFrame(sample)
      }
      sample()
    })
    await page.reload()
    await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).waitFor()
    await page.waitForTimeout(300)
    const first = await page.evaluate(() => window.testFirst)
    assert.deepEqual(first.themes, ['dark'], 'No frame shows the wrong theme')
    assert.equal(first.links.length, 1, `The navigation never grows after a reload: ${first.links}`)
    assert.deepEqual(errors, [])
    summary.push('first-paint')
    await context.close()
  }

  // A page that fails to render stays a failed page: the console and other pages work.
  {
    // TEST_ONLY: the page's own code throws while rendering, whatever data it reads. The backup
    // page is loaded on first visit only (not preloaded), so its module is always this one.
    const { context, page } = await open({}, '/servers')
    await page.route('**/assets/Backup-*.js', route => route.fulfill({ contentType: 'text/javascript', body: 'export default function TestBrokenPage() { throw new Error("TEST_ONLY render failure") }' }))
    await page.evaluate(() => { location.hash = '#/settings/backup' })
    await page.locator('[data-slot=page-failure]').waitFor()
    assert.equal(await page.locator('[data-slot=sidebar]').count(), 1, 'The console survives')
    await page.getByRole('navigation', { name: '主导航' }).getByRole('link', { name: '服务器列表', exact: true }).click()
    await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
    assert.equal(await page.locator('[data-slot=page-failure]').count(), 0, 'Going elsewhere clears the failure')
    summary.push('page-boundary')
    await context.close()
  }

  // Returning to a page shows what it read last instead of a skeleton.
  {
    const state = {}
    const { context, page } = await open(state)
    await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).waitFor()
    await page.evaluate(() => { location.hash = '#/settings/account' })
    await page.getByRole('heading', { name: '账号安全', exact: true }).waitFor()
    await page.route('**/api/servers', async route => { await new Promise(done => setTimeout(done, 500)); return route.fallback() })
    await page.evaluate(() => { location.hash = '#/servers' })
    await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
    assert.equal(await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).count(), 1, 'The last reading paints at once')
    assert.equal(await page.locator('main [data-slot=loading]').count(), 0)
    summary.push('return-to-page')
    await context.close()
  }

  // Signed out, the board stands alone with its own theme switch; signed in, it does not.
  {
    const state = { signedOut: true, publicDashboard: true, routes: { '/api/dashboard/exchange-rates': route => route.fulfill({ json: { base: 'CNY', rates: { CNY: 1 }, rate_dates: {}, rate_date: null, source: null, source_url: null, fetched_at: null, attempted_at: null, next_refresh_at: 0, stale: true, status: 'unavailable' } }), '/api/dashboard/probes/overview': route => route.fulfill({ json: [] }) } }
    const { context, page, errors } = await open(state, '/dashboard')
    await page.locator('[data-slot=server-card]').first().waitFor()
    assert.equal(await page.locator('[data-slot=sidebar]').count(), 0)
    await page.getByRole('button', { name: '切换深色主题', exact: true }).click()
    await until(async () => await page.evaluate(() => document.documentElement.dataset.theme) === 'dark', 'The public board switches theme')
    assert.deepEqual(errors, [])
    summary.push('public-board')
    await context.close()
  }

  // Copying a subscription address asks for proof once and needs no dialog of its own.
  {
    let proven = false
    const users = [{ id: 7, name: 'TEST_ONLY 用户', subscription_token: '', subscription_url: '', credentials_redacted: true }]
    const state = { routes: {
      '/api/plugins/sing-box/users': route => route.fulfill({ json: users }),
      '/api/plugins/sing-box/usage': route => route.fulfill({ json: { uplink: '0', downlink: '0', total: '0', by_user: [], by_node: [] } }),
      '/api/plugins/sing-box/policy-groups': route => route.fulfill({ json: [] }),
      '/api/plugins/sing-box/package-groups': route => route.fulfill({ json: [] }),
      '/api/plugins/sing-box/users/7/policy-groups': route => route.fulfill({ json: { group_ids: [] } }),
      '/api/plugins/sing-box/users/7/entitlement': route => route.fulfill({ json: { user_id: 7, package_group_id: null, package_name: null, monthly_bytes: null, reset_day: null, reset_hour: null, reset_minute: null, timezone: null, starts_at: null, expires_at: null, cycle_start: null, next_reset: null, used_bytes: '0', status: 'unmetered', allowed: true } }),
      '/api/plugins/sing-box/users/7/portal': route => route.fulfill({ json: { configuration: { enabled: true, origin: origin, reason: null }, keys: 0, url: null, activation_expires_at: null } }),
      '/api/plugins/sing-box/users/7/subscription': route => proven ? route.fulfill({ json: { format: 'singbox', subscription_url: 'https://apsp-TEST.example.test/TEST_ONLY_PATH' } }) : route.fulfill({ status: 403, json: { error: '请先再次验证管理员身份，证明有效期为五分钟' } }),
      '/api/control-center/reauth': route => { proven = true; return route.fulfill({ status: 204 }) },
    } }
    const { context, page, reads } = await open(state, '/users', { permissions: ['clipboard-read', 'clipboard-write'] })
    // The selected user's access card holds the one copy button.
    await page.locator('[data-slot=user-access]').getByRole('button', { name: '复制订阅链接', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: '再次验证身份' })
    await dialog.getByLabel('当前管理员密码').fill('TEST_ONLY_PASSWORD')
    await dialog.getByRole('button', { name: '验证并继续', exact: true }).click()
    await page.locator('[data-slot=subscription-address]').filter({ hasText: 'https://apsp-TEST.example.test/TEST_ONLY_PATH' }).waitFor()
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), 'https://apsp-TEST.example.test/TEST_ONLY_PATH')
    // A second copy asks nothing and reads nothing more.
    const before = reads.get('/api/plugins/sing-box/users/7/subscription')
    await page.getByRole('button', { name: /复制订阅链接|已复制/ }).click()
    await page.waitForTimeout(100)
    assert.equal(reads.get('/api/plugins/sing-box/users/7/subscription'), before)
    assert.equal(await dialog.count(), 0)
    summary.push('copy-subscription')
    await context.close()
  }

  console.log(JSON.stringify(summary))
} finally {
  await browser.close()
  await new Promise(done => http.close(done))
}
