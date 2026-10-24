import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { extname, resolve, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import { catalogResourceFixtures, flatResourceFixtures, proxyResourceFixtures } from './proxy-resource-fixtures.mjs'

// Exercise the final bundle with isolated public DTOs, never a live migration.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = resolve(process.env.APSP_WEB_DIST ?? fileURLToPath(new URL('../dist/', import.meta.url)))
const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(`${dist}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': mime[extname(file)] ?? 'application/octet-stream' }).end(body) }
  catch { response.writeHead(404).end() }
})
const wait = async (condition, description) => {
  const deadline = Date.now() + 8000
  while (!await condition()) { assert(Date.now() < deadline, description); await new Promise(done => setTimeout(done, 25)) }
}
const prefix = '/api/plugins/sing-box'
let browser
try {
  await new Promise(done => server.listen(0, '127.0.0.1', done))
  const origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  for (const width of [1440, 320]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } })
    page.setDefaultTimeout(8000)
    const errors = [], unexpected = [], external = [], writes = []
    page.on('pageerror', error => errors.push(error.message))
    const hosts = [{ id: 1, name: '受管服务器', enabled: true, online: true, read_only: false, agent_supported: true }]
    const nodes = [{ id: 1, name: 'TEST_ONLY 受管入口', server_id: 1, protocol: 'vless-reality', enabled: true, port: 443,
      public_host: 'managed.example.invalid', sni: 'www.example.invalid', public_key: 'TEST_ONLY', short_id: 'abcd' }]
    // The imported ID deliberately overlaps a managed ID; the inventories stay separate.
    const imported = [{ id: 1, source_id: 'TEST_ONLY_archive', legacy_node_id: 31, legacy_machine_id: 7, legacy_host_id: 9,
      server_id: 20, server_name: 'TEST_ONLY 待接管主机', machine_name: '旧实例', machine_active: true,
      name: 'TEST_ONLY Nexus 入口', protocol: 'vless', public_host: 'imported.example.invalid', public_port: '21000-21020', listen_port: '8443',
      rate: '0', enabled: true, visible: false, sort_order: 0, tags: ['归档'], parent_id: null,
      groups: [{ id: 1, name: '测试组' }], routes: [], protocol_summary: { tls: 2, short_id: '0123456789abcdef' },
      custom_route_count: 1, custom_outbound_count: 1, timed_rate_enabled: false, timed_rate_count: 0,
      certificate_saved: false, configuration_saved: true, state: 'pending_agent', exported_at: 1, imported_at: 2 }]
    const flat = flatResourceFixtures(nodes, hosts), ordered = proxyResourceFixtures(nodes, hosts), catalog = catalogResourceFixtures(flat)
    let mode = 'valid', nexusReads = 0
    await page.route('**/*', async route => {
      const request = route.request(), url = new URL(request.url()), path = url.pathname
      if (url.origin !== origin) { external.push(url.href); return route.abort() }
      if (!path.startsWith('/api/')) return route.continue()
      if (request.method() !== 'GET') {
        writes.push(`${request.method()} ${path}`)
        return route.fulfill({ status: 405, json: { error: 'TEST_ONLY 此只读夹具不允许写入' } })
      }
      let value
      if (path === '/api/dashboard/access' || path === '/api/me') value = { authenticated: true, public_dashboard: false }
      else if (path === `${prefix}/nexus-nodes`) {
        nexusReads++
        if (mode === 'failed') return route.fulfill({ status: 503, json: { error: 'TEST_ONLY Nexus 库存读取失败' } })
        value = mode === 'invalid' ? [{ ...imported[0], state: 'active' }] : mode === 'empty' ? [] : imported
      } else if (path === `${prefix}/nodes`) value = nodes
      else if (path === `${prefix}/servers`) value = hosts
      else if (path === `${prefix}/proxy-resources`) value = flat
      else if (path === `${prefix}/ordered-proxy-resources`) value = ordered
      else if (path === `${prefix}/node-catalog`) value = catalog
      else if (path === `${prefix}/usage`) value = { total: '0', uplink: '0', downlink: '0', by_node: [], by_user: [] }
      else if (path === `${prefix}/source-migration`) value = { migrated: false, migrated_at: null }
      else if ([`${prefix}/subscription-sources`, `${prefix}/ordered-subscription-sources`].includes(path)) value = []
      else { unexpected.push(`GET ${path}`); return route.fulfill({ status: 404, json: { error: 'TEST_ONLY 未声明接口' } }) }
      return route.fulfill({ json: value })
    })
    // The default empty inventory must not intercept this suite's negative cases.
    await installControlCenterFixtures(page, { nexusNodes: false })
    // This final routing guard also covers the shared shell's fixture routes.
    await page.route('**/*', route => {
      if (new URL(route.request().url()).origin === origin) return route.fallback()
      external.push(route.request().url()); return route.abort()
    })
    try {
      await page.goto(`${origin}/#/plugins/sing-box/nodes`)
      const inventory = page.getByRole('region', { name: 'Nexus 节点', exact: true })
      const library = page.getByRole('region', { name: '节点库', exact: true })
      const importedRow = inventory.getByRole('row').filter({ has: page.getByText(imported[0].name, { exact: true }) })
      const managedRow = library.locator(width < 1200 ? '[data-slot=catalog-card]' : '[data-slot=catalog-table] tbody tr').filter({ hasText: nodes[0].name })
      await importedRow.waitFor(); await managedRow.waitFor()
      assert(nexusReads > 0, 'The node page loads imported inventory immediately')
      assert.equal(await page.locator('[data-slot=stat]').filter({ hasText: '代理资源' }).locator('strong').innerText(), '2')
      assert.equal(await page.locator('[data-slot=stat]').filter({ hasText: '所在服务器' }).locator('strong').innerText(), '2')
      assert.equal(await page.locator('[data-slot=stat]').filter({ hasText: '物理监听数' }).locator('strong').innerText(), '1')
      await importedRow.getByText('imported.example.invalid:21000-21020', { exact: true }).waitFor()
      await importedRow.getByText('订阅隐藏', { exact: true }).waitFor()
      assert.equal(await importedRow.getByRole('button').count(), 1, 'Imported inventory exposes only read-only details')
      const detail = importedRow.getByRole('button', { name: '详情', exact: true })
      await detail.focus(); await page.keyboard.press('Enter')
      const dialog = page.getByRole('dialog', { name: imported[0].name, exact: true })
      await dialog.getByText('0123456789abcdef', { exact: true }).waitFor()
      await dialog.getByText('测试组（#1）', { exact: true }).waitFor()
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `The details dialog fits at ${width}px`)
      await dialog.getByRole('button', { name: '关闭对话框', exact: true }).click()
      await dialog.waitFor({ state: 'hidden' })
      await wait(() => detail.evaluate(element => element === document.activeElement), 'Closing Nexus details restores its original button focus')

      const refresh = () => page.locator('header[data-slot=page-header]').getByRole('button', { name: '刷新', exact: true }).click()
      mode = 'failed'; await refresh()
      await inventory.getByRole('alert').filter({ hasText: 'TEST_ONLY Nexus 库存读取失败' }).waitFor()
      await importedRow.waitFor(); await managedRow.waitFor()
      mode = 'invalid'; await refresh()
      await inventory.getByRole('alert').filter({ hasText: '资源信息格式不完整' }).waitFor()
      await importedRow.waitFor(); await managedRow.waitFor()
      assert.equal(await inventory.getByText('imported.example.invalid:21000-21020', { exact: true }).count(), 1, 'Malformed inventory retains the last valid public data')
      mode = 'valid'; await inventory.getByRole('button', { name: '重试', exact: true }).click()
      await wait(async () => await inventory.getByRole('alert').count() === 0, 'A successful retry clears the inventory error')
      assert.equal(await page.locator('[data-slot=stat]').filter({ hasText: '代理资源' }).locator('strong').innerText(), '2')
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `The inventories stay within the page at ${width}px`)
      mode = 'empty'; await refresh()
      await inventory.waitFor({ state: 'hidden' }); await managedRow.waitFor()
      assert.equal(await page.locator('[data-slot=stat]').filter({ hasText: '代理资源' }).locator('strong').innerText(), '1')
      assert.deepEqual(writes, []); assert.deepEqual(external, []); assert.deepEqual(unexpected, []); assert.deepEqual(errors, [])
    } finally { await page.close() }
  }
  console.log('PASS: imported and managed inventories coexist, details restore focus, failed and invalid reads retain history, recovery and empty inventory, 320px page containment, no writes or external requests')
} finally {
  await browser?.close()
  await new Promise(done => server.close(done))
}
