import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/command-palette.mjs`.
// Drives the administrator command palette against loopback fixtures: page
// jumps, keyboard selection, recent visits, server search and favourites.
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
const summary = []

try {
  for (const width of [1440, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 900 } })
    const errors = [], writes = [], searches = [], answered = []
    const preferences = { recent: { value: [], revision: 0 }, favorite: { value: [], revision: 0 } }
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/api/**', async route => {
      const request = route.request(), url = new URL(request.url()), path = url.pathname
      const json = (body, status = 200) => route.fulfill({ status, json: body })
      const preference = path.match(/^\/api\/control-center\/preferences\/(recent|favorite)$/)
      if (preference && request.method() === 'PUT') {
        const body = request.postDataJSON(), record = preferences[preference[1]]
        writes.push({ key: preference[1], value: body.value })
        assert.equal(body.expected_revision, record.revision, 'Preference writes carry the read revision')
        Object.assign(record, { value: body.value, revision: record.revision + 1 })
        return json({ revision: record.revision })
      }
      if (request.method() !== 'GET') { errors.push(`Unexpected ${request.method()} ${path}`); return json({ error: 'TEST_ONLY unexpected write' }, 400) }
      if (path === '/api/dashboard/access') return json({ authenticated: true, public_dashboard: false })
      if (path === '/api/control-center/me') return json({ id: 1, role: 'owner', display_name: 'TEST_ONLY 所有者', all_servers: true, capabilities: [], token_capabilities: null, token_servers: null })
      if (preference) return json({ ...preferences[preference[1]], updated_at: 0 })
      if (path.startsWith('/api/control-center/preferences/')) return json({ value: null, revision: 0, updated_at: 0 })
      if (path === '/api/control-center/search') {
        const q = url.searchParams.get('q') ?? ''
        searches.push(q)
        // Answer slowly so results land while the keyboard selection is moving.
        await new Promise(done => setTimeout(done, 300))
        await json({ results: q.includes('东京') ? [{ kind: 'server', id: 7, label: 'TEST_ONLY 东京 · 主节点', path: '/servers/7' }] : [] }).catch(() => {})
        answered.push(q)
        return
      }
      if (path === '/api/servers') return json([])
      // Pages opened from the palette may read more data; an empty error keeps them inert.
      return json({ error: 'TEST_ONLY 此夹具未提供该数据。' }, 404)
    })
    try {
      await page.goto(`${origin}/#/servers`)
      await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
      const trigger = page.getByRole('button', { name: '全局搜索', exact: true })
      const dialog = page.getByRole('dialog', { name: '搜索或跳转' })
      const input = dialog.getByRole('combobox', { name: '搜索页面或对象' })
      const options = dialog.getByRole('option')
      const selected = () => dialog.locator('[role=option][aria-selected=true]')

      // The trigger opens the palette with the query field focused and every permitted page listed.
      await trigger.click()
      await dialog.waitFor()
      assert.equal(await input.evaluate(element => element === document.activeElement), true, 'The query field receives focus')
      assert.equal(await dialog.getByRole('group', { name: '页面' }).getByRole('option').count() >= 15, true, 'Every sidebar page can be jumped to')
      assert.equal(await selected().count(), 1, 'Exactly one option is active')
      assert.equal(await input.getAttribute('aria-activedescendant'), await selected().getAttribute('id'), 'The active option is announced')

      // Typing filters pages; arrow keys move and wrap without leaving the field.
      await input.fill('告警')
      await dialog.getByRole('option', { name: /告警设置/ }).waitFor()
      assert.equal(await dialog.getByRole('option', { name: /代理节点/ }).count(), 0, 'Unrelated pages are filtered out')
      const total = await options.count()
      assert.equal(total >= 2, true)
      const first = await selected().getAttribute('id')
      await input.press('ArrowDown')
      assert.notEqual(await selected().getAttribute('id'), first, 'ArrowDown moves the active option')
      await input.press('ArrowUp'); await input.press('ArrowUp')
      assert.equal(await selected().getAttribute('id'), await options.nth(total - 1).getAttribute('id'), 'ArrowUp wraps to the last option')
      assert.equal(await input.getAttribute('aria-activedescendant'), await selected().getAttribute('id'))
      const chosen = await selected().textContent()
      const deadline = Date.now() + 5000
      while (!answered.includes('告警') && Date.now() < deadline) await page.waitForTimeout(20)
      await page.waitForTimeout(150)
      assert.equal(searches.includes('告警'), true, 'Typing also queries the server-side search')
      assert.equal(await selected().textContent(), chosen, 'A late search response keeps the keyboard selection')

      // Enter opens the active page, closes the palette and records the visit.
      await input.fill('告警设置')
      await dialog.getByRole('option', { name: /告警设置/ }).waitFor()
      await input.press('Enter')
      await dialog.waitFor({ state: 'hidden' })
      assert.equal(new URL(page.url()).hash, '#/monitoring/settings')
      assert.deepEqual(writes.at(-1), { key: 'recent', value: [{ kind: 'page', id: 'alert-settings', label: '告警设置', path: '/monitoring/settings' }] })

      // Ctrl+K reopens it from anywhere; the visit now appears under recent pages.
      await page.locator('body').press('Control+k')
      await dialog.waitFor()
      await dialog.getByRole('group', { name: '最近访问' }).getByRole('option', { name: /告警设置/ }).waitFor()

      // Server results come from the search API; Shift+Enter favourites the active result.
      await input.fill('东京')
      const result = dialog.getByRole('group', { name: '搜索结果' }).getByRole('option', { name: /TEST_ONLY 东京 · 主节点/ })
      await result.waitFor()
      assert.equal(await result.getAttribute('aria-selected'), 'true', 'The only match becomes active')
      await input.press('Shift+Enter')
      await dialog.getByRole('button', { name: /取消收藏/ }).waitFor()
      assert.deepEqual(writes.at(-1), { key: 'favorite', value: [{ kind: 'server', id: 7, label: 'TEST_ONLY 东京 · 主节点', path: '/servers/7' }] })
      assert.equal(await dialog.isVisible(), true, 'Favouriting keeps the palette open')
      await input.press('Enter')
      await dialog.waitFor({ state: 'hidden' })
      assert.equal(new URL(page.url()).hash, '#/servers/7')

      // "/" opens the palette, but stays text inside form fields; Escape closes it.
      await page.goto(`${origin}/#/servers`)
      await page.getByRole('heading', { name: '服务器列表', exact: true }).waitFor()
      const filter = page.getByLabel('筛选名称、标签或地区')
      await filter.fill('a')
      await filter.press('/')
      assert.equal(await filter.inputValue(), 'a/', '"/" is typed into a focused field')
      assert.equal(await dialog.count(), 0)
      // Outside any field, "/" opens the palette.
      await filter.evaluate(element => element.blur())
      await page.keyboard.press('/')
      await dialog.waitFor()
      await dialog.getByRole('group', { name: '收藏' }).getByRole('option', { name: /TEST_ONLY 东京 · 主节点/ }).waitFor()
      assert.equal(await input.inputValue(), '', 'A reopened palette starts with an empty query')
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `No horizontal scroll at ${width}px`)
      const box = await dialog.boundingBox()
      assert.equal(box.x >= 0 && box.x + box.width <= width, true, 'The palette fits the viewport')
      await page.keyboard.press('Escape')
      await dialog.waitFor({ state: 'hidden' })
      assert.equal(await trigger.isVisible(), true)
      assert.deepEqual(errors, [])
      summary.push({ width, options: total, searches: searches.length, writes: writes.length })
    } finally { await page.close() }
  }
  console.log(JSON.stringify({ passed: true, scenarios: summary }, null, 2))
} finally {
  await browser.close()
  await new Promise(done => http.close(done))
}
