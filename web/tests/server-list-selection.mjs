import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/server-list-selection.mjs`.
// Covers the server list's select-all checkbox, the contextual bulk action bar,
// threshold-coloured usage meters and the offline "last contact" hint.
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
const summary = []

try {
  for (const width of [1440, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 900 } })
    const errors = [], lifecycle = []
    const now = Math.floor(Date.now() / 1000)
    const servers = [['TEST_ONLY 高负载', true, 95], ['TEST_ONLY 正常', true, 40], ['TEST_ONLY 离线', false, 10]].map(([name, online, cpu], index) => ({
      id: index + 1, name, online, device_public_key: 'TEST_ONLY', metrics_stale: false, registered: true, manifest_rev: 0,
      last_seen: online ? now : now - 12 * 60, last_heartbeat_at: now, metrics_sampled_at: Date.now(),
      static_info: { memory_total: 8 * GiB, hostname: `fixture-${index + 1}` },
      latest_metrics: { cpu_percent: cpu, memory_used: (index === 0 ? 6.4 : 2) * GiB },
    }))
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname
      const json = (body, status = 200) => route.fulfill({ status, json: body })
      const transition = path.match(/^\/api\/servers\/(\d+)\/fleet\/lifecycle$/)
      if (transition && request.method() === 'POST') { lifecycle.push({ id: Number(transition[1]), ...request.postDataJSON() }); return json({}) }
      if (request.method() !== 'GET') { errors.push(`Unexpected ${request.method()} ${path}`); return json({ error: 'TEST_ONLY unexpected write' }, 400) }
      if (path === '/api/dashboard/access') return json({ authenticated: true, public_dashboard: false })
      if (path === '/api/control-center/me') return json({ id: 1, role: 'owner', display_name: 'TEST_ONLY 所有者', all_servers: true, capabilities: [], token_capabilities: null, token_servers: null })
      if (path.startsWith('/api/control-center/preferences/')) return json({ value: /recent|favorite/.test(path) ? [] : null, revision: 0, updated_at: 0 })
      if (path === '/api/servers') return json(servers)
      errors.push(`Unexpected GET ${path}`)
      return json({ error: 'TEST_ONLY 此夹具未提供该数据。' }, 404)
    })
    try {
      await page.goto(`${origin}/#/servers`)
      const row = name => page.getByRole('row').filter({ has: page.getByText(name, { exact: true }) })
      await row('TEST_ONLY 高负载').waitFor()
      const all = page.getByRole('checkbox', { name: '选择当前筛选的全部服务器' })
      const bar = page.getByRole('region', { name: '批量操作' })
      assert.equal(await bar.count(), 0, 'No bulk bar without a selection')

      // Usage meters carry accessible names and turn red from 90%.
      const hot = page.getByRole('progressbar', { name: 'TEST_ONLY 高负载 处理器使用率' })
      assert.equal(await hot.getAttribute('aria-valuenow'), '95')
      assert.match(await hot.locator('[data-slot=progress-indicator]').getAttribute('class'), /bg-destructive/)
      assert.match(await page.getByRole('progressbar', { name: 'TEST_ONLY 高负载 内存使用率' }).locator('[data-slot=progress-indicator]').getAttribute('class'), /bg-warning/)
      assert.doesNotMatch(await page.getByRole('progressbar', { name: 'TEST_ONLY 正常 处理器使用率' }).locator('[data-slot=progress-indicator]').getAttribute('class'), /bg-(warning|destructive)/)
      await row('TEST_ONLY 离线').getByText('最后联络 12 分钟前', { exact: true }).waitFor()

      // Select all, then partially deselect: the header shows a mixed state.
      await all.check()
      await bar.getByText('已选择 3 台', { exact: true }).waitFor()
      await row('TEST_ONLY 正常').getByRole('checkbox').uncheck()
      assert.equal(await all.getAttribute('aria-checked'), 'mixed')
      await bar.getByText('已选择 2 台', { exact: true }).waitFor()

      // Bulk actions apply to the selection only; clearing hides the bar again.
      await bar.getByRole('button', { name: '进入维护', exact: true }).click()
      const deadline = Date.now() + 5000
      while (lifecycle.length < 2 && Date.now() < deadline) await page.waitForTimeout(25)
      assert.deepEqual(lifecycle.map(item => [item.id, item.lifecycle]), [[1, 'maintenance'], [3, 'maintenance']])
      await bar.getByRole('button', { name: '清空选择', exact: true }).click()
      await bar.waitFor({ state: 'hidden' })
      assert.equal(await all.getAttribute('aria-checked'), 'false')
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `No horizontal scroll at ${width}px`)
      assert.deepEqual(errors, [])
      summary.push({ width, lifecycle: lifecycle.length })
    } finally { await page.close() }
  }
  console.log(JSON.stringify({ passed: true, scenarios: summary }, null, 2))
} finally {
  await browser.close()
  await new Promise(done => http.close(done))
}
