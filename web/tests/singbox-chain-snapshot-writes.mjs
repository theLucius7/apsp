import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import { catalogResourceFixtures } from './proxy-resource-fixtures.mjs'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { mkdir, readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { extname, resolve, sep } from 'node:path'

// TEST_ONLY: owned loopback fixtures exercise actual built UI; no provider or production writes.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url)), trafficApi = '/api/traffic', accessApi = '/api/access'
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname, file = resolve(dist, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) return response.writeHead(400).end()
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' })[extname(file)] ?? 'application/octet-stream' }).end(body) }
  catch { response.writeHead(404).end() }
})
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
const origin = `http://127.0.0.1:${server.address().port}`
const totals = { scenarios: 0, blocked: 0, recovered: 0, requests: 0, external: [], unexpected: [], errors: [] }
const browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
const wait = async (condition, description) => { const deadline = Date.now() + 10000; while (!await condition()) { assert(Date.now() < deadline, description); await new Promise(resolve => setTimeout(resolve, 20)) } }
const refresh = page => page.locator('[data-slot=page-header] button').filter({ hasText: /^刷新$/ }).evaluate(button => button.click())
const refreshReads = async page => {
  const responses = ['nodes', 'ordered-proxy-resources', 'servers'].map(name => page.waitForResponse(response => new URL(response.url()).pathname === `${trafficApi}/${name}` && response.request().method() === 'GET'))
  await refresh(page); await Promise.all(responses)
}
const forceForm = (form, reload = false) => form.evaluate((element, reload) => {
  if (reload) Array.from(document.querySelectorAll('[data-slot=page-header] button')).find(button => button.textContent.trim() === '刷新').click()
  element.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }))
}, reload)
const forceClick = (button, reload = false) => button.evaluate((element, reload) => {
  if (reload) Array.from(document.querySelectorAll('[data-slot=page-header] button')).find(button => button.textContent.trim() === '刷新').click()
  const disabled = element.disabled; try { element.disabled = false; element.click() } finally { element.disabled = disabled }
}, reload)
try {
  for (const width of [1440, 390]) {
    const fixture = async (routeHash, callback) => {
      const page = await browser.newPage({ viewport: { width, height: 1000 } }); page.setDefaultTimeout(10000)
      page.on('pageerror', error => totals.errors.push(error.message))
      const hosts = [1, 2, 3].map(id => ({ id, name: `TEST_ONLY 服务器 ${id}`, enabled: true, online: true, agent_supported: true }))
      const nodes = [1, 2, 3].map(id => ({ id, name: `TEST_ONLY 节点 ${id}`, server_id: id, protocol: 'vless-reality', port: 20000 + id, public_host: '127.0.0.1', sni: 'localhost', enabled: true }))
      const direct = node => ({ ...node, kind: 'direct', server_name: `TEST_ONLY 服务器 ${node.server_id}`, available: true, role: 'direct', entry_node_id: null, tcp: true, udp: true, legacy: false, active_generation: null, pending_generation: null, minimum_generation: 0, stage: 'direct', last_error: null, reference_count: 0, entry_eligible: true })
      const resources = nodes.map(direct)
      const failures = new Set(), gates = new Map(), pending = [], writes = []
      const control = { page, hosts, nodes, resources, writes,
        hold(path) { let release; const promise = new Promise(resolve => { release = resolve }); const gate = { promise, release, reached: 0 }; gates.set(path, gate); return gate },
        fail(path) { failures.add(path); gates.get(path)?.release(); gates.delete(path) },
        recover(path) { failures.delete(path); gates.get(path)?.release(); gates.delete(path) },
      }
      await page.route('**/*', route => {
        const task = (async () => {
          const request = route.request(), url = new URL(request.url()), path = url.pathname, method = request.method()
          if (url.origin !== origin) { totals.external.push(url.href); return route.abort() }
          if (!path.startsWith('/api/')) return route.continue()
          ++totals.requests
          if (method === 'GET') {
            const gate = gates.get(path); if (gate) { ++gate.reached; await gate.promise }
            if (failures.has(path)) return route.fulfill({ status: 503, json: { error: 'TEST_ONLY 链路相关读取失败' } })
          }
          let value
          if (method !== 'GET') { totals.unexpected.push(`${method} ${path}`); return route.fulfill({ status: 404, json: {} }) }
          else if (['/api/me', '/api/dashboard/access'].includes(path)) value = { authenticated: true, public_dashboard: false }
          else if (path === `${trafficApi}/servers`) value = hosts
          else if (path === `${trafficApi}/nodes`) value = nodes
          else if ([`${trafficApi}/ordered-proxy-resources`, `${trafficApi}/ordered-subscription-sources`].includes(path)) value = []
      else if (path === `${trafficApi}/node-catalog`) value = catalogResourceFixtures(resources)
          else if (path === `${accessApi}/usage`) value = { total: '0', uplink: '0', downlink: '0', by_node: [], by_user: [] }
          else { totals.unexpected.push(`${method} ${path}`); return route.fulfill({ status: 404, json: {} }) }
          return route.fulfill({ json: value })
        })()
        pending.push(task); return task
      })
      try {
        await installControlCenterFixtures(page)
        await page.goto(`${origin}/#${routeHash}`)
        await wait(() => page.locator('header[data-slot=page-header]').getByRole('button', { name: '创建链路', exact: true }).isEnabled(), 'Initial chain snapshots')
        await callback(control)
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
        ++totals.scenarios
      } finally { for (const gate of gates.values()) gate.release(); await Promise.all(pending); await page.close() }
    }
    // Chain deletion guards live with the chain list (singbox-chains-write-guards.mjs, proxy-resources.mjs);
    // this file keeps the node creation guard against a server filter that no longer exists.
    for (const label of ['创建节点']) await fixture('/nodes', async control => {
      const button = control.page.getByRole('button', { name: label, exact: true }).first()
      await button.evaluate(button => {
        const filter = document.querySelector('select[aria-label="按服务器筛选"]')
        filter.add(new Option('TEST_ONLY 不存在的服务器', '999')); filter.value = '999'
        filter.dispatchEvent(new Event('change', { bubbles: true }))
        button.disabled = false; button.click()
      })
      assert.equal(await control.page.getByRole('dialog').count(), 0)
      assert.equal(await control.page.getByRole('region', { name: '创建链路', exact: true }).count(), 0)
      assert.equal(control.writes.length, 0); ++totals.blocked
      await control.page.getByLabel('按服务器筛选').selectOption('2'); await button.click()
      const selected = control.page.getByRole('dialog').locator('[name=server_id]')
      assert.equal(await selected.inputValue(), '2', 'Creation uses the explicitly restored current filter')
      assert.equal(control.writes.length, 0)
    })
  }
  assert.deepEqual(totals.external, []); assert.deepEqual(totals.unexpected, []); assert.deepEqual(totals.errors, [])
  console.log(JSON.stringify({ result: 'PASS', ...totals }))
} finally { await browser.close(); await new Promise(resolve => server.close(resolve)) }
