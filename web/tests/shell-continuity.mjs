import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import { proxyResourceFixtures } from './proxy-resource-fixtures.mjs'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/shell-continuity.mjs`.
// The console shell must stay usable as the viewport and the address change:
// phones scroll to the end of long pages; crossing the desktop breakpoint keeps
// open dialogs and typed input; leaving unsaved input always asks first, whether
// through a link, the command palette, code or the browser history; and English
// labels follow live data instead of the first value they were translated from.
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
const prefix = '/api/plugins/sing-box'
const GiB = 1024 ** 3
const serverFixture = id => {
  const now = Date.now()
  return {
    id, name: `TEST_ONLY 服务器 ${id}`, online: true, metrics_stale: false, device_public_key: 'TEST_ONLY', registered: true, manifest_rev: 0,
    metrics_sampled_at: now, last_seen: now / 1000, last_heartbeat_at: now / 1000,
    static_info: { system: 'Ubuntu 24.04', arch: 'amd64', cpu_cores: 2, memory_total: 4 * GiB, disk_total: 32 * GiB, hostname: `fixture-${id}` },
    latest_metrics: { cpu_percent: 20, memory_used: GiB, disk_used: 4 * GiB, uptime_secs: 3600, network_interfaces: {} },
    asset_settings: { region: 'JP', group_name: '亚洲', tags: [], hidden: false, price: '0', currency: 'CNY', billing_cycle: 30, expires_at: null, auto_renewal: false, traffic_limit: '0', traffic_limit_type: 'sum', reset_day: 1, network_interface: '' },
  }
}
// Proxy users render their detail beside the list through the shared list/detail layout.
const user = { id: 1, name: 'TEST_ONLY 代理用户', subscription_token: 'TEST_ONLY', subscription_url: 'http://127.0.0.1/s/TEST_ONLY' }
const node = { enabled: true, id: 1, name: 'TEST_ONLY 节点', server_id: 1, protocol: 'vless-reality', public_host: '127.0.0.1', port: 24443, sni: 'localhost', public_key: 'TEST_ONLY', short_id: '0123abcd' }
const pluginServers = [{ id: 1, name: 'TEST_ONLY 服务器', enabled: true, online: true, read_only: false }]
const proxyReads = {
  [`${prefix}/policy-groups`]: [], [`${prefix}/package-groups`]: [], [`${prefix}/nodes`]: [node], [`${prefix}/users`]: [user],
  [`${prefix}/proxy-resources`]: [], [`${prefix}/ordered-proxy-resources`]: proxyResourceFixtures([node], pluginServers),
  [`${prefix}/usage`]: { uplink: '0', downlink: '0', total: '0', by_user: [], by_node: [] },
  [`${prefix}/users/1/accesses`]: [], [`${prefix}/users/1/external-accesses`]: { revision: 0, accesses: [], available_nodes: [] },
  [`${prefix}/users/1/policy-groups`]: { group_ids: [] },
  [`${prefix}/users/1/entitlement`]: { user_id: 1, package_group_id: null, package_name: null, monthly_bytes: null, starts_at: null, expires_at: null, cycle_start: null, next_reset: null, used_bytes: '0', status: 'unlimited', allowed: true },
  [`${prefix}/users/1/portal`]: { configuration: { enabled: false, reason: 'TEST_ONLY 尚未启用', origin: 'http://127.0.0.1' }, keys: 0, url: null, activation_expires_at: null },
  [`${prefix}/users/1/diagnosis`]: { user_id: 1, account: { user_id: 1, name: user.name, portal_created: false, keys: 0, active_sessions: 0, activation_expires_at: null },
    subscription: { status: 'empty', message: 'TEST_ONLY 真实设备状态未验证。', granted_nodes: 0, ready_managed_nodes: 0, ready_external_nodes: 0 },
    permissions: [], external_authorizations: [], ledger: [], quota_credits: [], package_history: [], rotations: [], events: [],
    limitations: { credentials_read: { available: false, reason: 'TEST_ONLY 敏感内容未读取。' } } },
  [`${prefix}/users/1/client-template`]: { template: null, definition_redacted: false, credential_access_reason: 'TEST_ONLY 完整模板未读取。', supported_client: 'singbox', supported_version: '1.14.2', schema_validation: true, runtime_validation: false, limitations: 'TEST_ONLY 没有保存的模板。' },
}

async function open(width, height, state) {
  const page = await browser.newPage({ viewport: { width, height } })
  const errors = [], prompts = []
  page.on('pageerror', error => errors.push(error.message))
  // Each leave confirmation is answered by the scenario that expects it.
  page.on('dialog', dialog => { prompts.push(dialog.message()); void (state.answer?.shift() ? dialog.accept() : dialog.dismiss()) })
  await page.route('**/api/**', route => {
    const path = new URL(route.request().url()).pathname
    const json = (body, status = 200) => route.fulfill({ status, json: body })
    if (route.request().method() !== 'GET') return json({ error: 'TEST_ONLY 此夹具不接受写入。' }, 400)
    if (path === '/api/dashboard/access') return json({ authenticated: true, public_dashboard: false })
    if (path === '/api/servers') return json(state.servers ?? [])
    if (path.startsWith('/api/network-workbench/')) return json([])
    if (path in proxyReads) return json(proxyReads[path])
    return json({ error: 'TEST_ONLY 此夹具未提供该数据。' }, 404)
  })
  await installControlCenterFixtures(page)
  return { page, errors, prompts }
}
const until = async (condition, message, limit = 5000) => {
  const deadline = Date.now() + limit
  while (!await condition()) { assert(Date.now() < deadline, message); await new Promise(done => setTimeout(done, 40)) }
}
const hash = page => page.evaluate(() => location.hash)
const palette = page => page.getByRole('dialog', { name: '搜索或跳转' })

const summary = []
try {
  // Phones: the main column owns vertical scrolling, so wheel and touch reach the
  // end of a long list and the footer, also while the menu is open.
  {
    const state = { servers: Array.from({ length: 30 }, (_, index) => serverFixture(index + 1)) }
    const { page, errors } = await open(390, 900, state)
    try {
      await page.goto(`${origin}/#/servers`)
      const cards = page.locator('[data-slot=server-list-card]')
      await cards.first().waitFor()
      const scroller = page.locator('[data-slot=main-scroller]')
      const metrics = () => scroller.evaluate(element => ({ top: element.scrollTop, height: element.scrollHeight, client: element.clientHeight }))
      const start = await metrics()
      assert.equal(start.height > start.client * 2, true, 'Thirty servers are longer than the phone screen')
      await page.mouse.move(195, 640)
      for (let step = 0; step < 40 && (await metrics()).top + start.client < start.height - 1; step++) { await page.mouse.wheel(0, 1200); await page.waitForTimeout(60) }
      const end = await metrics()
      assert.equal(end.top + end.client >= end.height - 1, true, 'Wheel scrolling reaches the end of the page')
      const last = await cards.last().boundingBox()
      assert.equal(last.y >= 0 && last.y + last.height <= 900, true, 'The last server card can be brought on screen')
      const footer = await page.getByText('清晰掌握，自在连接。').boundingBox()
      assert.equal(footer.y + footer.height <= 900, true, 'The footer is reachable')
      // Back at the top, bulk actions for a selection stay on screen above the rest of the list.
      await page.mouse.wheel(0, -100000); await until(async () => (await metrics()).top === 0, 'Scrolls back to the top')
      await cards.first().getByRole('checkbox').click()
      const bar = page.locator('[data-slot=bulk-bar]')
      await bar.waitFor()
      const barBox = await bar.boundingBox()
      assert.equal(barBox.y >= 0 && barBox.y + barBox.height <= 900, true, 'The bulk action bar is reachable on a phone')
      await bar.getByRole('button', { name: '清空选择', exact: true }).click(); await bar.waitFor({ state: 'hidden' })
      // The open menu takes part of the screen; the page below it still scrolls.
      await page.getByRole('button', { name: '菜单', exact: true }).click()
      await page.locator('#admin-navigation').getByRole('link', { name: '服务器列表', exact: true }).waitFor()
      const menu = await metrics()
      assert.equal(menu.client > 0 && menu.height > menu.client, true, 'The page keeps a scrolling area below the open menu')
      await page.mouse.move(195, 880); await page.mouse.wheel(0, 600)
      await until(async () => (await metrics()).top > 0, 'The page below the open menu scrolls')
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'phone-scroll', height: end.height, client: end.client })
    } finally { await page.close() }
  }

  // Crossing the breakpoint keeps the page mounted: open dialogs and typed input survive.
  {
    const state = { servers: [serverFixture(1), serverFixture(2), serverFixture(3)] }
    const { page, errors } = await open(1440, 900, state)
    try {
      await page.goto(`${origin}/#/servers`)
      await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
      await page.locator('header[data-slot=page-header]').getByRole('button', { name: '添加服务器', exact: true }).click()
      const dialog = page.getByRole('dialog'), name = dialog.getByLabel('服务器名称')
      await name.fill('TEST_ONLY 未保存的服务器')
      for (const width of [390, 1440, 820, 1440]) {
        await page.setViewportSize({ width, height: 900 })
        await page.waitForTimeout(250)
        assert.equal(await dialog.isVisible(), true, `The add-server dialog stays open at ${width}px`)
        assert.equal(await name.inputValue(), 'TEST_ONLY 未保存的服务器', `Typed input survives at ${width}px`)
      }
      // The proxy users list/detail keeps its detail dialog across the breakpoint too.
      await dialog.getByRole('button', { name: '取消', exact: true }).click(); await dialog.waitFor({ state: 'hidden' })
      await page.goto(`${origin}/#/plugins/sing-box/users`)
      const edit = page.locator('[data-slot=user-heading]').getByRole('button', { name: '编辑', exact: true })
      await until(() => edit.isEnabled(), 'The proxy user detail is ready')
      await edit.click()
      const userName = page.getByRole('dialog').locator('input[name="name"]')
      await userName.fill('TEST_ONLY 未保存的用户名')
      for (const width of [390, 1440]) {
        await page.setViewportSize({ width, height: 900 })
        await page.waitForTimeout(250)
        assert.equal(await userName.inputValue(), 'TEST_ONLY 未保存的用户名', `The proxy user draft survives at ${width}px`)
      }
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'breakpoint' })
    } finally { await page.close() }
  }

  // Unsaved input asks before any way of leaving; declining keeps page and input.
  {
    const state = { servers: [serverFixture(1)], answer: [] }
    const { page, errors, prompts } = await open(1440, 900, state)
    try {
      await page.goto(`${origin}/#/monitoring/settings`)
      await page.getByRole('heading', { name: '告警设置', exact: true }).waitFor()
      await page.getByRole('navigation', { name: '主导航' }).getByRole('link', { name: '服务器列表', exact: true }).click()
      await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
      await page.locator('header[data-slot=page-header]').getByRole('button', { name: '添加服务器', exact: true }).click()
      const dialog = page.getByRole('dialog').filter({ has: page.getByLabel('服务器名称') }), name = dialog.getByLabel('服务器名称')
      await name.fill('TEST_ONLY 待确认离开')

      // The palette never opens over another dialog's form.
      await name.press('Control+k'); await name.evaluate(element => element.blur()); await page.keyboard.press('/')
      await page.waitForTimeout(150)
      assert.equal(await palette(page).count(), 0, 'Shortcuts do not open the palette over an open dialog')

      // Code that writes the address directly is caught by the router.
      state.answer.push(false)
      await page.evaluate(() => { location.hash = '#/monitoring/settings' })
      await until(() => prompts.length === 1, 'Changing the address asks first')
      await until(async () => await hash(page) === '#/servers', 'Declining restores the address')
      assert.equal(await name.inputValue(), 'TEST_ONLY 待确认离开', 'Declining keeps the input')

      // Browser back asks too; declining stays, accepting leaves.
      state.answer.push(false)
      await page.evaluate(() => history.back())
      await until(() => prompts.length === 2, 'Back asks first')
      await until(async () => await hash(page) === '#/servers', 'Declining back stays on the page')
      assert.equal(await name.inputValue(), 'TEST_ONLY 待确认离开')
      state.answer.push(true)
      await page.evaluate(() => history.back())
      await until(() => prompts.length === 3, 'Back asks again')
      await page.getByRole('heading', { name: '告警设置', exact: true }).waitFor()
      assert.equal(await dialog.count(), 0)
      assert.equal(prompts.every(message => message.includes('未保存')), true)
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'leave-dialog', prompts: prompts.length })
    } finally { await page.close() }
  }

  // An inline draft: the palette and sidebar links ask before replacing the page.
  {
    const state = { servers: [serverFixture(1)], answer: [] }
    const { page, errors, prompts } = await open(1440, 900, state)
    try {
      await page.goto(`${origin}/#/network-workbench`)
      const planName = page.getByLabel('方案名称')
      await planName.waitFor()
      await planName.fill('TEST_ONLY 未保存方案')
      const jump = async () => {
        await page.locator('[data-slot=topbar]').getByRole('button', { name: '全局搜索', exact: true }).click()
        const input = palette(page).getByRole('combobox', { name: '搜索页面或对象' })
        await input.fill('告警设置')
        await palette(page).getByRole('option', { name: /告警设置/ }).first().waitFor()
        await input.press('Enter')
      }
      state.answer.push(false)
      await jump()
      await until(() => prompts.length === 1, 'The palette asks before leaving unsaved input')
      await palette(page).waitFor({ state: 'hidden' })
      assert.equal(await hash(page), '#/network-workbench')
      assert.equal(await planName.inputValue(), 'TEST_ONLY 未保存方案', 'Declining the palette jump keeps the input')
      state.answer.push(false)
      await page.getByRole('navigation', { name: '主导航' }).getByRole('link', { name: '告警设置', exact: true }).click()
      await until(() => prompts.length === 2, 'A sidebar link asks first')
      assert.equal(await hash(page), '#/network-workbench')
      assert.equal(await planName.inputValue(), 'TEST_ONLY 未保存方案')
      state.answer.push(true)
      await jump()
      await page.getByRole('heading', { name: '告警设置', exact: true }).waitFor()
      assert.equal(prompts.length, 3, 'An accepted palette jump asks exactly once')
      assert.equal(await hash(page), '#/monitoring/settings')
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'leave-inline', prompts: prompts.length })
    } finally { await page.close() }
  }

  // Text widgets that are not form fields (the remote terminal) keep "/" as input.
  {
    const state = { servers: [serverFixture(1)] }
    const { page, errors } = await open(1440, 900, state)
    try {
      await page.goto(`${origin}/#/servers`)
      await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
      await page.evaluate(() => { const box = document.createElement('div'); box.id = 'test-textbox'; box.tabIndex = 0; box.setAttribute('role', 'textbox'); box.textContent = 'TEST_ONLY'; document.querySelector('main').append(box) })
      await page.locator('#test-textbox').focus(); await page.keyboard.press('/')
      await page.waitForTimeout(150)
      assert.equal(await palette(page).count(), 0, '"/" inside a text widget stays input')
      await page.locator('#test-textbox').evaluate(element => element.blur()); await page.keyboard.press('/')
      await palette(page).waitFor()
      await page.keyboard.press('Escape'); await palette(page).waitFor({ state: 'hidden' })
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'terminal-slash' })
    } finally { await page.close() }
  }

  // English labels follow the data: a server going offline reads "Offline".
  {
    const state = { servers: [serverFixture(1), serverFixture(2)] }
    const { page, errors } = await open(1440, 900, state)
    try {
      await page.goto(`${origin}/#/servers`)
      const language = page.locator('[data-slot=topbar] select')
      await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
      await language.selectOption('en-US')
      const row = page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' })
      await row.getByText('Online', { exact: true }).waitFor()
      state.servers[0].online = false
      await page.locator('header[data-slot=page-header]').getByRole('button', { name: 'Refresh', exact: true }).click()
      await row.getByText('Offline', { exact: true }).waitFor()
      assert.equal(await row.getByText('Online', { exact: true }).count(), 0, 'The old English status does not come back')
      // Text the DOM translator handles follows later writes as well, for text and attributes.
      await page.evaluate(() => { const probe = document.createElement('span'); probe.id = 'test-i18n'; probe.textContent = '在线'; probe.setAttribute('aria-label', '在线'); document.body.append(probe) })
      const probe = page.locator('#test-i18n')
      await until(async () => await probe.textContent() === 'Online' && await probe.getAttribute('aria-label') === 'Online', 'Static text is translated')
      await probe.evaluate(element => { element.firstChild.nodeValue = '离线'; element.setAttribute('aria-label', '离线') })
      await until(async () => await probe.textContent() === 'Offline' && await probe.getAttribute('aria-label') === 'Offline', 'Rewritten text is translated from its new value')
      await language.selectOption('zh-CN')
      await row.getByText('离线', { exact: true }).waitFor()
      await until(async () => await probe.textContent() === '离线' && await probe.getAttribute('aria-label') === '离线', 'Chinese restores the latest value')
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'english-status' })
    } finally { await page.close() }
  }
  console.log(JSON.stringify({ passed: true, scenarios: summary }, null, 2))
} finally {
  await browser.close()
  await new Promise(done => http.close(done))
}
