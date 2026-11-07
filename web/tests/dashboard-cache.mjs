// Read-only browser audit against the locally built APSP page and isolated API fixtures.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { resolve, extname } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
import { installControlCenterFixtures } from './control-center-fixtures.mjs'

const numeric = { mem_used: 2 * 1024 ** 3, mem_total: 8 * 1024 ** 3, disk_used: 32 * 1024 ** 3, disk_total: 128 * 1024 ** 3, swap_used: 0, swap_total: 0, load1: .5, load5: .2, load15: .1, uptime: 3600, net_out: 0, net_in: 1024, net_tx_total: 2048, net_rx_total: 4096 }
const root = fileURLToPath(new URL('../dist', import.meta.url))
const host = createServer(async (req, res) => {
  const file = resolve(root, '.' + (new URL(req.url, 'http://localhost').pathname === '/' ? '/index.html' : new URL(req.url, 'http://localhost').pathname))
  if (!file.startsWith(root + '/')) return res.writeHead(403).end()
  try { const body = await readFile(file); res.writeHead(200, { 'Content-Type': ({ '.js': 'text/javascript', '.html': 'text/html', '.css': 'text/css', '.svg': 'image/svg+xml' })[extname(file)] ?? 'application/octet-stream' }).end(body) }
  catch { res.writeHead(404).end() }
})
await new Promise(done => host.listen(0, '127.0.0.1', done))
const origin = `http://127.0.0.1:${host.address().port}`
const browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
const results = []
const pause = () => new Promise(done => setTimeout(done, 120))
try {
  for (const width of [1440, 390, 320]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 } })
    const page = await context.newPage(), errors = [], writes = [], pending = []
    page.on('pageerror', e => errors.push(e.message))
    const anchor = Date.UTC(2026, 10, 7, 4)
    await page.clock.install({ time: anchor })
    let now = anchor, older = false, hold = false
    const metrics = cpu => ({ cpu_percent: cpu, memory_used: numeric.mem_used, memory_total: numeric.mem_total,
      disk_used: numeric.disk_used, disk_total: numeric.disk_total, swap_used: numeric.swap_used, swap_total: numeric.swap_total,
      load_1: numeric.load1, load_5: numeric.load5, load_15: numeric.load15, uptime_secs: numeric.uptime,
      network_interfaces: { eth0: { transmit_bytes_per_sec: numeric.net_out, receive_bytes_per_sec: numeric.net_in,
        transmitted_bytes: numeric.net_tx_total, received_bytes: numeric.net_rx_total } } })
    const server = { id: 1, name: '测试缓存节点', registered: true, public_view: false, device_public_key: 'TEST_ONLY',
      online: true, last_seen: anchor / 1000, last_heartbeat_at: anchor / 1000, metrics_stale: false,
      metrics_sampled_at: anchor - 60000, latest_metrics: metrics(10), served_at: anchor,
      static_info: { system: 'Linux', arch: 'amd64', cpu_cores: 4, memory_total: numeric.mem_total, disk_total: numeric.disk_total },
      agent_settings: { sample_interval_secs: 1, upload_interval_secs: 3 }, telemetry_settings: { persist_interval_secs: 60 },
      asset_settings: { price: '30', currency: 'CNY', billing_cycle: 30, expires_at: anchor / 1000 + 90 * 86400,
        region: 'JP', group_name: '测试', tags: [], hidden: false, traffic_limit: '0', traffic_limit_type: 'sum', reset_day: 1, network_interface: '', auto_renewal: false } }
    await page.route('**/*', async route => {
      const url = new URL(route.request().url())
      if (url.origin !== origin) return route.abort()
      if (!url.pathname.startsWith('/api/')) return route.continue()
      if (route.request().method() !== 'GET') { writes.push(url.pathname); return route.abort() }
      let data
      if (url.pathname === '/api/dashboard/access') data = { authenticated: true, public_dashboard: false }
      else if (url.pathname === '/api/me') data = {}
      else if (url.pathname === '/api/dashboard/servers') data = [server]
      else if (url.pathname === '/api/dashboard/live') data = { served_at: now, public_view: false, servers: [{ ...server, metrics_sampled_at: older ? anchor - 5000 : anchor, latest_metrics: metrics(older ? 11 : 73) }] }
      else if (url.pathname.endsWith('/exchange-rates')) data = { base: 'CNY', rates: { CNY: 1 }, rate_dates: {}, stale: false, status: 'fresh' }
      else data = []
      if (hold && ['/api/dashboard/live', '/api/dashboard/servers'].includes(url.pathname)) await new Promise(done => pending.push(done))
      return route.fulfill({ json: data }).catch(() => {})
    })
    await installControlCenterFixtures(page)
    const card = page.locator('[data-slot=server-card]').first()
    const cpu = () => card.locator('[data-slot=metric]').first().locator('strong').innerText()
    await page.goto(`${origin}/#/dashboard`)
    await card.waitFor()
    await page.waitForFunction(() => document.querySelector('[data-slot=server-card] [data-slot=metric] strong')?.textContent === '73.0%')
    const costs = page.locator('[data-slot=overview-item]').filter({ has: page.getByText('资产', { exact: true }) })
    assert.match(await costs.innerText(), /剩余价值\s*CNY\s*90\.00/)
    const layout = await card.evaluate(el => {
      const box = el.getBoundingClientRect(), header = el.querySelector('[data-slot=card-header]').getBoundingClientRect()
      return { tag: el.tagName, width: box.width, headerWidth: header.width, alignItems: getComputedStyle(el).alignItems,
        overflow: document.documentElement.scrollWidth > innerWidth + 1 }
    })
    assert.equal(layout.tag, 'A')
    assert(layout.headerWidth >= layout.width - 3)
    assert.equal(layout.overflow, false)

    // A short visibility change must preserve a valid reading.
    await page.evaluate(() => { Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'hidden' }); document.dispatchEvent(new Event('visibilitychange')) })
    now += 1000; await page.clock.runFor(1000)
    assert.equal(await cpu(), '73.0%')
    await page.evaluate(() => { Object.defineProperty(document, 'visibilityState', { configurable: true, value: 'visible' }); document.dispatchEvent(new Event('visibilitychange')) })
    await pause()

    // A late stored sample must remain reconciled after unmounting.
    older = true
    const received = page.waitForResponse(r => r.url().endsWith('/api/dashboard/live'))
    now += 3000; await page.clock.runFor(3000); await received; await pause()
    const beforeReturn = await cpu()
    assert.equal(beforeReturn, '73.0%')
    await page.evaluate(() => { location.hash = '/not-a-real-page' })
    await card.waitFor({ state: 'detached' })
    hold = true
    await page.evaluate(() => { location.hash = '/dashboard' })
    await card.waitFor(); await pause()
    const afterReturn = await cpu()
    assert.equal(afterReturn, beforeReturn, 'Returning before a fresh read keeps the reconciled sample')
    assert.deepEqual(errors, [])
    assert.deepEqual(writes, [])
    results.push({ width, prepaid: '30 CNY/30 days, 90 days remaining = 90 CNY', layout, shortVisibilityPreserved: true,
      beforeReturn, afterReturn, cacheRegression: afterReturn !== beforeReturn, errors, writes })
    hold = false; for (const done of pending) done()
    await context.close()
  }
  console.log(JSON.stringify(results, null, 2))
} finally { await browser.close(); await new Promise(done => host.close(done)) }
