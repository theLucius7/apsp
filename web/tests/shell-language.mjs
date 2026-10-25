import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/shell-language.mjs`.
// Covers the 2026-10-24 shell and language fixes: phone pages scroll, crossing the
// tablet breakpoint keeps an open dialog, the phone menu closes on Escape and on an
// outside press, English text follows live updates, and the header language switch
// is not reverted by the saved appearance.
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

async function open(width, state) {
  const page = await browser.newPage({ viewport: { width, height: 700 } })
  const errors = []
  page.on('pageerror', error => errors.push(error.message))
  await page.route('**/api/**', async route => {
    const request = route.request(), path = new URL(request.url()).pathname
    const json = (body, status = 200) => route.fulfill({ status, json: body })
    if (request.method() === 'PUT' && path === '/api/control-center/preferences/appearance') return json({ value: request.postDataJSON().value, revision: 2, updated_at: 1 })
    if (request.method() !== 'GET') { errors.push(`Unexpected ${request.method()} ${path}`); return json({ error: 'TEST_ONLY unexpected write' }, 400) }
    if (path === '/api/dashboard/access') return json({ authenticated: true, public_dashboard: false })
    if (path === '/api/control-center/me') return json({ id: 1, role: 'owner', display_name: 'TEST_ONLY 所有者', all_servers: true, capabilities: [], token_capabilities: null, token_servers: null })
    if (path === '/api/control-center/preferences/appearance') return json({ value: { theme: 'light', density: 'comfortable', locale: 'zh-CN' }, revision: 1, updated_at: 0 })
    if (path.startsWith('/api/control-center/preferences/')) return json({ value: /recent|favorite/.test(path) ? [] : null, revision: 0, updated_at: 0 })
    if (path === '/api/servers') return json(state.servers())
    errors.push(`Unexpected GET ${path}`)
    return json({ error: 'TEST_ONLY 此夹具未提供该数据。' }, 404)
  })
  await page.goto(`${origin}/#/servers`)
  return { page, errors }
}

function servers(count, online) {
  const now = Math.floor(Date.now() / 1000)
  return Array.from({ length: count }, (_, index) => ({
    id: index + 1, name: `TEST_ONLY 服务器 ${index + 1}`, online: index === 0 ? online() : true, device_public_key: 'TEST_ONLY',
    metrics_stale: false, registered: true, manifest_rev: 0, last_seen: now, last_heartbeat_at: now, metrics_sampled_at: Date.now(),
    static_info: { memory_total: 8 * GiB, hostname: `fixture-${index + 1}` }, latest_metrics: { cpu_percent: 20, memory_used: 2 * GiB },
  }))
}

const results = {}
try {
  // Phones: the page scrolls and the menu closes on Escape or an outside press.
  {
    const { page, errors } = await open(390, { servers: () => servers(30, () => true) })
    await page.locator('[data-slot=server-list-card]').filter({ has: page.getByText('TEST_ONLY 服务器 30', { exact: true }) }).waitFor()
    const column = page.locator('[data-slot=topbar]').locator('xpath=..')
    const scrolled = await column.evaluate(element => { element.scrollTop = element.scrollHeight; return element.scrollTop })
    assert.ok(scrolled > 0, 'The phone page column scrolls')
    const menu = page.getByRole('button', { name: '菜单' })
    await menu.click()
    await page.locator('#admin-navigation').getByRole('navigation').waitFor()
    await page.keyboard.press('Escape')
    assert.equal(await menu.getAttribute('aria-expanded'), 'false', 'Escape closes the menu')
    await menu.click()
    assert.equal(await menu.getAttribute('aria-expanded'), 'true')
    // A press near the bottom of the viewport lands on the page, outside the menu.
    await page.mouse.click(195, 690)
    assert.equal(await menu.getAttribute('aria-expanded'), 'false', 'An outside press closes the menu')
    assert.equal(new URL(page.url()).hash, '#/servers', 'The press closed the menu without following a link')
    assert.deepEqual(errors, [])
    results.phone = 'passed'
    await page.close()
  }
  // Crossing the tablet breakpoint keeps the page and its open dialog.
  {
    const { page, errors } = await open(1024, { servers: () => servers(2, () => true) })
    // The delete confirmation is page state that needs no further reads.
    await page.getByRole('row').filter({ has: page.getByText('TEST_ONLY 服务器 1', { exact: true }) }).getByRole('button', { name: '删除', exact: true }).click()
    const dialog = page.getByRole('alertdialog')
    await dialog.waitFor()
    await page.setViewportSize({ width: 600, height: 700 })
    await page.waitForTimeout(300)
    assert.equal(await dialog.isVisible(), true, 'The dialog stays open on the phone layout')
    await page.setViewportSize({ width: 1024, height: 700 })
    await page.waitForTimeout(300)
    assert.equal(await dialog.isVisible(), true, 'The dialog stays open back on the desktop layout')
    assert.deepEqual(errors, [])
    results.breakpoint = 'passed'
    await page.close()
  }
  // English text follows live updates and the header switch is not reverted.
  {
    let online = true
    const { page, errors } = await open(1440, { servers: () => servers(1, () => online) })
    const first = page.getByRole('row').filter({ has: page.getByText('TEST_ONLY 服务器 1', { exact: true }) })
    await first.getByText('在线', { exact: true }).waitFor()
    await page.getByRole('combobox', { name: '界面语言' }).selectOption('en-US')
    await first.getByText('Online', { exact: true }).waitFor()
    online = false
    await first.getByText('Offline', { exact: true }).waitFor({ timeout: 15000 })
    await page.waitForTimeout(1000)
    assert.equal(await page.evaluate(() => document.documentElement.lang), 'en-US', 'The saved Chinese preference does not revert the header switch')
    await page.getByRole('combobox', { name: 'Interface language' }).selectOption('zh-CN')
    await first.getByText('离线', { exact: true }).waitFor()
    assert.deepEqual(errors, [])
    results.language = 'passed'
    await page.close()
  }
  console.log(JSON.stringify({ passed: true, ...results }, null, 2))
} finally {
  await browser.close()
  await new Promise(done => http.close(done))
}
