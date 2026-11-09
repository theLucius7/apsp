import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'

// Exercise the actual dist with loopback fixtures; no Agent, release, or production access.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
// Read-only runtime operations view shown on the server's network services tab once sing-box is installed.
const operationsViewFixture = {
  runtime: { supported_versions: [], selected_version: 'TEST_ONLY', reason: 'TEST_ONLY 未读取真实运行时版本。', compatibility_metadata: { upstream_release: 'https://example.com/TEST_ONLY', upstream_commit: 'TEST_ONLY', protocols: [], acceptance_scope: 'TEST_ONLY 只读夹具' } },
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

let browser
try {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  const origin = `http://127.0.0.1:${server.address().port}`
  for (const width of [1440, 390]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 }, serviceWorkers: 'block' })
    context.setDefaultTimeout(10000)
    const page = await context.newPage()
    const errors = [], unexpected = [], reads = [], writes = []
    const prefix = '/api/plugins/sing-box/servers'
    const name = '能力支持但尚未安装服务器'
    const otherName = '另一台未安装服务器'
    const missingArtifact = '清单准备失败：目标版本 1：缺少此平台的已验签 sing-box 1.14.2 制品，请先导入可信发布制品'
    let enabled = false, targetRev = 0, preparationError = null
    const device = { downloaded: false, appliedRev: 0, healthy: false }
    const installation = () => {
      const revisions = { target_rev: targetRev, applied_rev: device.appliedRev }
      if (!enabled) return { ...revisions, state: 'not_enabled', reason: '尚未安装 sing-box；设备支持 sing-box 不代表已安装' }
      if (!targetRev) return { ...revisions, state: 'queued', reason: '安装请求已保存，正在生成初始运行配置' }
      if (preparationError) return { ...revisions, state: 'failed', reason: preparationError }
      // Neither enablement nor downloading bytes establishes a device application/health result.
      if (device.appliedRev === targetRev && device.healthy) return { ...revisions, state: 'ready', reason: '设备已确认应用目标版本，运行时健康检查通过' }
      return { ...revisions, state: 'pending', reason: '等待设备下载已签制品、应用配置并确认健康；尚未确认安装完成' }
    }
    const selected = () => ({ id: 1, name, enabled, online: true, agent_supported: true, read_only: false, source: enabled ? 'administrator' : null, installation: installation() })
    const other = { id: 2, name: otherName, enabled: false, online: false, agent_supported: false, read_only: false, source: null, installation: { state: 'not_enabled', reason: '尚未安装 sing-box', target_rev: 0, applied_rev: 0 } }
    page.on('pageerror', error => errors.push(error.message))
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()) })
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
        else if (pathname === prefix) value = [selected(), other]
        else if (pathname === `${prefix}/1`) value = selected()
        else if (pathname === `${prefix}/2`) value = other
        // The server's network services tab shares the server header and shows the runtime once installed.
        else if (pathname === '/api/servers/1') value = { id: 1, name, online: true, device_public_key: 'TEST_ONLY', capabilities: ['singbox'], static_info: {}, latest_metrics: {}, manifest_rev: targetRev }
        else if (pathname === `${prefix}/1/profile`) value = {definition:{},revision:0}
        else if (pathname === `${prefix}/1/deployments`) value = { status: null, history: [] }
        else if (pathname === `${prefix}/1/runtime-operations`) value = { supported: false, online: false, retiring: false, operations: [] }
        else if (pathname === '/api/plugins/sing-box/nodes') value = []
        else if (pathname === `${prefix}/1/operations-view` && !url.search) value = operationsViewFixture
        // The network services workspace lists every server's runtime beside its nodes and chains.
        else if (pathname === '/api/plugins/sing-box/ordered-proxy-resources') value = []
        else if (pathname === '/api/plugins/sing-box/node-catalog') value = []
        else if (pathname === '/api/plugins/sing-box/ordered-subscription-sources') value = []
        else if (pathname === '/api/plugins/sing-box/usage') value = { uplink: '0', downlink: '0', total: '0', by_user: [], by_node: [] }
      } else if (pathname === `${prefix}/1/enable` && method === 'POST') {
        const payload = request.postDataJSON()
        assert.deepEqual(payload, {})
        writes.push({ pathname, method, payload })
        enabled = true
        value = selected()
      }
      if (value === undefined) {
        unexpected.push(`${method} ${pathname}`)
        await route.fulfill({ status: 404, json: { error: '测试拒绝未知接口' } }); return
      }
      await route.fulfill({ json: value })
    })

    // A runtime is installed from the server's own network services tab, under its old address too.
    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/servers/1/plugins`)
    const card = page.locator('section').filter({ has: page.getByRole('heading', { name: /^sing-box/ }) })
    await card.getByRole('button', { name: '安装 sing-box', exact: true }).waitFor()
    assert.equal(await page.getByRole('navigation', { name: '服务器导航' }).getByRole('link', { name: '网络服务', exact: true }).getAttribute('aria-current'), 'page')
    assert.equal(await card.getByText('设备尚未声明支持 sing-box', { exact: true }).count(), 0, 'A supported device needs no note')
    assert.equal(await card.getByText('未安装', { exact: true }).count(), 1)
    assert.equal(await card.getByRole('link', { name: '管理节点', exact: true }).count(), 0)
    assert.equal(await card.getByText('已安装并运行', { exact: true }).count(), 0)
    assert.equal(await card.getByRole('button', { name: '安装 sing-box', exact: true }).isEnabled(), true)
    assert.equal(writes.length, 0, 'capability discovery must not implicitly enable a server')

    await card.getByRole('button', { name: '安装 sing-box', exact: true }).click()
    await card.getByText('安装已安排', { exact: true }).waitFor()
    await card.getByText('安装请求已保存，正在生成初始运行配置', { exact: true }).waitFor()
    assert.deepEqual(writes, [{ pathname: `${prefix}/1/enable`, method: 'POST', payload: {} }])
    assert.equal(device.appliedRev, 0)
    assert.equal(await card.getByText('已安装并运行', { exact: true }).count(), 0)
    assert.equal(await card.getByRole('button', { name: '安装 sing-box', exact: true }).count(), 0)

    const refresh = async (label, reason) => {
      const response = page.waitForResponse(response => new URL(response.url()).pathname === `${prefix}/1` && response.request().method() === 'GET')
      await card.getByRole('button', { name: '刷新', exact: true }).click()
      await response
      await card.getByText(label, { exact: true }).waitFor()
      if (reason) await card.getByText(reason, { exact: true }).waitFor()
      assert.equal(await card.getByText('已安装并运行', { exact: true }).count(), label === '已安装并运行' ? 1 : 0)
      assert.equal(writes.length, 1, 'refreshing installation evidence must not install again')
    }
    targetRev = 1
    await refresh('等待应用配置', '等待设备下载已签制品、应用配置并确认健康；尚未确认安装完成')
    preparationError = missingArtifact
    await refresh('安装或部署失败', missingArtifact)
    assert.equal(device.appliedRev, 0, 'a preparation failure must not claim a device application')
    preparationError = null
    device.downloaded = true
    await refresh('等待应用配置')
    assert.equal(device.healthy, false)
    device.appliedRev = 1
    await refresh('等待应用配置')
    assert.equal(device.downloaded, true)
    device.healthy = true
    await refresh('已安装并运行')
    // The single-server tab reads only this server's runtime, never the collection.
    assert.equal(reads.includes(prefix), false, 'per-server view must not load the entire collection')

    // The network services workspace lists every runtime and sends installation to the server's tab.
    await page.goto(`${origin}/#/nodes?view=runtimes`)
    await page.getByRole('heading', { name: '各服务器的运行时', exact: true }).waitFor()
    assert.equal(await page.title(), '代理节点 · apsp')
    const row = page.locator('tbody tr').filter({ has: page.getByRole('link', { name, exact: true }) })
    await row.getByText('已安装并运行', { exact: true }).waitFor()
    assert.equal(await row.getByRole('link', { name: '查看部署', exact: true }).getAttribute('href'), '#/servers/1/services')
    const otherRow = page.locator('tbody tr').filter({ hasText: otherName })
    assert.equal(await otherRow.getByText('未安装', { exact: true }).count(), 1)
    assert.equal(await otherRow.getByText('设备尚未声明支持 sing-box', { exact: true }).count(), 1)
    assert.equal(await otherRow.getByRole('link', { name: '安装运行时', exact: true }).getAttribute('href'), '#/servers/2/services')
    assert.equal(await page.getByRole('button', { name: '安装 sing-box', exact: true }).count(), 0, 'the list only links to each server')
    // The plugin centre's runtime list opens this view for one release.
    await page.goto(`${origin}/#/settings/plugins/servers`)
    await page.getByRole('heading', { name: '各服务器的运行时', exact: true }).waitFor()
    assert.equal(writes.length, 1)
    assert.deepEqual(unexpected, [])
    assert.deepEqual(errors, [])
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
    await context.close()
  }
  console.log('PASS: dist 1440/390, install once from the server tab, queued/pending/missing artifact reason, readiness requires fixture device evidence, runtime list links each server, unexpected API/errors=0')
} finally {
  try { await browser?.close() }
  finally { await new Promise(resolve => server.close(resolve)) }
}
