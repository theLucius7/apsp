import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/interaction-consistency.mjs`.
// Controls that look alike behave alike: a language picked in the header is kept
// and saved; each entry point owns the document theme; closing a dialog returns
// focus to what opened it, or to the page when that is gone; the server list
// tells totals, filter results, a first failed read and a stale refresh apart;
// the phone menu closes on Escape and outside presses; multiple selects show
// several rows; and member bulk changes never touch members the search hides.
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
const commerce = '/api/plugins/sing-box/commerce'
const GiB = 1024 ** 3
const serverFixture = (id, online = true) => {
  const now = Date.now()
  return {
    id, name: `TEST_ONLY 服务器 ${id}`, online, metrics_stale: false, device_public_key: 'TEST_ONLY', registered: true, manifest_rev: 0,
    metrics_sampled_at: now, last_seen: now / 1000, last_heartbeat_at: now / 1000,
    static_info: { system: 'Ubuntu 24.04', arch: 'amd64', cpu_cores: 2, memory_total: 4 * GiB, disk_total: 32 * GiB, hostname: `fixture-${id}` },
    latest_metrics: { cpu_percent: 20, memory_used: GiB, disk_used: 4 * GiB, uptime_secs: 3600, network_interfaces: {} },
    asset_settings: { region: 'JP', group_name: '亚洲', tags: [], hidden: false, price: '0', currency: 'CNY', billing_cycle: 30, expires_at: null, auto_renewal: false, traffic_limit: '0', traffic_limit_type: 'sum', reset_day: 1, network_interface: '' },
  }
}
const member = (id, name) => ({ user_id: id, name, email: `member-${id}@example.com`, state: 'active', balance_cents: 0, version: id * 10 })
const rates = { base: 'CNY', rates: { CNY: 1 }, rate_dates: {}, rate_date: null, source: null, source_url: null, fetched_at: null, attempted_at: null, next_refresh_at: 0, stale: true, status: 'unavailable', error_code: null }

async function open(width, state) {
  const page = await browser.newPage({ viewport: { width, height: 900 }, colorScheme: 'light' })
  const errors = [], writes = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/api/**', route => {
    const request = route.request(), path = new URL(request.url()).pathname, method = request.method()
    const json = (body, status = 200) => route.fulfill({ status, json: body })
    if (method !== 'GET') {
      writes.push({ method, path, body: request.postDataJSON() })
      return path === `${commerce}/members/bulk` ? json({}) : json({ error: 'TEST_ONLY 此夹具不接受该写入。' }, 400)
    }
    if (path === '/api/dashboard/access') return json({ authenticated: true, public_dashboard: false })
    if (path === '/api/servers' || path === '/api/dashboard/servers') return state.failServers ? json({ error: 'TEST_ONLY 读取失败' }, 503) : json(state.servers ?? [])
    if (path === '/api/dashboard/exchange-rates') return json(rates)
    if (path === '/api/dashboard/probes/overview' || path.startsWith('/api/network-workbench/')) return json([])
    if (path === `${commerce}/overview`) return json({ members: 3, pending_members: 0, balance_cents: '0', orders: 0, sales_cents: '0', open_exceptions: 0, invitations: 0, ledger_entries: 0 })
    if (path === `${commerce}/members`) return json({ entries: state.members ?? [], total: state.members?.length ?? 0 })
    return json({ error: 'TEST_ONLY 此夹具未提供该数据。' }, 404)
  })
  await installControlCenterFixtures(page)
  // The administrator's saved appearance, read and written with revisions like the panel.
  await page.route('**/api/control-center/preferences/appearance', route => {
    const request = route.request()
    if (request.method() === 'GET') return route.fulfill({ json: { ...state.appearance, updated_at: 0 } })
    const body = request.postDataJSON()
    writes.push({ method: request.method(), path: 'appearance', body })
    if (body.expected_revision !== state.appearance.revision) return route.fulfill({ status: 409, json: { error: 'TEST_ONLY 版本冲突' } })
    state.appearance = { value: body.value, revision: state.appearance.revision + 1 }
    return route.fulfill({ json: { revision: state.appearance.revision } })
  })
  return { page, errors, writes }
}
const until = async (condition, message, limit = 5000) => {
  const deadline = Date.now() + limit
  while (!await condition()) { assert(Date.now() < deadline, message); await new Promise(done => setTimeout(done, 40)) }
}
const html = page => page.evaluate(() => ({ theme: document.documentElement.dataset.theme, density: document.documentElement.dataset.density, lang: document.documentElement.lang }))
const focused = page => page.evaluate(() => { const element = document.activeElement; return { tag: element?.tagName, slot: element?.getAttribute('data-slot'), label: element?.getAttribute('aria-label'), text: element?.textContent?.trim() } })
// Relative luminance of the document background, 0 (black) to 1 (white).
const backgroundLuminance = page => page.evaluate(() => {
  const probe = document.createElement('canvas').getContext('2d')
  probe.fillStyle = getComputedStyle(document.body).backgroundColor
  probe.fillRect(0, 0, 1, 1)
  const [r, g, b] = probe.getImageData(0, 0, 1, 1).data
  const channel = value => { const c = value / 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4 }
  return 0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
})
const addServer = page => page.locator('header[data-slot=page-header]').getByRole('button', { name: '添加服务器', exact: true })

const summary = []
try {
  // A language picked in the header stays, is saved, and applies in a new session (E30).
  {
    const state = { servers: [serverFixture(1)], appearance: { value: { theme: 'light', density: 'comfortable', locale: 'zh-CN' }, revision: 1 } }
    const { page, errors, writes } = await open(1440, state)
    try {
      await page.goto(`${origin}/#/servers`)
      await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
      const language = page.locator('[data-slot=topbar] select')
      await language.selectOption('en-US')
      await until(() => writes.some(write => write.path === 'appearance'), 'The header choice is saved')
      assert.deepEqual(writes.find(write => write.path === 'appearance').body, { value: { theme: 'light', density: 'comfortable', locale: 'en-US' }, expected_revision: 1 })
      await page.getByRole('heading', { name: 'Server list', exact: true }).waitFor()
      await page.waitForTimeout(600)
      assert.equal(await language.inputValue(), 'en-US', 'The saved Chinese preference does not undo the header choice')
      assert.equal((await html(page)).lang, 'en-US')
      assert.deepEqual(errors, [])
    } finally { await page.close() }
    // Another browser starts from the saved preference, not from its own storage.
    const next = await open(1440, state)
    try {
      await next.page.goto(`${origin}/#/servers`)
      await next.page.getByRole('heading', { name: 'Server list', exact: true }).waitFor()
      assert.equal(await next.page.locator('[data-slot=topbar] select').inputValue(), 'en-US')
      assert.deepEqual(next.writes, [], 'Applying the saved language writes nothing')
      assert.deepEqual(next.errors, [])
    } finally { await next.page.close() }
    summary.push({ scenario: 'language', revision: state.appearance.revision })
  }

  // Each entry point owns the document theme: the console's dark preference never
  // reaches the light dashboard, and returning to the console restores it.
  {
    const state = { servers: [serverFixture(1)], appearance: { value: { theme: 'dark', density: 'compact' }, revision: 1 } }
    const { page, errors } = await open(1440, state)
    try {
      await page.goto(`${origin}/#/servers`)
      await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
      await until(async () => (await html(page)).theme === 'dark', 'The console applies the dark preference')
      assert.equal((await html(page)).density, 'compact')
      assert.equal(await backgroundLuminance(page) < 0.2, true, 'The console background is dark')
      await page.evaluate(() => { location.hash = '#/dashboard' })
      const toggle = page.getByRole('button', { name: '切换深色主题', exact: true })
      await toggle.waitFor()
      const { theme, density } = await html(page)
      assert.deepEqual({ theme, density }, { theme: 'light', density: 'comfortable' }, 'The dashboard owns the document theme and density')
      assert.equal(await backgroundLuminance(page) > 0.8, true, 'The dashboard background is light')
      await toggle.click()
      await page.getByRole('button', { name: '切换浅色主题', exact: true }).waitFor()
      assert.equal((await html(page)).theme, 'dark')
      await page.getByRole('button', { name: '切换浅色主题', exact: true }).click()
      await until(async () => (await html(page)).theme === 'light', 'The dashboard switches back to light')
      await page.evaluate(() => { location.hash = '#/servers' })
      await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
      await until(async () => (await html(page)).theme === 'dark' && (await html(page)).density === 'compact', 'Returning to the console restores its preference')
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'theme-entries' })
    } finally { await page.close() }
  }

  // Closing a dialog returns focus to its opener, or to the page when the opener is gone.
  {
    const state = { servers: [serverFixture(1), serverFixture(2)], appearance: { value: null, revision: 0 } }
    const { page, errors } = await open(1440, state)
    try {
      await page.goto(`${origin}/#/servers`)
      await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).waitFor()
      const palette = page.getByRole('dialog', { name: '搜索或跳转' })
      await page.locator('[data-slot=topbar]').getByRole('button', { name: '全局搜索', exact: true }).click()
      await palette.waitFor(); await page.keyboard.press('Escape'); await palette.waitFor({ state: 'hidden' })
      await until(async () => (await focused(page)).slot === 'global-search', 'The palette returns focus to its button')
      await addServer(page).focus(); await page.keyboard.press('Control+k')
      await palette.waitFor(); await page.keyboard.press('Escape'); await palette.waitFor({ state: 'hidden' })
      await until(async () => (await focused(page)).text === '添加服务器', 'The palette returns focus to where the shortcut was pressed')
      await addServer(page).click()
      const dialog = page.getByRole('dialog').filter({ has: page.getByLabel('服务器名称') })
      await dialog.waitFor(); await page.keyboard.press('Escape'); await dialog.waitFor({ state: 'hidden' })
      await until(async () => (await focused(page)).text === '添加服务器', 'The add-server dialog returns focus to its button')
      // The desktop row that opened the editor is hidden in the phone layout.
      await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 1' }).getByRole('button', { name: '编辑', exact: true }).click()
      const editor = page.getByRole('dialog')
      await editor.waitFor()
      await page.setViewportSize({ width: 390, height: 900 }); await page.waitForTimeout(250)
      await page.keyboard.press('Escape'); await editor.waitFor({ state: 'hidden' })
      await until(async () => (await focused(page)).tag === 'MAIN', 'Focus falls back to the page when the opener cannot take it')
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'focus-return' })
    } finally { await page.close() }
  }

  // Totals, filter results, the first read failing and a stale refresh read differently.
  {
    const state = { servers: [serverFixture(1), serverFixture(2), serverFixture(3, false)], failServers: true, appearance: { value: null, revision: 0 } }
    const { page, errors } = await open(1440, state)
    try {
      await page.goto(`${origin}/#/servers`)
      await page.getByText('无法读取服务器列表', { exact: true }).waitFor()
      assert.equal(await page.getByText('从第一台服务器开始', { exact: true }).count(), 0, 'A failed first read is not shown as an empty panel')
      state.failServers = false
      await page.getByRole('button', { name: '重试', exact: true }).last().click()
      await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 3' }).waitFor()
      const count = page.locator('[data-slot=server-count]'), title = count.locator('..')
      assert.equal(await count.textContent(), '3')
      const search = page.getByRole('searchbox', { name: '筛选名称、标签或地区' })
      await search.fill('TEST_ONLY 服务器 1')
      await until(async () => await count.textContent() === '1 / 3', 'Filtered results show matches out of all servers')
      assert.match(await title.textContent(), /^筛选结果/)
      const total = page.locator('[data-slot=stat]').filter({ hasText: '服务器总数' }).locator('strong')
      assert.equal(await total.textContent(), '3', 'The total counts every server under a filter')
      await search.fill('TEST_ONLY 不存在')
      await page.getByText('没有符合条件的服务器', { exact: true }).waitFor()
      assert.equal(await page.getByText('从第一台服务器开始', { exact: true }).count(), 0, 'No match is not the first-run state')
      await page.getByRole('button', { name: '清除筛选', exact: true }).click()
      await until(async () => await count.textContent() === '3', 'Clearing filters shows every server')
      assert.equal(await search.inputValue(), '')
      assert.match(await title.textContent(), /^全部服务器/)
      // A failed background refresh keeps the last data and says it may be out of date.
      state.failServers = true
      await page.locator('header[data-slot=page-header]').getByRole('button', { name: '刷新', exact: true }).click()
      await page.getByRole('status').filter({ hasText: '最近一次刷新失败' }).waitFor()
      assert.equal(await page.getByRole('row').filter({ hasText: 'TEST_ONLY 服务器 3' }).count(), 1, 'The last good data stays visible')
      state.failServers = false; state.servers = []
      await page.locator('header[data-slot=page-header]').getByRole('button', { name: '刷新', exact: true }).click()
      await page.getByText('从第一台服务器开始', { exact: true }).waitFor()
      assert.equal(await page.getByRole('status').filter({ hasText: '最近一次刷新失败' }).count(), 0)
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'list-states' })
    } finally { await page.close() }
  }

  // The phone menu closes on Escape (focus back on its button) and on a press outside (E53).
  {
    const state = { servers: [serverFixture(1)], appearance: { value: null, revision: 0 } }
    const { page, errors } = await open(390, state)
    try {
      await page.goto(`${origin}/#/servers`)
      await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
      const button = page.getByRole('button', { name: '菜单', exact: true }), menu = page.locator('#admin-navigation')
      const expanded = async () => await button.getAttribute('aria-expanded') === 'true'
      await button.click(); await menu.getByRole('link', { name: '服务器列表', exact: true }).waitFor()
      await menu.getByRole('link', { name: '告警设置', exact: true }).focus()
      await page.keyboard.press('Escape')
      await until(async () => !await expanded(), 'Escape closes the menu')
      await until(async () => (await focused(page)).text?.includes('菜单'), 'Escape returns focus to the menu button')
      await button.click(); await until(expanded, 'The menu opens again')
      await page.getByRole('heading', { name: '服务器列表', exact: true }).click()
      await until(async () => !await expanded(), 'A press outside closes the menu')
      await button.click(); await until(expanded, 'The menu opens once more')
      await menu.getByText(/当前账号/).click()
      await page.waitForTimeout(150)
      assert.equal(await expanded(), true, 'A press inside the menu keeps it open')
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'phone-menu' })
    } finally { await page.close() }
  }

  // A multiple select shows several rows and no drop-down arrow (E33).
  {
    const state = { servers: [serverFixture(1), serverFixture(2), serverFixture(3)], appearance: { value: null, revision: 0 } }
    const { page, errors } = await open(1440, state)
    try {
      await page.goto(`${origin}/#/network-workbench`)
      await page.getByLabel('探测来源').selectOption('group')
      const list = page.getByLabel('来源服务器')
      await list.waitFor()
      assert.equal(await list.evaluate(element => element.multiple), true)
      const box = await list.boundingBox()
      assert.equal(box.height >= 90, true, `The list box shows several rows (${box.height}px)`)
      assert.equal(await list.locator('xpath=..').locator('[data-slot=native-select-icon]').count(), 0, 'No drop-down arrow on a list box')
      const single = page.getByLabel('探测来源')
      assert.equal((await single.boundingBox()).height < 48, true, 'Single selects keep their control height')
      assert.equal(await single.locator('xpath=..').locator('[data-slot=native-select-icon]').count(), 1)
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'multiple-select', height: box.height })
    } finally { await page.close() }
  }

  // Member bulk changes apply only to selected members the search shows (E52).
  {
    const state = { members: [member(1, 'TEST_ONLY 甲'), member(2, 'TEST_ONLY 乙'), member(3, 'TEST_ONLY 丙')], appearance: { value: null, revision: 0 } }
    const { page, errors, writes } = await open(1440, state)
    try {
      await page.goto(`${origin}/#/plugins/sing-box/commerce`)
      await page.getByRole('button', { name: '成员', exact: true }).click()
      await page.getByRole('checkbox', { name: '选择成员 TEST_ONLY 甲' }).check()
      await page.getByRole('checkbox', { name: '选择成员 TEST_ONLY 乙' }).check()
      await page.getByRole('button', { name: '批量修改状态（2）', exact: true }).waitFor()
      await page.getByRole('textbox', { name: '搜索当前页成员' }).fill('甲')
      await page.getByText('另有 1 位已选成员不在搜索结果中，批量操作不会修改。', { exact: true }).waitFor()
      await page.getByRole('button', { name: '批量修改状态（1）', exact: true }).click()
      const dialog = page.getByRole('dialog', { name: '批量修改成员状态' })
      assert.equal(await dialog.locator('[data-slot=bulk-targets]').textContent(), 'TEST_ONLY 甲（#1）')
      await dialog.getByText('另有 1 位已选成员不在当前搜索结果中，不会被修改。', { exact: true }).waitFor()
      await dialog.getByLabel('原因').fill('TEST_ONLY 批量暂停')
      await dialog.getByRole('button', { name: '保存', exact: true }).click()
      await dialog.waitFor({ state: 'hidden' })
      const bulk = writes.filter(write => write.path === `${commerce}/members/bulk`)
      assert.deepEqual(bulk.map(write => write.body), [{ members: [{ id: 1, version: 10 }], state: 'suspended', reason: 'TEST_ONLY 批量暂停' }])
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'member-bulk' })
    } finally { await page.close() }
  }
  console.log(JSON.stringify({ passed: true, scenarios: summary }, null, 2))
} finally {
  await browser.close()
  await new Promise(done => http.close(done))
}
