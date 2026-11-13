import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mkdir, readFile } from 'node:fs/promises'
import { extname, resolve, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const root = fileURLToPath(new URL('../dist/', import.meta.url))
const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(root, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(root.endsWith(sep) ? root : `${root}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': mime[extname(file)] ?? 'application/octet-stream' }).end(body) }
  catch { response.writeHead(404).end() }
})
await new Promise(ready => server.listen(0, '127.0.0.1', ready))
const origin = `http://127.0.0.1:${server.address().port}`
const browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
const screenshots = process.env.APSP_UI_SCREENSHOT_DIR
if (screenshots) await mkdir(screenshots, { recursive: true })

const now = Math.floor(Date.now() / 1000)
const probe = (carrier, connected) => ({ carrier, network: `TEST_ONLY ${carrier}`, city: 'TEST_ONLY', connected, avg_ms: connected ? 40 : null })
const china = connected => ({ probes: [probe('telecom', connected), probe('unicom', connected), probe('mobile', connected)], error: null })
const overseas = { probes: [probe('other', true), probe('other', true)], error: null }
const target = (key, port, state, extra = {}) => ({
  key, host: `${key}.example.invalid`, port, tcp: true, state, blocked: ['port_blocked', 'ip_blocked', 'unconfirmed'].includes(state),
  pending_state: null, failing_streak: 0, observed_state: state, resolved_ip: '192.0.2.1', detail: { china: china(state === 'reachable'), overseas },
  checked_at: now - 120, state_changed_at: now - 3600, deferred_at: null,
  nodes: [{ kind: 'nexus', id: 1, name: `TEST_ONLY ${key}`, server_id: 1, server_name: 'TEST_ONLY host', protocol: 'vless' }], risks: [], ...extra,
})
const targets = [
  target('open', 443, 'reachable'),
  target('blockedport', 2053, 'port_blocked', {
    detail: { china: china(false), overseas, control: { status: 'checked', port: 22, probes: china(true).probes } },
    risks: [{ code: 'port_not_443', value: '2053' }, { code: 'name_risky_brand', value: 'itunes.apple.com' }],
  }),
  target('blockedip', 443, 'ip_blocked', { detail: { china: china(false), overseas, control: { status: 'checked', port: 22, probes: china(false).probes } } }),
  target('udp', 21000, 'not_applicable', { tcp: false, checked_at: null, detail: {} }),
]

try {
  for (const width of [1440, 390]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 } })
    const page = await context.newPage(), errors = [], unexpected = [], external = [], writes = []
    page.on('pageerror', error => errors.push(error.message))
    let enabled = false
    const overview = () => ({
      settings: {
        enabled, interval_minutes: 30, minimum_interval_minutes: 15, maximum_interval_minutes: 1440, token_configured: false, revision: 1, next_run_at: now + 600, run_requested: false, running: false,
        last_run: { started_at: now - 130, finished_at: now - 120, manual: false, targets: 4, due: 3, measured: 3, deferred: 0, unresolved: 0, tests_used: 28, allowance: 200, limit: 250, remaining: 222, reset_secs: 1800, error: null },
      },
      targets, events: [],
    })
    await page.route('**/*', route => {
      const request = route.request(), url = new URL(request.url()), path = url.pathname, method = request.method()
      if (url.origin !== origin) { external.push(request.url()); return route.abort() }
      if (!path.startsWith('/api/')) return route.continue()
      const respond = (json, status = 200) => route.fulfill({ status, json })
      if (path === '/api/dashboard/access') return respond({ authenticated: true, public_dashboard: false })
      if (path === '/api/servers') return respond([{ id: 1, name: 'TEST_ONLY host', online: true }])
      if (path === '/api/observability/reachability' && method === 'GET') return respond(overview())
      // Expanding a row loads the server's probe history; this server has no probes.
      if ((path === '/api/probes/overview' || path === '/api/latency-tasks' || /^\/api\/servers\/\d+\/(probes|probe-results)$/.test(path)) && method === 'GET') return respond([])
      if (path === '/api/observability/reachability/settings' && method === 'PUT') {
        const body = request.postDataJSON(); writes.push(body); enabled = body.enabled; return respond(overview())
      }
      unexpected.push(`${method} ${path}`)
      return respond({ error: 'Unexpected fixture API' }, 500)
    })
    await installControlCenterFixtures(page)
    // The old address opens the quality overview, where reachability is a column of each server.
    await page.goto(`${origin}/#/latency/reachability`)
    await page.getByRole('heading', { name: '质量概况', exact: true }).waitFor()
    const line = page.locator('[data-slot=server-line]').filter({ has: page.getByRole('link', { name: 'TEST_ONLY host', exact: true }) })
    await line.getByRole('group', { name: '中国可达性' }).getByText('IP 被封锁', { exact: true }).waitFor({ timeout: 20000 })
    assert.match(await line.getByRole('group', { name: '中国可达性' }).innerText(), /4 个端点/)
    assert.match(await line.locator('[data-slot=line-problems]').innerText(), /中国可达性：IP 被封锁/)
    await line.getByRole('button', { name: 'TEST_ONLY host 详情' }).click()
    const endpoints = line.getByRole('list', { name: '端点可达性' })
    await endpoints.getByText('TEST_ONLY blockedport', { exact: false }).first().waitFor()
    const text = await endpoints.innerText()
    for (const expected of ['端口被封锁', 'IP 被封锁', '可达', '不适用']) assert.ok(text.includes(expected), `missing state label ${expected}`)
    assert.ok(text.includes('公开端口 2053 不是 443'), 'a non-443 port must be flagged')
    assert.ok(text.includes('itunes.apple.com'), 'a risky target must be flagged')
    assert.ok(text.indexOf('TEST_ONLY blockedip') < text.indexOf('TEST_ONLY open'), 'problems come first')
    // Name and address keep their own lines inside each endpoint.
    const name = endpoints.locator('[data-slot=reachability-endpoint]').filter({ hasText: 'TEST_ONLY blockedport' }).locator('div').first().locator('span')
    assert.ok((await name.nth(1).boundingBox()).y > (await name.nth(0).boundingBox()).y, 'address must sit below the node name')
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth)
    assert.ok(overflow <= 1, `page must not overflow horizontally at ${width}px, overflow ${overflow}`)
    // The settings tab holds the switch, interval and token; running needs it enabled.
    await page.goto(`${origin}/#/latency/settings`)
    const settings = page.getByRole('region', { name: '中国可达性' })
    await settings.getByRole('switch', { name: /^启用中国可达性检测/ }).waitFor()
    assert.equal(await settings.getByRole('button', { name: /立即检测/ }).isDisabled(), true)
    assert.match(await settings.innerText(), /检测 3 组/)
    await settings.getByRole('switch', { name: /^启用中国可达性检测/ }).check()
    await settings.getByRole('button', { name: '保存设置' }).click()
    await settings.getByRole('button', { name: /立即检测/ }).and(page.locator(':enabled')).waitFor()
    assert.deepEqual(writes.map(write => [write.enabled, write.interval_minutes, write.revision]), [[true, 30, 1]])
    assert.equal(Object.hasOwn(writes[0], 'api_token'), false, 'an empty token field keeps the saved token')
    if (screenshots) await page.screenshot({ path: `${screenshots}/china-reachability-${width}.png`, fullPage: true })
    assert.deepEqual(errors, [], 'page errors')
    assert.deepEqual(unexpected, [], 'unexpected API calls')
    assert.deepEqual(external, [], 'requests leaving the fixture origin')
    await context.close()
  }
  console.log('china reachability page: renders at 1440 and 390')
} finally {
  await browser.close()
  server.close()
}
