import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import { proxyResourceFixtures } from './proxy-resource-fixtures.mjs'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'

// TEST_ONLY: exercise shipped UI against private loopback APIs. No real provider or credentials.
const catalogView = resources => resources.map(resource => ({ ...resource, original_name: resource.name, tags: [], note: '', sort_order: resource.id, revision: '1'.repeat(64), metadata_revision: 0 }))
// TEST_ONLY independent read-only view; no actual preflight or device evidence.
const operationsViewFixture = {
  runtime: { supported_versions: [], selected_version: 'TEST_ONLY', reason: 'TEST_ONLY 未读取真实运行时版本。', compatibility_metadata: { upstream_release: 'https://example.com/TEST_ONLY', upstream_commit: 'TEST_ONLY', protocols: [], acceptance_scope: 'TEST_ONLY 只读夹具' } },
  preflight: { id: null, ready: false, confirmed: false, device_checks_pending: false, checks: [] },
  drift: { state: 'unknown', target_revision: null, applied_revision: null, last_observed_at: null, reason: 'TEST_ONLY 无真实配置检查点。', checkpoint_supported: false, checkpoint: { state: 'unknown', observed_at: null, reason: 'TEST_ONLY 未读取进程或文件。' } },
  changes: [], history: [], paths: [], hop_observations: [], path_diagnosis: 'TEST_ONLY 无真实路径证据。',
}
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const root = fileURLToPath(new URL('../dist/', import.meta.url)), trafficApi = '/api/traffic', accessApi = '/api/access'
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname, file = resolve(root, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(root.endsWith(sep) ? root : `${root}${sep}`)) return response.writeHead(400).end()
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': ({ '.html': 'text/html', '.css': 'text/css', '.js': 'text/javascript', '.svg': 'image/svg+xml' })[extname(file)] ?? 'application/octet-stream' }).end(body) }
  catch { response.writeHead(404).end() }
})
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
const origin = `http://127.0.0.1:${server.address().port}`, totals = { scenarios: 0, blocked: 0, recovered: 0, external: [], unexpected: [], errors: [] }
const browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
const wait = async (condition, message) => { const end = Date.now() + 10000; while (!await condition()) { assert(Date.now() < end, message); await new Promise(resolve => setTimeout(resolve, 20)) } }
const forceForm = form => form.evaluate(element => element.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })))
const forceClick = button => button.evaluate(element => { const disabled = element.disabled; try { element.disabled = false; element.click() } finally { element.disabled = disabled } })
try {
  for (const width of [1440, 390]) {
    const fixture = async (hash, callback) => {
      const page = await browser.newPage({ viewport: { width, height: 1000 } }); page.setDefaultTimeout(10000)
      page.on('pageerror', error => totals.errors.push(error.message))
      const hosts = [1, 2].map(id => ({ id, name: `TEST_ONLY 服务器 ${id}`, enabled: true, online: true, agent_supported: true, read_only: false }))
      const nodes = [1, 2].map(id => ({ id, name: `TEST_ONLY 节点 ${id}`, server_id: id, protocol: 'vless-reality', public_host: '127.0.0.1', port: 20000 + id, sni: 'localhost', enabled: true }))
      const direct = node => ({ ...node, kind: 'direct', server_name: `TEST_ONLY 服务器 ${node.server_id}`, role: 'direct', available: true, tcp: true, udp: true, entry_node_id: null, stage: 'direct', active_generation: null, pending_generation: null, minimum_generation: 0, reference_count: 0, entry_eligible: true, last_error: null, legacy: false })
      const resources = nodes.map(direct), ordered = proxyResourceFixtures(nodes, hosts)
      const writes = [], gates = new Map(), failures = new Set()
      const tasks = [{ id: '00000000-0000-0000-0000-000000000001', spec: { id: '00000000-0000-0000-0000-000000000001', name: 'TEST_ONLY 周期任务', kind: 'tcp', target: '127.0.0.1', port: 443, interval_secs: 30, carrier: '', enabled: false, monitor: null }, default_enabled: false, server_ids: [1], revision: 1 }]
      const status = { module: 'singbox', target_rev: 2, applied_rev: 2, last_result_rev: 2, healthy: true, last_error: null, updated_at: 1 }
      const operation = { supported: true, online: true, retiring: false, operations: [] }
      const control = { page, writes, hosts, status, tasks,
        hold(path) { let release; const promise = new Promise(resolve => { release = resolve }); const gate = { promise, release, reached: 0 }; gates.set(path, gate); return gate },
        fail(path) { failures.add(path); gates.get(path)?.release(); gates.delete(path) },
        recover(path) { failures.delete(path); gates.get(path)?.release(); gates.delete(path) },
      }
      await page.route('**/*', async route => {
        const request = route.request(), url = new URL(request.url()), path = url.pathname, method = request.method()
        if (url.origin !== origin) { totals.external.push(url.href); return route.abort() }
        if (!path.startsWith('/api/')) return route.continue()
        if (method === 'GET') {
          const gate = gates.get(path); if (gate) { ++gate.reached; await gate.promise }
          if (failures.has(path)) return route.fulfill({ status: 503, json: { error: 'TEST_ONLY 资源刷新失败' } })
        }
        let value
        if (method !== 'GET') {
          const allowed = method === 'POST' && [ `${trafficApi}/nodes`, `${trafficApi}/servers/1/enable`, `${trafficApi}/servers/1/deployments/check`, `${trafficApi}/servers/1/runtime-operations` ].includes(path)
            || method === 'PATCH' && path === `${trafficApi}/nodes/1`
            || method === 'PATCH' && path === '/api/latency-tasks/00000000-0000-0000-0000-000000000001'
            || method === 'DELETE' && path === `${trafficApi}/nodes/1`
          assert(allowed, `${method} ${path}`)
          const body = request.postData() ? request.postDataJSON() : undefined; writes.push({ method, path, body }); value = {}
          if (path.endsWith('/deployments/check')) value = { ready: true, checks: [] }
          if (path.endsWith('/enable')) hosts[0].enabled = true
          if (path.endsWith('/runtime-operations')) value = { id: 'TEST_ONLY', operation: body.operation, requested_at: 1, expires_at: 600 }
        } else if (['/api/me', '/api/dashboard/access'].includes(path)) value = { authenticated: true, public_dashboard: false }
        else if (path === `${trafficApi}/servers` || path === '/api/servers') value = hosts
        else if (path === '/api/servers/1') value = { id: 1, name: hosts[0].name, online: true, device_public_key: 'TEST_ONLY', static_info: {}, latest_metrics: {}, last_seen: 1, manifest_rev: 2, capabilities: [] }
        else if (path === `${trafficApi}/servers/1`) value = hosts[0]
        else if (path === `${trafficApi}/servers/1/profile`) value = { definition: { outbounds: [], endpoints: [], services: [], cloudflared: [], dns: null, route: null }, revision: 0 }
        else if (path === '/api/latency-tasks') value = tasks
        else if (path === '/api/probes/overview') value = []
        else if (path === '/api/observability/reachability') value = { settings: { enabled: false, interval_minutes: 60, minimum_interval_minutes: 15, maximum_interval_minutes: 1440, token_configured: false, revision: 1, next_run_at: 0, run_requested: false, running: false, last_run: {} }, targets: [], events: [] }
        else if (path === `${trafficApi}/nodes`) value = nodes
        else if (path === `${trafficApi}/ordered-proxy-resources`) value = ordered
        else if (path === `${trafficApi}/ordered-subscription-sources`) value = []
        else if (path === `${trafficApi}/node-catalog`) value = catalogView(resources)
        else if (path === `${accessApi}/usage`) value = { total: '0', uplink: '0', downlink: '0', by_node: [], by_user: [] }
        else if (path === `${trafficApi}/servers/1/deployments`) value = { status, history: [], pending: false, enabled_nodes: 1, authorized_nodes: 1 }
        else if (!url.search && path === `${trafficApi}/servers/1/operations-view`) value = operationsViewFixture
        else if (path === `${trafficApi}/servers/1/runtime-operations`) value = operation
        else { totals.unexpected.push(`${method} ${path}`); return route.fulfill({ status: 404, json: {} }) }
        return route.fulfill({ json: value })
      })
      await installControlCenterFixtures(page)
      try { await page.goto(`${origin}/#${hash}`); await callback(control); assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true); ++totals.scenarios }
      finally { for (const gate of gates.values()) gate.release(); await page.close() }
    }
    const block = async (control, path, refresh, attempt, restored, draft = async () => {}) => {
      const baseline = control.writes.length, gate = control.hold(path)
      await refresh(); await attempt(); await wait(() => gate.reached > 0, 'Held GET must be requested'); await draft()
      assert.equal(control.writes.length, baseline, 'Pending refresh sends zero writes'); ++totals.blocked
      control.fail(path); await control.page.getByText('TEST_ONLY 资源刷新失败', { exact: true }).first().waitFor(); await attempt(); await draft()
      assert.equal(control.writes.length, baseline, 'Failed refresh sends zero writes'); ++totals.blocked
      control.recover(path); await refresh(); await restored(); await wait(() => control.writes.length === baseline + 1, 'Exactly one recovered write'); ++totals.recovered
    }
    const nodesRefresh = page => page.locator('[data-slot=page-header] button').filter({ hasText: /^刷新$/ }).evaluate(button => button.click())
    await fixture('/nodes', async control => {
      const { page, writes } = control
      await page.getByRole('button', { name: '创建节点', exact: true }).first().click()
      const dialog = page.getByRole('dialog'); await dialog.locator('[name=name]').fill('TEST_ONLY 节点草稿'); await dialog.locator('[name=public_host]').fill('127.0.0.1'); await dialog.locator('[name=sni]').fill('localhost')
      await block(control, `${trafficApi}/servers`, () => nodesRefresh(page), () => forceForm(dialog.locator('form')), () => dialog.getByRole('button', { name: '创建目标节点', exact: true }).click(), async () => assert.equal(await dialog.locator('[name=name]').inputValue(), 'TEST_ONLY 节点草稿'))
      assert.equal(writes[0].body.server_id, 1)
    })
    await fixture('/nodes', async ({ page, hosts, writes }) => {
      await page.getByRole('button', { name: '创建节点', exact: true }).first().click()
      const dialog = page.getByRole('dialog'); await dialog.locator('[name=name]').fill('TEST_ONLY 固定服务器草稿')
      hosts.splice(0, 1); await nodesRefresh(page)
      await dialog.getByText('已选节点所属服务器已不存在或未安装 sing-box；当前草稿已保留。', { exact: true }).waitFor()
      assert.equal(await dialog.locator('[name=server_id]').inputValue(), '1'); await forceForm(dialog.locator('form')); assert.equal(writes.length, 0)
      await dialog.locator('[name=server_id]').selectOption('2'); await dialog.locator('[name=public_host]').fill('127.0.0.1'); await dialog.locator('[name=sni]').fill('localhost'); await dialog.getByRole('button', { name: '创建目标节点', exact: true }).click()
      await wait(() => writes.length === 1, 'Explicit new server selection'); assert.equal(writes[0].body.server_id, 2)
    })
    await fixture('/nodes', async control => {
      const { page } = control
      await page.locator(width < 768 ? '[data-slot=catalog-card]' : '[data-slot=catalog-table] tbody tr').filter({ hasText: 'TEST_ONLY 节点 1' }).getByRole('button', { name: '删除', exact: true }).click()
      const button = page.getByRole('alertdialog').getByRole('button', { name: '确认删除', exact: true })
      await block(control, `${trafficApi}/ordered-proxy-resources`, () => nodesRefresh(page), () => forceClick(button), () => button.click())
    })
    const servicesRefresh = page => page.locator('[data-slot=panel-header]').filter({ hasText: 'sing-box' }).getByRole('button', { name: '刷新', exact: true }).evaluate(button => button.click())
    await fixture('/servers/1/services', async control => {
      const { page, writes, status } = control
      const runtime = page.locator('[data-slot=runtime-operations]'), refresh = () => servicesRefresh(page)
      const inspect = runtime.getByRole('button', { name: '读取状态与日志', exact: true })
      await wait(() => inspect.isEnabled(), 'Runtime/deployment initial snapshots')
      await block(control, `${trafficApi}/servers/1/deployments`, refresh, () => forceClick(inspect), () => inspect.click())
      await runtime.getByRole('button', { name: '重启运行时', exact: true }).click(); status.target_rev = 3; await refresh()
      await runtime.getByRole('alert').getByText('期望版本已改变，请取消后重新确认。', { exact: true }).waitFor()
      await forceClick(runtime.getByRole('button', { name: '确认重启', exact: true })); assert.equal(writes.length, 1, 'A confirmation must never retarget to a newer revision')
    })
    await fixture('/latency/settings', async control => {
      const { page, writes } = control
      await page.getByRole('row').filter({ hasText: 'TEST_ONLY 周期任务' }).getByRole('button', { name: '编辑', exact: true }).click()
      const dialog = page.getByRole('dialog'); await dialog.getByLabel('任务名称').fill('TEST_ONLY 周期草稿')
      await block(control, '/api/latency-tasks', () => nodesRefresh(page), () => forceForm(dialog.locator('form')), () => dialog.getByRole('button', { name: '保存任务', exact: true }).click(), async () => assert.equal(await dialog.getByLabel('任务名称').inputValue(), 'TEST_ONLY 周期草稿'))
      assert.equal(writes[0].body.revision, 1); assert.equal(writes[0].body.spec.enabled, false)
    })
    await fixture('/latency/settings', async ({ page, writes, tasks }) => {
      const row = page.getByRole('row').filter({ hasText: 'TEST_ONLY 周期任务' })
      await wait(() => row.getByRole('button', { name: '编辑', exact: true }).isEnabled(), 'Initial task revision')
      delete tasks[0].revision
      const read = page.waitForResponse(response => new URL(response.url()).pathname === '/api/latency-tasks' && response.request().method() === 'GET')
      await nodesRefresh(page); await read
      for (const label of ['编辑', '启用', '删除']) {
        const button = row.getByRole('button', { name: label, exact: true })
        await wait(() => button.isDisabled(), 'Missing task revisions fail closed')
        await forceClick(button); ++totals.blocked
      }
      assert.equal(await page.getByRole('dialog').count(), 0); assert.equal(writes.length, 0)
      tasks[0].revision = 1
      const restored = page.waitForResponse(response => new URL(response.url()).pathname === '/api/latency-tasks' && response.request().method() === 'GET')
      await nodesRefresh(page); await restored
      await row.getByRole('button', { name: '编辑', exact: true }).click()
      const dialog = page.getByRole('dialog'); await dialog.getByLabel('任务名称').fill('TEST_ONLY 原修订草稿')
      tasks[0].revision = 2
      await nodesRefresh(page)
      await dialog.getByText('延迟任务目标或版本已变化；草稿已保留。', { exact: true }).waitFor()
      await forceForm(dialog.locator('form')); assert.equal(writes.length, 0); ++totals.blocked
      assert.equal(await dialog.getByLabel('任务名称').inputValue(), 'TEST_ONLY 原修订草稿')
      await dialog.getByRole('button', { name: '取消', exact: true }).click()
      await row.getByRole('button', { name: '编辑', exact: true }).click()
      await page.getByRole('dialog').getByRole('button', { name: '保存任务', exact: true }).click()
      await wait(() => writes.length === 1, 'Explicit current revision selection permits one write'); ++totals.recovered
      assert.equal(writes[0].body.revision, 2)
    })
    await fixture('/servers/1/services', async control => {
      const { page, hosts } = control; hosts[0].enabled = false
      await servicesRefresh(page); const enable = page.getByRole('button', { name: '安装 sing-box', exact: true }); await wait(() => enable.isEnabled(), 'Enable snapshot')
      await block(control, `${trafficApi}/servers/1`, () => servicesRefresh(page), () => forceClick(enable), () => enable.click())
    })
  }
  assert.deepEqual(totals.external, []); assert.deepEqual(totals.unexpected, []); assert.deepEqual(totals.errors, [])
  console.log(JSON.stringify({ result: 'PASS', ...totals }))
} finally { await browser.close(); await new Promise(resolve => server.close(resolve)) }
