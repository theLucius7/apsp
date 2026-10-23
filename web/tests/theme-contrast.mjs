import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { extname, resolve, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

// TEST_ONLY: after rebuilding web/dist, run `node web/tests/theme-contrast.mjs`.
// Audits the built sign-in page, administrator server list, public dashboard and
// a dialog in both themes. Every visible text node is measured against the
// background actually painted behind it (all translucent layers composited),
// so a token change that weakens contrast anywhere fails here. Only local
// fixtures are served; no deployed panel is contacted.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE
  ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const root = fileURLToPath(new URL('../dist/', import.meta.url))
const types = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.webp': 'image/webp', '.txt': 'text/plain' }
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(root, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(root.endsWith(sep) ? root : `${root}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': types[extname(file)] || 'application/octet-stream' }).end(body) }
  catch { response.writeHead(404).end() }
})

const GiB = 1024 ** 3
const now = Date.now()
const servers = [['东京 · 主节点', true, 'JP', 42], ['法兰克福', false, 'DE', 91], ['新加坡 · 出口', true, 'SG', 78]].map(([name, online, region, cpu], index) => ({
  id: index + 1, name, online, metrics_stale: false, device_public_key: 'TEST_ONLY', registered: true,
  metrics_sampled_at: now, last_seen: now / 1000, last_heartbeat_at: now / 1000, manifest_rev: index,
  static_info: { system: 'Ubuntu 24.04', arch: 'amd64', cpu_cores: 4, memory_total: 8 * GiB, disk_total: 64 * GiB, hostname: `fixture-${index + 1}` },
  latest_metrics: { cpu_percent: cpu, memory_used: 6 * GiB, disk_used: 20 * GiB, load_1: .4, load_5: .3, load_15: .2, uptime_secs: 86400, network_interfaces: { eth0: { transmit_bytes_per_sec: 1048576, receive_bytes_per_sec: 2097152, transmitted_bytes: 10 * GiB, received_bytes: 30 * GiB } } },
  asset_settings: { region, group_name: '亚洲', tags: ['BGP'], hidden: false, price: '12.5', currency: 'CNY', billing_cycle: 30, expires_at: Math.floor(now / 1000) + 5 * 86400, auto_renewal: false, traffic_limit: '0', traffic_limit_type: 'sum', reset_day: 1, network_interface: '' },
}))
const owner = { id: 1, role: 'owner', display_name: 'TEST_ONLY 所有者', all_servers: true, capabilities: [], token_capabilities: null, token_servers: null }
const rates = { base: 'CNY', rates: { CNY: 1 }, rate_dates: {}, rate_date: null, source: null, source_url: null, fetched_at: null, attempted_at: null, next_refresh_at: 0, stale: true, status: 'unavailable', error_code: null }

function fixtures(scenario) {
  return async route => {
    const request = route.request(), path = new URL(request.url()).pathname
    const json = (body, status = 200) => route.fulfill({ status, json: body })
    if (path === '/api/dashboard/access') return json({ authenticated: scenario === 'admin', public_dashboard: scenario === 'dashboard' })
    if (path === '/api/control-center/me') return json(owner)
    if (path.startsWith('/api/control-center/preferences/')) return json({ value: /recent|favorite/.test(path) ? [] : null, revision: 0, updated_at: 0 })
    if (path === '/api/servers' || path === '/api/dashboard/servers') return json(servers)
    if (path === '/api/dashboard/exchange-rates') return json(rates)
    if (path === '/api/dashboard/probes/overview') return json([])
    if (path === '/api/login') return json({ error: '测试：密码或验证码不正确。' }, 400)
    return json({ error: '测试：此夹具未提供该数据。' }, 404)
  }
}

// Runs in the page: composite every painted ancestor layer and measure text.
function audit() {
  const canvas = document.createElement('canvas'); canvas.width = canvas.height = 1
  const context = canvas.getContext('2d', { willReadFrequently: true })
  const rgba = colour => { context.clearRect(0, 0, 1, 1); context.fillStyle = '#000'; context.fillStyle = colour; context.fillRect(0, 0, 1, 1); const [r, g, b, a] = context.getImageData(0, 0, 1, 1).data; return [r, g, b, a / 255] }
  const over = (front, back) => { const alpha = front[3] + back[3] * (1 - front[3]); return alpha === 0 ? [0, 0, 0, 0] : [...front.slice(0, 3).map((value, index) => (value * front[3] + back[index] * back[3] * (1 - front[3])) / alpha), alpha] }
  const luminance = colour => colour.slice(0, 3).map(value => { const channel = value / 255; return channel <= .04045 ? channel / 12.92 : ((channel + .055) / 1.055) ** 2.4 }).reduce((sum, value, index) => sum + value * [.2126, .7152, .0722][index], 0)
  const failures = []
  let checked = 0
  for (const element of document.querySelectorAll('body *')) {
    const text = [...element.childNodes].filter(node => node.nodeType === Node.TEXT_NODE).map(node => node.textContent.trim()).join('')
    if (!text || !element.checkVisibility({ opacityProperty: true, visibilityProperty: true })) continue
    if (element.closest(':disabled, [aria-disabled="true"], [data-disabled], .sr-only, option, script, style')) continue
    const box = element.getBoundingClientRect()
    if (box.width < 1 || box.height < 1) continue
    const chain = []
    for (let node = element; node; node = node.parentElement) chain.unshift(node)
    let background = rgba(getComputedStyle(document.documentElement).backgroundColor)
    if (background[3] < 1) background = over(background, [255, 255, 255, 1])
    let opacity = 1
    for (const node of chain) {
      const style = getComputedStyle(node)
      opacity *= Number(style.opacity)
      background = over(rgba(style.backgroundColor), background)
    }
    const style = getComputedStyle(element)
    const foreground = rgba(style.color)
    foreground[3] *= opacity
    const rendered = over(foreground, background)
    const first = luminance(rendered), second = luminance(background)
    const ratio = (Math.max(first, second) + .05) / (Math.min(first, second) + .05)
    const size = parseFloat(style.fontSize), weight = Number(style.fontWeight)
    const large = size >= 24 || (size >= 18.66 && weight >= 700)
    checked++
    if (ratio < (large ? 3 : 4.5)) failures.push({ text: text.slice(0, 40), ratio: Number(ratio.toFixed(2)), colour: style.color, size, slot: element.closest('[data-slot]')?.getAttribute('data-slot') ?? element.tagName })
  }
  return { checked, failures }
}

const totals = { runs: [], failures: [], pageErrors: [] }
async function run() {
  await new Promise(done => server.listen(0, '127.0.0.1', done))
  const origin = `http://127.0.0.1:${server.address().port}`
  const browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  try {
    for (const width of [1440, 390]) for (const theme of ['light', 'dark']) {
      const context = await browser.newContext({ viewport: { width, height: 1000 }, colorScheme: theme, reducedMotion: 'reduce' })
      try {
        for (const scenario of ['login', 'admin', 'dialog', 'dashboard']) {
          const page = await context.newPage()
          page.on('pageerror', error => totals.pageErrors.push({ width, theme, scenario, error: error.message }))
          await page.route('**/api/**', fixtures(scenario === 'dialog' ? 'admin' : scenario))
          await page.goto(`${origin}/#${scenario === 'dashboard' ? '/dashboard' : '/servers'}`)
          if (scenario === 'login') {
            await page.getByRole('heading', { name: '欢迎回来' }).waitFor()
            await page.getByLabel('管理员密码').fill('TEST_ONLY')
            await page.getByRole('button', { name: /登录面板/ }).click()
            await page.getByRole('alert').waitFor()
          } else if (scenario === 'dashboard') {
            await page.locator('[data-slot=server-card]').first().waitFor()
          } else {
            await page.getByRole('heading', { name: '全部服务器' }).waitFor()
            if (scenario === 'dialog') {
              await page.getByRole('row').filter({ hasText: '法兰克福' }).getByRole('button', { name: '删除', exact: true }).click()
              await page.getByRole('alertdialog').waitFor()
            }
          }
          await page.evaluate(() => Promise.all(document.getAnimations().map(animation => animation.finished.catch(() => {}))))
          assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `${scenario}/${theme}/${width} must not scroll horizontally`)
          const result = await page.evaluate(audit)
          assert(result.checked > 10, `${scenario}/${theme}/${width} should expose text to audit`)
          totals.runs.push({ width, theme, scenario, checked: result.checked })
          for (const failure of result.failures) totals.failures.push({ width, theme, scenario, ...failure })
          await page.close()
        }
      } finally { await context.close() }
    }
    assert.deepEqual(totals.pageErrors, [])
    assert.deepEqual(totals.failures, [], 'Every visible text must meet WCAG AA contrast against its painted background')
  } finally {
    await browser.close()
    await new Promise(done => server.close(done))
  }
}

try { await run() }
catch (error) { process.exitCode = 1; totals.failure = error.stack ?? String(error) }
finally { console.log(JSON.stringify({ runs: totals.runs.length, checked: totals.runs.reduce((sum, item) => sum + item.checked, 0), failures: totals.failures.slice(0, 40), pageErrors: totals.pageErrors, failure: totals.failure }, null, 2)) }
