import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/motion-resilience.mjs`.
// Content must never depend on an animation finishing. With the page clock paused,
// requestAnimationFrame never advances, so any element that starts transparent
// would stay invisible; the sign-in form, page content and error notices must be
// readable anyway. The public dashboard loads through Suspense, whose reveal waits
// on timers, so its cards are covered by the motion preset unit test instead.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const root = fileURLToPath(new URL('../dist/', import.meta.url))
const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.webp': 'image/webp' }
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
const GiB = 1024 ** 3
const now = Date.now()
const servers = [1, 2].map(id => ({
  id, name: `TEST_ONLY 服务器 ${id}`, online: true, metrics_stale: false, device_public_key: 'TEST_ONLY', registered: true, manifest_rev: 0,
  metrics_sampled_at: now, last_seen: now / 1000, last_heartbeat_at: now / 1000,
  static_info: { system: 'Ubuntu 24.04', arch: 'amd64', cpu_cores: 2, memory_total: 4 * GiB, disk_total: 32 * GiB, hostname: `fixture-${id}` },
  latest_metrics: { cpu_percent: 20, memory_used: GiB, disk_used: 4 * GiB, uptime_secs: 3600, network_interfaces: {} },
  asset_settings: { region: 'JP', group_name: '亚洲', tags: [], hidden: false, price: '0', currency: 'CNY', billing_cycle: 30, expires_at: null, auto_renewal: false, traffic_limit: '0', traffic_limit_type: 'sum', reset_day: 1, network_interface: '' },
}))

// Product of every ancestor's opacity: what a reader actually sees.
const visibility = locator => locator.evaluate(element => {
  let opacity = 1
  for (let node = element; node; node = node.parentElement) opacity *= Number(getComputedStyle(node).opacity)
  const box = element.getBoundingClientRect()
  return { opacity, height: box.height }
})

const summary = []
try {
  for (const scenario of ['login', 'admin', 'admin-error']) {
    const page = await browser.newPage({ viewport: { width: 1280, height: 900 } })
    const errors = []
    page.on('pageerror', error => errors.push(error.message))
    await page.clock.install({ time: new Date('2026-10-03T00:00:00Z') })
    await page.clock.pauseAt(new Date('2026-10-03T00:00:00Z'))
    await page.route('**/api/**', route => {
      const path = new URL(route.request().url()).pathname
      const json = (body, status = 200) => route.fulfill({ status, json: body })
      if (path === '/api/dashboard/access') return json({ authenticated: scenario.startsWith('admin'), public_dashboard: false })
      if (path === '/api/control-center/me') return json({ id: 1, role: 'owner', display_name: 'TEST_ONLY 所有者', all_servers: true, capabilities: [], token_capabilities: null, token_servers: null })
      if (path.startsWith('/api/control-center/preferences/')) return json({ value: /recent|favorite/.test(path) ? [] : null, revision: 0, updated_at: 0 })
      if (path === '/api/servers') return scenario === 'admin-error' ? json({ error: 'TEST_ONLY 服务器列表暂不可用' }, 503) : json(servers)
      return json({ error: 'TEST_ONLY 此夹具未提供该数据。' }, 404)
    })
    try {
      await page.goto(`${origin}/#/servers`)
      let checked
      if (scenario === 'login') {
        checked = page.getByRole('heading', { name: '欢迎回来' })
      } else if (scenario === 'admin') {
        await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).waitFor()
        checked = page.locator('main')
      } else {
        checked = page.getByRole('alert').filter({ hasText: 'TEST_ONLY 服务器列表暂不可用' })
      }
      await checked.waitFor()
      // Real time passes, but the paused clock keeps every animation frame on hold.
      await page.waitForTimeout(500)
      const seen = await visibility(checked)
      assert.equal(seen.opacity, 1, `${scenario}: content is fully opaque without running animations`)
      assert.equal(seen.height > 0, true, `${scenario}: content takes up space without running animations`)
      assert.deepEqual(errors, [])
      summary.push({ scenario, ...seen })
    } finally { await page.close() }
  }
  console.log(JSON.stringify({ passed: true, scenarios: summary }, null, 2))
} finally {
  await browser.close()
  await new Promise(done => http.close(done))
}
