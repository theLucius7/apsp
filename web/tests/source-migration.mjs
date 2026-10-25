import assert from 'node:assert/strict'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { resolve, extname, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { catalogResourceFixtures, flatResourceFixtures, proxyResourceFixtures } from './proxy-resource-fixtures.mjs'
import { sourceMigrationFixture, sourceNodePageFixture, sourceRevisionFixture, subscriptionSourceFixture } from './subscription-source-fixtures.mjs'

// After the source migration numbered sources are a read-only archive, and the
// page header creates ordered chains instead of mixed ones. Until the state is
// confirmed, numbered-source writes and the header form stay blocked. Owned
// fixtures only.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
const server = createServer(async (request, response) => {
  const pathname = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, pathname === '/' ? 'index.html' : `.${pathname}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' })[extname(file)] ?? 'application/octet-stream' }).end(body) }
  catch { response.writeHead(404).end() }
})
let browser
try {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  const origin = `http://127.0.0.1:${server.address().port}`, prefix = '/api/plugins/sing-box'
  for (const width of [1440, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } })
    page.setDefaultTimeout(8000)
    const errors = [], writes = []
    let migration = sourceMigrationFixture(true)
    const servers = [1, 2].map(id => ({ id, name: `受管服务器 ${id}`, enabled: true, online: true, agent_supported: true, read_only: false }))
    const nodes = [1, 2].map(id => ({ id, name: `受管监听 ${id}`, server_id: id, protocol: 'vless-reality', enabled: true, public_host: `managed${id}.example.com`, sni: 'www.example.com', port: 20000 + id }))
    const numbered = { id: 10, name: '旧机场', kind: 'url', source_host: 'old.example.com', url_configured: true, authorization_configured: false, content_configured: false, settings_revision: 1, identity_epoch: 1, refresh_interval_seconds: 86400, auto_refresh: true, user_agent: 'Apsp-subscription-import/1', traffic: {}, changes: { added: 0, updated: 0, missing: 0, unsupported: 0 }, archived: false, current_revision_id: 5, last_attempt_at: 1790860800, last_success_at: 1790860800, last_error: null, supported_count: 1, unsupported_count: 0, active_job_id: null, dependency_ids: [], migrated_to: 1 }
    const numberedNode = { id: 501, source_id: 10, node_version_id: 601, source_revision_id: 5, identity_epoch: 1, name: '旧机场节点', protocol: 'trojan', server: 'exit.example.com', port: 443, transport: 'tcp', tcp: true, udp: false, selectable: true, present: true, identity_unique: true, adopted: true, metadata_revision: 1, reason: null }
    const ordered = subscriptionSourceFixture({ name: '旧机场', latest_success: sourceRevisionFixture() })
    page.on('pageerror', error => errors.push(error.message))
    page.on('request', request => { if (!request.url().startsWith(origin)) errors.push(`Unexpected outbound URL: ${request.url()}`) })
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname, method = request.method()
      if (method !== 'GET') { writes.push(`${method} ${path}`); await route.fulfill({ status: 409, json: { error: '夹具拒绝写入' } }); return }
      let value
      if (path === '/api/dashboard/access') value = { authenticated: true, public_dashboard: false }
      else if (path === '/api/me') value = { authenticated: true }
      else if (path === `${prefix}/servers`) value = servers
      else if (path === `${prefix}/nodes`) value = nodes
      else if (path === `${prefix}/proxy-resources`) value = flatResourceFixtures(nodes, servers)
      else if (path === `${prefix}/node-catalog`) value = catalogResourceFixtures(flatResourceFixtures(nodes, servers))
      else if (path === `${prefix}/ordered-proxy-resources`) value = proxyResourceFixtures(nodes, servers)
      else if (path === `${prefix}/usage`) value = { total: '0', uplink: '0', downlink: '0', by_node: [], by_user: [] }
      else if (path === `${prefix}/source-migration`) {
        if (migration === 'error') { await route.fulfill({ status: 503, json: { error: '迁移状态夹具不可用' } }); return }
        value = migration
      } else if (path === `${prefix}/subscription-sources`) value = [numbered]
      else if (path === `${prefix}/subscription-sources/10/nodes`) value = [numberedNode]
      else if (path === `${prefix}/ordered-subscription-sources`) value = [ordered]
      else if (path === `${prefix}/ordered-subscription-sources/1`) value = ordered
      else if (path === `${prefix}/ordered-subscription-sources/1/nodes`) value = sourceNodePageFixture()
      else if (path === `${prefix}/ordered-subscription-sources/1/revisions`) value = { source_id: 1, revisions: [sourceRevisionFixture()] }
      else { errors.push(`Unexpected API: ${method} ${path}`); await route.fulfill({ status: 404, json: { error: '夹具拒绝未知接口' } }); return }
      await route.fulfill({ json: value })
    })
    const manager = page.getByRole('region', { name: '订阅来源管理', exact: true }), archive = page.locator('details[data-slot=source-archive]')
    const numberedPanel = page.locator('section[data-slot=subscription-sources]')

    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/plugins/sing-box/nodes?view=sources`)
    await manager.getByRole('heading', { level: 2 }).filter({ hasText: /^订阅来源/ }).waitFor()
    assert.equal(await manager.getByRole('heading', { level: 2 }).filter({ hasText: '有序链路' }).count(), 0)
    assert.equal(await archive.getAttribute('open'), null, 'the archive starts collapsed')
    await archive.locator('summary').click()
    await numberedPanel.getByRole('heading', { name: '旧版数字编号来源（只读保留）', exact: true }).waitFor()
    await numberedPanel.getByText('已迁移为订阅来源 #1', { exact: true }).waitFor()
    for (const name of ['添加来源', '设置与更新', '立即更新', '归档', '删除']) assert.equal(await numberedPanel.getByRole('button', { name, exact: true }).count(), 0, name)
    await numberedPanel.getByRole('button', { name: '查看节点', exact: true }).click()
    await numberedPanel.getByText('旧机场节点', { exact: true }).waitFor()
    await numberedPanel.getByText('已加入', { exact: true }).waitFor()
    assert.equal(await numberedPanel.getByRole('button', { name: /节点库/ }).count(), 0)

    // After the migration the page header opens the ordered chain form in the
    // chain view; the mixed editor stays closed.
    const header = page.locator('header[data-slot=page-header]').getByRole('button', { name: '创建链路', exact: true })
    await header.click()
    const orderedForm = page.getByRole('dialog', { name: '创建链路', exact: true })
    await orderedForm.waitFor()
    assert.match(page.url(), /view=chains/)
    assert.equal(await page.getByRole('region', { name: '创建链路', exact: true }).count(), 0)
    await orderedForm.getByRole('button', { name: '取消', exact: true }).click()
    await orderedForm.waitFor({ state: 'detached' })
    await page.goto(`${origin}/#/plugins/sing-box/nodes?view=sources`)
    await manager.getByRole('heading', { level: 2 }).filter({ hasText: /^订阅来源/ }).waitFor()

    // Unknown state: the error is shown and numbered-source writes stay blocked.
    migration = 'error'
    await page.reload()
    await page.getByText('订阅来源迁移状态读取失败', { exact: false }).first().waitFor()
    await numberedPanel.getByRole('heading', { name: '订阅来源', exact: true }).waitFor()
    assert.equal(await numberedPanel.getByRole('button', { name: '添加来源', exact: true }).isDisabled(), true)
    assert.equal(await archive.count(), 0)
    assert.equal(await header.isDisabled(), false, 'the ordered chain form does not depend on the migration state')

    // Before the migration both panels are writable as before.
    migration = sourceMigrationFixture(false)
    await page.reload()
    const add = numberedPanel.getByRole('button', { name: '添加来源', exact: true })
    await add.waitFor(); const deadline = Date.now() + 8000
    while (await add.isDisabled() && Date.now() < deadline) await page.waitForTimeout(20)
    assert.equal(await add.isDisabled(), false)
    await manager.getByRole('heading', { level: 2 }).filter({ hasText: '有序链路订阅来源' }).waitFor()
    // Before the migration the panel explains what each source list feeds, and the
    // header opens the same ordered form as after it (ADR 0098); no mixed editor remains.
    await numberedPanel.locator('xpath=..').getByText('订阅来源尚未合并。', { exact: false }).first().waitFor()
    while (await header.isDisabled() && Date.now() < deadline) await page.waitForTimeout(20)
    const preForm = page.getByRole('dialog', { name: '创建链路', exact: true })
    for (let attempt = 0; attempt < 5 && !(await preForm.isVisible()); attempt++) { await header.click(); await preForm.waitFor({ timeout: 1500 }).catch(() => {}) }
    await preForm.waitFor()
    assert.equal(await page.getByRole('region', { name: '创建链路', exact: true }).count(), 0)
    await preForm.getByRole('button', { name: '取消', exact: true }).click()
    await preForm.waitFor({ state: 'detached' })

    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), true)
    assert.deepEqual(writes, [])
    assert.deepEqual(errors, [])
    await page.close()
  }
  console.log('PASS: migrated numbered sources are a read-only archive, the header always opens the ordered form, unknown migration state blocks numbered writes but not the ordered form, pre-migration panels stay writable and explain their roles, desktop/mobile')
} finally { if (browser) await browser.close(); await new Promise(resolve => server.close(resolve)) }
