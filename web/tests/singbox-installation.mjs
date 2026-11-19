import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import { catalogNodeFixtures, catalogResourceFixtures, proxyResourceFixtures, runtimeServerFixture } from './proxy-resource-fixtures.mjs'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'

// Exercise the actual dist with loopback fixtures; no Agent, release, or production access.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
// Read-only runtime operations view shown on the server's network services tab once configured.
const operationsViewFixture = {
  runtime: { supported_versions: [], selected_version: '1.14.4-apsp.2', reason: 'TEST_ONLY 一体化 Agent 内置运行时只读夹具。', compatibility_metadata: { upstream_release: 'https://example.com/TEST_ONLY', upstream_commit: 'TEST_ONLY_COMMIT', protocols: [], acceptance_scope: 'TEST_ONLY 只读夹具' } },
  preflight: { id: null, ready: false, confirmed: false, device_checks_pending: false, checks: [] },
  drift: { state: 'unknown', target_revision: null, applied_revision: null, last_observed_at: null, reason: 'TEST_ONLY 无真实配置检查点。', checkpoint_supported: false, checkpoint: { state: 'unknown', observed_at: null, reason: 'TEST_ONLY 未读取进程或文件。' } },
  changes: [], history: [], paths: [], hop_observations: [], path_diagnosis: 'TEST_ONLY 无真实路径证据。',
}
const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }
const server = createServer(async (request, response) => {
  const pathname = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, pathname === '/' ? 'index.html' : `.${pathname}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': mime[extname(file)] ?? 'application/octet-stream' }); response.end(body) }
  catch { response.writeHead(404).end() }
})
const wait = async (condition, message) => {
  const deadline = Date.now() + 10000
  while (!await condition()) { assert(Date.now() < deadline, message); await new Promise(done => setTimeout(done, 25)) }
}

let browser
try {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  const origin = `http://127.0.0.1:${server.address().port}`
  for (const width of [1440, 390]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 }, serviceWorkers: 'block' })
    context.setDefaultTimeout(10000)
    const page = await context.newPage()
    const errors = [], unexpected = [], reads = [], writes = [], now = Math.floor(Date.now() / 1000)
    const prefix = '/api/traffic/servers'
    const name = '一体化待配置服务器'
    const offlineName = '离线待配置服务器'
    const nexusName = '只读兼容接管主机'
    let configured = false, targetRev = 0, agentSupported = true, online = true, enrolled = true
    let stateOverride = null, reasonOverride = null, deploymentError = null
    let failServers = false, omitReadOnly = false
    const nodes = []
    const device = { appliedRev: 0, healthy: false }
    const computeStatus = () => {
      const revisions = { target_rev: targetRev, applied_rev: device.appliedRev, current_healthy: device.appliedRev > 0 && device.healthy }
      if (!configured) return { ...revisions, state: 'unconfigured', reason: '' }
      if (!enrolled) return { ...revisions, state: 'waiting_agent', reason: '已保存代理服务配置，等待服务器首次接入 Agent' }
      if (!online) return { ...revisions, state: 'offline', reason: '设备当前离线，重连后自动同步应用' }
      if (stateOverride) return { ...revisions, state: stateOverride, reason: reasonOverride ?? '' }
      if (!agentSupported) return { ...revisions, state: 'upgrade_required', reason: reasonOverride ?? '当前连接的 Agent 为旧版非一体化版本，请升级至自带 sing-box 能力的 Agent' }
      if (device.appliedRev === targetRev && targetRev > 0 && device.healthy && !deploymentError) return { ...revisions, state: 'ready', reason: '一体化 Agent 已应用最新目标配置且健康检查通过' }
      return { ...revisions, state: 'pending', reason: reasonOverride ?? '配置已生成，等待一体化 Agent 自动准备运行时并应用' }
    }
    const selected = () => {
      const record = runtimeServerFixture(1, {
        name,
        configured,
        online,
        agent_supported: agentSupported,
        read_only: false,
        source: configured ? 'configured' : null,
        runtime_version: agentSupported ? '1.14.4-apsp.2' : null,
        status: computeStatus(),
      })
      if (omitReadOnly) delete record.read_only
      return record
    }
    const offlineServer = runtimeServerFixture(2, {
      name: offlineName,
      configured: false,
      online: false,
      agent_supported: false,
      read_only: false,
      source: null,
      runtime_version: null,
      status: { state: 'unconfigured', reason: '', target_rev: 0, applied_rev: 0, current_healthy: false },
    })
    const nexusServer = runtimeServerFixture(3, {
      name: nexusName,
      configured: true,
      online: true,
      agent_supported: false,
      read_only: true,
      source: 'nexus',
      runtime_version: '1.12.0',
      status: { state: 'upgrade_required', reason: '兼容保留主机只读运行中', target_rev: 0, applied_rev: 0, current_healthy: true },
    })
    const allServers = () => [selected(), offlineServer, nexusServer]

    page.on('pageerror', error => errors.push(error.message))
    page.on('console', message => { if (message.type() === 'error' && !message.text().includes('503')) errors.push(message.text()) })
    await context.route('**/*', async route => {
      const request = route.request(), url = new URL(request.url()), method = request.method(), pathname = url.pathname
      if (url.origin !== origin) {
        unexpected.push(`External request refused: ${method} ${url.origin}${pathname}`)
        await route.abort('blockedbyclient'); return
      }
      if (!pathname.startsWith('/api/')) { await route.continue(); return }
      let value
      if (method === 'GET') {
        reads.push(pathname)
        if (pathname === '/api/dashboard/access') value = { authenticated: true, public_dashboard: false }
        else if (pathname === '/api/me') value = { authenticated: true }
        else if (pathname === prefix) {
          if (failServers) { await route.fulfill({ status: 503, json: { error: 'TEST_ONLY 服务器状态读取失败' } }); return }
          value = allServers()
        }
        else if (pathname === `${prefix}/1`) value = selected()
        else if (pathname === `${prefix}/2`) value = offlineServer
        else if (pathname === `${prefix}/3`) value = nexusServer
        else if (pathname === '/api/servers/1') value = {
          id: 1, name, online, device_public_key: enrolled ? 'TEST_ONLY' : null,
          capabilities: agentSupported ? ['agent:self-contained:v1'] : [],
          static_info: agentSupported ? { agent_version: '0.4.0', runtime_version: '1.14.4-apsp.2', components: { 'sing-box': '1.14.4-apsp.2', tcpquality: '0.1.0' } } : { agent_version: '0.2.0' },
          latest_metrics: {}, manifest_rev: targetRev,
        }
        else if (pathname === '/api/servers/3') value = { id: 3, name: nexusName, online: true, device_public_key: 'TEST_ONLY_NEXUS', capabilities: [], static_info: {}, latest_metrics: {}, manifest_rev: 0 }
        else if (pathname === `${prefix}/1/profile`) value = { definition: { outbounds: [], endpoints: [], services: [], cloudflared: [], dns: null, route: null }, revision: 0 }
        else if (pathname === `${prefix}/1/deployments`) {
          const current = computeStatus()
          value = {
            status: targetRev > 0 ? { module: 'singbox', target_rev: targetRev, applied_rev: device.appliedRev, last_result_rev: device.appliedRev || targetRev, healthy: current.state === 'ready' || Boolean(current.current_healthy && !deploymentError), last_error: deploymentError, updated_at: now } : null,
            history: targetRev > 0 ? [{ module: 'singbox', rev: targetRev, bundle_sha256: 'a'.repeat(64), created_at: now }] : [],
            pending: current.state === 'pending',
            enabled_nodes: nodes.length,
            authorized_nodes: 0,
          }
        }
        else if (pathname === `${prefix}/3/deployments`) value = { status: null, history: [], pending: false, enabled_nodes: 0, authorized_nodes: 0 }
        else if (pathname === `${prefix}/1/runtime-operations` || pathname === `${prefix}/3/runtime-operations`) value = { supported: false, online: false, retiring: false, operations: [] }
        else if (pathname === '/api/traffic/nodes') value = nodes
        else if ((pathname === `${prefix}/1/operations-view` || pathname === `${prefix}/3/operations-view`) && !url.search) value = operationsViewFixture
        else if (pathname === '/api/traffic/ordered-proxy-resources') value = proxyResourceFixtures(nodes, [runtimeServerFixture(1, { name, configured, online }), offlineServer, nexusServer])
        else if (pathname === '/api/traffic/node-catalog') value = catalogResourceFixtures(catalogNodeFixtures(nodes, [runtimeServerFixture(1, { name, configured, online }), offlineServer, nexusServer]))
        else if (pathname === '/api/traffic/ordered-subscription-sources') value = []
        else if (pathname === '/api/access/usage') value = { uplink: '0', downlink: '0', total: '0', by_user: [], by_node: [] }
      } else if (pathname === '/api/traffic/nodes' && method === 'POST') {
        const payload = request.postDataJSON()
        writes.push({ pathname, method, payload })
        const created = {
          id: nodes.length + 1,
          name: payload.name,
          server_id: payload.server_id,
          protocol: payload.protocol_config.type,
          port: payload.port ?? 443,
          public_host: payload.public_host,
          sni: payload.sni,
          enabled: payload.enabled,
          public_key: 'TEST_ONLY_PUBKEY',
          short_id: '0123abcd',
          protocol_config: payload.protocol_config,
          settings: payload.settings,
        }
        nodes.push(created)
        configured = true
        targetRev = 1
        value = created
      } else if (pathname === `${prefix}/1/deployments/redeploy` && method === 'POST') {
        writes.push({ pathname, method, payload: null })
        targetRev += 1
        deploymentError = null
        stateOverride = null
        reasonOverride = '重新部署已生成新版本，等待一体化 Agent 应用'
        value = { revision: targetRev }
      }
      if (value === undefined) {
        unexpected.push(`${method} ${pathname}`)
        await route.fulfill({ status: 404, json: { error: '测试拒绝未知接口' } }); return
      }
      await route.fulfill({ json: value })
    })

    // 1. Enrolled self-contained Agent on an unconfigured server stays monitor-only without starting sing-box.
    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/servers/1/services`)
    const card = page.locator('section').filter({ has: page.getByRole('heading', { name: /^sing-box/ }) })
    await card.getByText('未配置服务', { exact: true }).waitFor()
    assert.equal(await page.getByRole('navigation', { name: '服务器导航' }).getByRole('link', { name: '网络服务', exact: true }).getAttribute('aria-current'), 'page')
    await card.getByText('当前服务器尚未配置代理节点或链路，纯监控模式不运行代理进程。', { exact: false }).waitFor()
    assert.equal(await card.getByRole('button', { name: '安装 sing-box', exact: true }).count(), 0, 'No manual install button exists')
    assert.equal(await page.locator('[data-runtime="sing-box"]').count(), 0, 'Monitor-only server does not activate runtime')
    assert.equal(await card.getByRole('link', { name: '管理节点', exact: true }).count(), 0)
    assert.equal(await card.getByRole('link', { name: '添加节点', exact: true }).getAttribute('href'), '#/nodes?server=1')
    await page.getByLabel('服务端配置 JSON').waitFor()
    assert.equal(reads.includes('/api/traffic/servers/1/deployments'), false, 'Unconfigured server does not query deployments')
    assert.equal(reads.includes(prefix), false, 'Single-server view reads only /api/traffic/servers/1')
    assert.equal(writes.length, 0, 'Capability discovery never auto-enables a monitor-only server')

    // 2. Ordinary unconfigured or offline servers (!read_only) can be selected directly on the Nodes page;
    //    stale/failed reads and missing read_only fail closed; read_only Nexus hosts are excluded.
    await card.getByRole('link', { name: '添加节点', exact: true }).click()
    await page.getByRole('heading', { name: '节点', exact: true }).waitFor()
    await page.getByRole('button', { name: '创建节点', exact: true }).first().click()
    const dialog = page.getByRole('dialog')
    const serverSelect = dialog.locator('select[name="server_id"]')
    assert.equal(await serverSelect.inputValue(), '1')
    assert.equal(await serverSelect.locator('option[value="1"]').count(), 1, 'Unconfigured online server is selectable')
    assert.equal(await serverSelect.locator('option[value="2"]').count(), 1, 'Unconfigured offline server is selectable')
    assert.equal(await serverSelect.locator('option[value="3"]').count(), 0, 'Read-only Nexus server is excluded')

    await dialog.locator('input[name="name"]').fill('直配一体化节点')
    await dialog.locator('input[name="public_host"]').fill('edge.example.com')
    await dialog.locator('input[name="sni"]').fill('www.example.com')
    const createSubmit = dialog.getByRole('button', { name: '创建目标节点', exact: true })
    const refreshNodes = () => page.locator('header[data-slot="page-header"] button').filter({ hasText: /^刷新$/ }).evaluate(button => button.click())
    const forceForm = () => dialog.locator('form').evaluate(form => form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })))

    // Stale/failed server snapshot blocks submission while preserving the draft.
    failServers = true
    await refreshNodes()
    await page.getByText('TEST_ONLY 服务器状态读取失败', { exact: true }).first().waitFor()
    await wait(() => createSubmit.isDisabled(), 'Failed server snapshot disables node creation')
    await forceForm()
    assert.equal(writes.length, 0)
    assert.equal(await dialog.locator('input[name="name"]').inputValue(), '直配一体化节点')
    failServers = false
    await refreshNodes()
    await wait(() => createSubmit.isEnabled(), 'Recovered server snapshot restores node creation')

    // Missing `read_only` field fails closed (`server.read_only === false`).
    omitReadOnly = true
    await refreshNodes()
    await dialog.getByText('已选节点所属服务器不存在或为只读接管服务器；当前草稿已保留。', { exact: true }).waitFor()
    assert.equal(await createSubmit.isDisabled(), true)
    await forceForm()
    assert.equal(writes.length, 0, 'Missing read_only field must never permit node creation')
    omitReadOnly = false
    await refreshNodes()
    await wait(() => createSubmit.isEnabled(), 'Explicit read_only: false restores node creation')

    // Offline ordinary server is also selectable without prior installation; switch back to server 1 to submit.
    await serverSelect.selectOption('2')
    assert.equal(await createSubmit.isEnabled(), true, 'Offline unconfigured server can be configured directly')
    await serverSelect.selectOption('1')
    await createSubmit.click()
    await dialog.waitFor({ state: 'hidden' })
    assert.equal(writes.length, 1)
    assert.equal(writes[0].pathname, '/api/traffic/nodes')
    await page.getByText('节点已保存为目标配置，尚未授权给任何代理用户，所以暂不会监听端口。', { exact: false }).waitFor()
    assert.equal(await page.getByRole('link', { name: '加入策略组', exact: true }).getAttribute('href'), '#/groups')

    // 3. Automatic publish & runtime readiness without /enable, preflight/bootstrap, or preflight/confirm.
    await page.getByRole('button', { name: '查看部署状态', exact: true }).click()
    await page.waitForURL(/#\/servers\/1\/services$/)
    await card.getByText('等待应用配置', { exact: true }).waitFor()
    await card.getByText('配置已生成，等待一体化 Agent 自动准备运行时并应用', { exact: true }).waitFor()
    await card.getByText('还没有节点授权给有效用户，节点在授权前不监听。', { exact: false }).waitFor()
    assert.equal(await card.getByText('运行中', { exact: true }).count(), 0)

    const preflightSummary = page.locator('summary', { hasText: '只读部署环境预检' })
    await preflightSummary.click()
    await page.getByText('普通节点保存后自动发布并由 Agent 应用，此预检不阻塞发布。', { exact: false }).waitFor()
    assert.equal(await page.getByRole('button', { name: /首次安装空运行时|再次验证并确认继续发布/ }).count(), 0, 'Bootstrap and blocking confirm buttons are removed')

    const refreshService = async (label, reason) => {
      const response = page.waitForResponse(res => new URL(res.url()).pathname === `${prefix}/1` && res.request().method() === 'GET')
      await card.getByRole('button', { name: '刷新', exact: true }).click()
      await response
      await card.getByText(label, { exact: true }).waitFor()
      if (reason) await card.getByText(reason, { exact: true }).waitFor()
      assert.equal(await card.getByText('运行中', { exact: true }).count(), label === '运行中' ? 1 : 0)
    }

    enrolled = false
    await refreshService('等待设备接入', '已保存代理服务配置，等待服务器首次接入 Agent')
    enrolled = true; online = false
    await refreshService('设备离线', '设备当前离线，重连后自动同步应用')
    online = true; device.appliedRev = 1; device.healthy = true
    await refreshService('运行中')

    // Legacy non-self-contained Agent prompts for upgrade while retaining existing healthy runtime.
    targetRev = 2
    agentSupported = false
    reasonOverride = '当前连接的 Agent 为旧版非一体化版本，请升级至自带 sing-box 能力的 Agent'
    await refreshService('待升级 Agent', reasonOverride)
    await card.getByText('当前 Agent 尚未声明内置运行时能力，请升级 Agent', { exact: true }).waitFor()
    assert.equal(await card.getByText('当前配置健康', { exact: true }).count(), 1)

    // Blocked synchronization also preserves existing healthy state.
    agentSupported = true
    stateOverride = 'sync_blocked'
    reasonOverride = '链路恢复屏障尚未解除，保留设备当前健康配置'
    await refreshService('配置同步受阻', reasonOverride)
    assert.equal(await card.getByText('当前配置健康', { exact: true }).count(), 1)

    // Application failure & manual forced redeploy recovery.
    stateOverride = 'failed'
    reasonOverride = '目标端口冲突，已回滚保留上一健康版本'
    deploymentError = 'listen tcp :443: bind: address already in use'
    await refreshService('应用失败', reasonOverride)
    await card.getByText('最近一次部署失败', { exact: true }).waitFor()
    await card.getByRole('button', { name: '重新部署', exact: true }).click()
    const confirmDialog = page.getByRole('alertdialog', { name: '重新部署当前配置？' })
    await confirmDialog.getByRole('button', { name: '重新部署', exact: true }).click()
    await confirmDialog.waitFor({ state: 'hidden' })
    await card.getByText('已生成版本 3，等待设备应用。', { exact: true }).waitFor()
    device.appliedRev = 3; reasonOverride = null
    await refreshService('运行中')

    // 4. Runtimes workspace view, read-only retained server, and retired route check.
    await page.goto(`${origin}/#/nodes?view=runtimes`)
    await page.getByRole('heading', { name: '各服务器的运行时', exact: true }).waitFor()
    assert.equal(await page.title(), '节点 · apsp')
    const row1 = page.locator('tbody tr').filter({ has: page.getByRole('link', { name, exact: true }) })
    await row1.getByText('运行中', { exact: true }).waitFor()
    await row1.getByText('sing-box 1.14.4-apsp.2', { exact: true }).waitFor()
    assert.equal(await row1.getByRole('link', { name: '查看部署', exact: true }).getAttribute('href'), '#/servers/1/services')
    const row2 = page.locator('tbody tr').filter({ hasText: offlineName })
    await row2.getByText('未配置服务', { exact: true }).waitFor()
    assert.equal(await row2.getByRole('link', { name: '配置服务', exact: true }).getAttribute('href'), '#/servers/2/services')
    const row3 = page.locator('tbody tr').filter({ hasText: nexusName })
    await row3.getByText('待升级 Agent', { exact: true }).waitFor()
    await row3.getByText('只读保留主机', { exact: true }).waitFor()
    assert.equal(await page.getByRole('button', { name: '安装 sing-box', exact: true }).count(), 0)

    await page.goto(`${origin}/#/servers/3/services`)
    await page.getByText('只读保留主机', { exact: true }).waitFor()
    assert.equal(await page.getByRole('link', { name: '添加节点', exact: true }).count(), 0)
    assert.equal(await page.getByLabel('服务端配置 JSON').count(), 0, 'Read-only server does not expose editable ServerProfile')

    await page.goto(`${origin}/#/settings/plugins/servers`)
    await page.getByRole('heading', { name: '这个页面不存在', exact: true }).waitFor()
    assert.deepEqual(writes.map(item => item.pathname), ['/api/traffic/nodes', `${prefix}/1/deployments/redeploy`])
    assert.deepEqual(unexpected, [])
    assert.deepEqual(errors, [])
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
    await context.close()
  }
  console.log('PASS: dist 1440/390, monitor-only unconfigured server stays inert, direct node creation on ordinary/offline servers without enable/bootstrap/confirm, stale & missing-read_only fail-closed guards, waiting_agent/offline/pending/ready/upgrade_required/sync_blocked/failed & manual redeploy, read-only Nexus guard, unexpected API/errors=0')
} finally {
  try { await browser?.close() }
  finally { await new Promise(resolve => server.close(resolve)) }
}
