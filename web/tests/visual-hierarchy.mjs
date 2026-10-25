import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/visual-hierarchy.mjs`.
// The server list leads with its data: one toolbar row (search, status, more
// filters, view settings) instead of an expanded filter panel; extra filters show
// as removable chips; phones get a drawer and fewer card fields. Compact density
// tightens content but keeps control sizes. The proxy network leads the sidebar
// while addresses and the default page stay. Dialogs animate closed without
// blocking the page, and arrows are Lucide icons.
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
const serverFixture = (id, region, group) => {
  const now = Date.now()
  return {
    id, name: `TEST_ONLY ${region} ${id}`, online: id % 3 !== 0, metrics_stale: false, device_public_key: 'TEST_ONLY', registered: true, manifest_rev: 0,
    metrics_sampled_at: now, last_seen: now / 1000, last_heartbeat_at: now / 1000,
    static_info: { system: 'Ubuntu 24.04', arch: 'amd64', cpu_cores: 2, memory_total: 4 * GiB, disk_total: 32 * GiB, hostname: `fixture-${id}`, agent_version: '0.0.0-test' },
    latest_metrics: { cpu_percent: 10 * id, memory_used: GiB, disk_used: 4 * GiB, uptime_secs: 3600, network_interfaces: { eth0: { transmit_bytes_per_sec: 1024, receive_bytes_per_sec: 2048, transmitted_bytes: GiB, received_bytes: GiB } } },
    asset_settings: { region, group_name: group, tags: [], hidden: false, price: '0', currency: 'CNY', billing_cycle: 30, expires_at: null, auto_renewal: false, traffic_limit: '0', traffic_limit_type: 'sum', reset_day: 1, network_interface: '' },
  }
}
const fleet = () => [serverFixture(1, 'JP', '亚洲'), serverFixture(2, 'JP', '亚洲'), serverFixture(3, 'US', '美洲'), serverFixture(4, 'US', '美洲'), serverFixture(5, 'SG', '亚洲'), serverFixture(6, 'DE', '欧洲')]
const rates = { base: 'CNY', rates: { CNY: 1 }, rate_dates: {}, rate_date: null, source: null, source_url: null, fetched_at: null, attempted_at: null, next_refresh_at: 0, stale: true, status: 'unavailable', error_code: null }

async function open(width, state, height = 900) {
  const page = await browser.newPage({ viewport: { width, height }, colorScheme: 'light' })
  const errors = [], writes = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/api/**', route => {
    const request = route.request(), path = new URL(request.url()).pathname
    const json = (body, status = 200) => route.fulfill({ status, json: body })
    if (request.method() !== 'GET') { writes.push({ method: request.method(), path }); return json({ error: 'TEST_ONLY 此夹具不接受该写入。' }, 400) }
    if (path === '/api/dashboard/access') return json({ authenticated: true, public_dashboard: false })
    if (path === '/api/servers' || path === '/api/dashboard/servers') return json(state.servers)
    if (path === '/api/dashboard/exchange-rates') return json(rates)
    if (path === '/api/dashboard/probes/overview' || path.endsWith('/probes')) return json([])
    return json({ error: 'TEST_ONLY 此夹具未提供该数据。' }, 404)
  })
  await installControlCenterFixtures(page)
  // Saved preferences, written with revisions like the panel.
  await page.route('**/api/control-center/preferences/*', route => {
    const request = route.request(), key = decodeURIComponent(new URL(request.url()).pathname.split('/').pop())
    const record = state.preferences[key] ?? { value: /recent|favorite/.test(key) ? [] : null, revision: 0 }
    if (request.method() === 'GET') return route.fulfill({ json: { ...record, updated_at: 0 } })
    const body = request.postDataJSON()
    writes.push({ method: request.method(), path: `preference:${key}`, body })
    state.preferences[key] = { value: body.value, revision: record.revision + 1 }
    return route.fulfill({ json: { revision: record.revision + 1 } })
  })
  return { page, errors, writes }
}
const until = async (condition, message, limit = 5000) => {
  const deadline = Date.now() + limit
  while (!await condition()) { assert(Date.now() < deadline, message); await new Promise(done => setTimeout(done, 40)) }
}
const count = page => page.locator('[data-slot=server-count]').textContent()
const toolbar = page => page.locator('[data-slot=server-toolbar]')

const summary = []
try {
  // Desktop: the list starts on the first screen; extra filters become chips.
  {
    const state = { servers: fleet(), preferences: {} }
    const { page, errors, writes } = await open(1440, state)
    try {
      await page.goto(`${origin}/#/servers`)
      const firstRow = page.getByRole('row').filter({ hasText: 'TEST_ONLY JP 1' })
      await firstRow.waitFor()
      const top = (await firstRow.boundingBox()).y
      assert.equal(top < 600, true, `The first server row starts on the first screen (${top}px)`)
      assert.equal(await page.getByLabel('列表分组方式').count(), 0, 'View settings stay folded away until asked for')

      await toolbar(page).getByRole('button', { name: '更多筛选', exact: true }).click()
      const more = page.getByRole('dialog', { name: '更多筛选' })
      await more.getByLabel('分组').selectOption('亚洲')
      await until(async () => await count(page) === '3 / 6', 'The group filter narrows the list')
      await page.keyboard.press('Escape'); await more.waitFor({ state: 'hidden' })
      await until(async () => await page.evaluate(() => document.activeElement?.getAttribute('aria-label')) === '更多筛选', 'Focus returns to the button')
      const chip = page.locator('[data-slot=filter-chip]').filter({ hasText: '分组：亚洲' })
      await chip.waitFor()
      assert.equal(await toolbar(page).locator('[data-slot=toolbar-panel-count]').textContent(), '1')
      await toolbar(page).getByRole('button', { name: '更多筛选', exact: true }).click()
      await more.getByLabel('地区').selectOption('JP')
      await page.keyboard.press('Escape')
      await until(async () => await count(page) === '2 / 6', 'Group and region combine')
      await page.getByRole('button', { name: '移除筛选条件：分组：亚洲' }).click()
      await until(async () => await chip.count() === 0 && await count(page) === '2 / 6', 'Removing a chip clears only that condition')
      await page.getByRole('searchbox', { name: '筛选名称、标签或地区' }).fill('1')
      await until(async () => await count(page) === '1 / 6', 'Search combines with the remaining chip')
      await page.getByRole('button', { name: '清除全部筛选', exact: true }).click()
      await until(async () => await count(page) === '6', 'Clearing all filters shows every server')
      assert.equal(await page.getByRole('searchbox', { name: '筛选名称、标签或地区' }).inputValue(), '')

      // View settings: columns, row density and saved views.
      await toolbar(page).getByRole('button', { name: '视图', exact: true }).click()
      const view = page.getByRole('dialog', { name: '视图设置' })
      await view.getByRole('group', { name: '表格列' }).waitFor()
      await view.getByRole('checkbox', { name: '设备版本' }).uncheck()
      await until(async () => await page.getByRole('columnheader', { name: '设备版本' }).count() === 0, 'Hiding a column removes it from the table')
      await view.getByRole('button', { name: '紧凑', exact: true }).click()
      assert.equal(await view.getByRole('button', { name: '紧凑', exact: true }).getAttribute('aria-pressed'), 'true')
      await view.getByRole('button', { name: '保存当前视图', exact: true }).click()
      await until(() => writes.some(write => write.path === 'preference:view:servers'), 'The view is saved')
      const saved = writes.find(write => write.path === 'preference:view:servers').body.value
      assert.deepEqual({ columns: saved.columns, cardColumns: saved.cardColumns, density: saved.density, region: saved.region }, { columns: ['cpu', 'memory', 'cost', 'traffic'], cardColumns: ['cpu', 'memory'], density: 'compact', region: '' })
      await page.keyboard.press('Escape')
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'toolbar-desktop', firstRowTop: Math.round(top) })
    } finally { await page.close() }
  }

  // Phones: options open in a bottom drawer; cards show fewer fields by default.
  {
    const state = { servers: fleet(), preferences: {} }
    const { page, errors } = await open(390, state, 844)
    try {
      await page.goto(`${origin}/#/servers`)
      const card = page.locator('[data-slot=server-list-card]').first()
      await card.waitFor()
      assert.equal(await card.getByRole('progressbar', { name: /处理器使用率/ }).count(), 1, 'Cards keep the load meters')
      assert.equal(await card.getByText('本期观测 / 额度', { exact: true }).count(), 0, 'Traffic is optional on phones')
      assert.equal(await card.getByText('成本 / 到期', { exact: true }).count(), 0, 'Cost is optional on phones')
      await toolbar(page).getByRole('button', { name: '更多筛选', exact: true }).click()
      const drawer = page.getByRole('dialog', { name: '更多筛选' })
      await drawer.waitFor()
      assert.equal(await drawer.getAttribute('data-placement'), 'bottom')
      await page.waitForTimeout(300)
      const box = await drawer.boundingBox()
      assert.equal(Math.round(box.y + box.height) >= 843 && box.width >= 389, true, 'The drawer spans the bottom edge')
      await drawer.getByLabel('地区').selectOption('US')
      await drawer.getByRole('button', { name: '完成', exact: true }).click()
      await drawer.waitFor({ state: 'hidden' })
      await page.locator('[data-slot=filter-chip]').filter({ hasText: '地区：US' }).waitFor()
      assert.equal(await count(page), '2 / 6')
      await toolbar(page).getByRole('button', { name: '视图', exact: true }).click()
      const view = page.getByRole('dialog', { name: '视图设置' })
      await view.getByRole('group', { name: '卡片显示' }).getByRole('checkbox', { name: '流量' }).check()
      await view.getByRole('button', { name: '完成', exact: true }).click()
      await view.waitFor({ state: 'hidden' })
      await card.getByText('本期观测 / 额度', { exact: true }).waitFor()
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, 'No horizontal scroll')
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'toolbar-phone' })
    } finally { await page.close() }
  }

  // Compact density tightens content spacing and keeps every control's size.
  {
    const measure = async density => {
      const state = { servers: fleet(), preferences: { appearance: { value: { theme: 'light', density }, revision: 1 } } }
      const { page, errors } = await open(1440, state)
      try {
        await page.goto(`${origin}/#/servers`)
        await page.getByRole('row').filter({ hasText: 'TEST_ONLY JP 1' }).waitFor()
        await until(async () => await page.evaluate(() => document.documentElement.dataset.density) === density, `The ${density} density applies`)
        const sizes = await page.evaluate(() => {
          const box = selector => document.querySelector(selector).getBoundingClientRect()
          return {
            button: box('header[data-slot=page-header] [data-slot=button]').height,
            input: box('[data-slot=server-toolbar] input').height,
            checkbox: box('[data-slot=checkbox]').height,
            link: box('[data-slot=sidebar] nav a').height,
            stat: parseFloat(getComputedStyle(document.querySelector('[data-slot=stat]')).paddingTop),
            cell: parseFloat(getComputedStyle(document.querySelector('tbody td')).paddingTop),
          }
        })
        assert.deepEqual(errors, [])
        return sizes
      } finally { await page.close() }
    }
    const comfortable = await measure('comfortable'), compact = await measure('compact')
    // Compared at 0.01px: layout rounding can differ below a pixel.
    for (const key of ['button', 'input', 'checkbox', 'link']) assert.equal(Math.abs(compact[key] - comfortable[key]) < 0.01, true, `Compact keeps the ${key} size (${compact[key]} vs ${comfortable[key]})`)
    assert.equal(compact.stat < comfortable.stat, true, 'Compact tightens panel padding')
    assert.equal(compact.cell < comfortable.cell, true, 'Compact tightens table rows')
    summary.push({ scenario: 'density', comfortable, compact })
  }

  // The proxy network leads the sidebar; addresses and the default page stay.
  {
    const state = { servers: fleet(), preferences: {} }
    const { page, errors } = await open(1440, state)
    try {
      await page.goto(`${origin}/#/servers`)
      const nav = page.getByRole('navigation', { name: '主导航' })
      await nav.getByRole('link', { name: '概览', exact: true }).waitFor()
      const links = await nav.getByRole('link').evaluateAll(items => items.map(item => [item.textContent.trim(), item.getAttribute('href')]))
      // ADR 0098: the overview group leads with the landing page, then the proxy service.
      assert.deepEqual(links.slice(0, 8), [['概览', '#/plugins/sing-box'], ['服务器看板', '#/dashboard'], ['统计报表', '#/statistics'], ['费用与续费', '#/plugins/expenses'], ['代理节点', '#/plugins/sing-box/nodes'], ['代理用户', '#/plugins/sing-box/users'], ['策略与套餐', '#/plugins/sing-box/groups'], ['成员运营', '#/plugins/sing-box/commerce']])
      assert.equal(await nav.locator(':scope > div').first().locator(':scope > div').first().textContent(), '总览', 'The overview group comes first')
      assert.deepEqual(links.find(([label]) => label === '服务器列表'), ['服务器列表', '#/servers'])
      // Decision D6: signing in without an address lands on the overview, not the server board.
      await page.goto(`${origin}/`)
      await page.waitForFunction(() => location.hash === '#/plugins/sing-box')
      await page.getByRole('heading', { name: '概览', exact: true }).waitFor()
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'navigation' })
    } finally { await page.close() }
  }

  // Closing a dialog plays its exit animation without blocking the page; with
  // reduced motion it closes at once.
  for (const reducedMotion of ['no-preference', 'reduce']) {
    const state = { servers: fleet(), preferences: {} }
    const { page, errors, writes } = await open(1440, state)
    try {
      await page.emulateMedia({ reducedMotion })
      await page.goto(`${origin}/#/servers`)
      await page.getByRole('row').filter({ hasText: 'TEST_ONLY JP 1' }).waitFor()
      const add = page.locator('header[data-slot=page-header]').getByRole('button', { name: '添加服务器', exact: true })
      const box = await add.boundingBox()
      await add.click()
      const dialog = page.getByRole('dialog').filter({ has: page.getByLabel('服务器名称') })
      await dialog.waitFor(); await page.waitForTimeout(300)
      await page.evaluate(() => { window.__exitFrames = 0; const count = () => { if (document.querySelector('[data-slot=dialog-content]')) { window.__exitFrames++; requestAnimationFrame(count) } }; window.__countExit = count })
      await dialog.getByRole('button', { name: '取消', exact: true }).click()
      await page.evaluate(() => requestAnimationFrame(window.__countExit))
      if (reducedMotion === 'no-preference') {
        // While it animates out, the dialog is inert and its backdrop lets clicks
        // through to the page.
        await page.waitForFunction(() => document.querySelector('[data-slot=dialog-content][inert]'), null, { polling: 'raf', timeout: 2000 })
        const target = await page.evaluate(({ x, y }) => document.elementFromPoint(x, y)?.closest('button')?.textContent?.trim(), { x: box.x + box.width / 2, y: box.y + box.height / 2 })
        assert.equal(target, '添加服务器', 'The page under a closing dialog receives clicks')
      }
      await dialog.waitFor({ state: 'hidden' })
      assert.equal(await page.locator('[data-slot=dialog-content], [data-slot=dialog-overlay]').count(), 0, 'Hidden means gone from the document')
      const frames = await page.evaluate(() => window.__exitFrames)
      if (reducedMotion === 'no-preference') assert.equal(frames >= 3, true, `The exit animation runs over several frames (${frames})`)
      else assert.equal(frames <= 1, true, `Reduced motion closes at once (${frames} frames)`)
      await until(async () => await page.evaluate(() => document.activeElement?.textContent?.trim()) === '添加服务器', 'Focus returns after the exit')
      // Confirmations close the same way.
      await page.getByRole('row').filter({ hasText: 'TEST_ONLY JP 1' }).getByRole('button', { name: '删除', exact: true }).click()
      const confirm = page.getByRole('alertdialog')
      await confirm.waitFor(); await page.waitForTimeout(300)
      await confirm.getByRole('button', { name: '取消', exact: true }).click()
      await confirm.waitFor({ state: 'hidden' })
      assert.equal(await page.locator('[data-slot=alert-dialog-content]').count(), 0)
      assert.deepEqual(writes, [], 'Cancelling sends nothing')
      assert.deepEqual(errors, [])
      summary.push({ scenario: `dialog-exit-${reducedMotion}`, frames })
    } finally { await page.close() }
  }

  // Arrows are Lucide icons, not text characters.
  {
    const state = { servers: fleet(), preferences: {} }
    const { page, errors } = await open(1440, state)
    try {
      await page.goto(`${origin}/#/dashboard`)
      await page.getByRole('button', { name: '筛选与视图', exact: true }).click()
      await page.getByRole('button', { name: '表格', exact: true }).click()
      const table = page.locator('[data-slot=fleet]')
      await table.locator('tbody tr').first().waitFor()
      const text = await table.innerText()
      assert.equal(/[↑↓→]/.test(text), false, 'The dashboard table has no text arrows')
      assert.equal(await table.locator('tbody tr').first().getByLabel('上传').count(), 1)
      assert.deepEqual(errors, [])
      summary.push({ scenario: 'icons' })
    } finally { await page.close() }
  }
  console.log(JSON.stringify({ passed: true, scenarios: summary }, null, 2))
} finally {
  await browser.close()
  await new Promise(done => http.close(done))
}
