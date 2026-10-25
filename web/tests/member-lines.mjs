import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { resolve, extname, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) return response.writeHead(400).end()
  try { response.writeHead(200, { 'Content-Type': ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css' })[extname(file)] ?? 'application/octet-stream' }).end(await readFile(file)) } catch { response.writeHead(404).end() }
})

// Member line catalogue and probe panel (ADR 0101). The fixture carries the
// fields the panel returns; addresses never reach the member portal.
const now = Math.floor(Date.now() / 1000)
const entry = (region, provider, node) => ({ id: 'entry', role: 'entry', region, provider, node_id: node })
const legacyLatency = { source: 'legacy_monitor', monitored: true, updated_at: now - 120, window_minutes: 60, direction: 'node_to_carrier', carriers: { telecom: { latest: 45, average: 50, loss: 0 }, unicom: { latest: 160, average: 150, loss: 1.5 }, mobile: { latest: 38, average: 40, loss: 0 } } }
const lines = [
  { id: 11, name: '香港 01', protocol: { name: 'vless', network: 'tcp', security: 'reality' }, topology: { region: 'HK', provider: 'Example', residential: false, dynamic: false, classification_source: 'node_catalog', kind: 'direct', source: 'node_catalog', inferred: false, position_precision: 'region', front_node_id: null, points: [entry('HK', 'Example', 11)], edges: [] }, status: { state: 'online', checked_at: now - 30, source: 'apsp' }, rate: '1.00', timed_rate: false, latency: legacyLatency, host: 'h1', network_scope: 'entry', plans: [{ name: '标准', current: true }], access: true },
  { id: 12, name: '日本家宽中转', protocol: { name: 'hysteria2', network: 'udp', security: 'tls' }, topology: { region: 'JP', provider: null, residential: true, dynamic: true, classification_source: 'catalog_tag', kind: 'relay', source: 'shared_ingress', inferred: true, position_precision: 'region', front_node_id: 11, points: [entry('HK', 'Example', 11), { id: 'egress', role: 'egress', region: 'JP', provider: null, node_id: 12 }], edges: [{ from: 'entry', to: 'egress', traffic: 'all' }] }, status: { state: 'entry_online', checked_at: now - 40, source: 'legacy' }, rate: '2', timed_rate: true, latency: { ...legacyLatency, referenced: true }, host: 'h1', network_scope: 'egress_unverified', plans: [{ name: '高级', current: false }], access: false },
  { id: 13, name: '美国 AI 分流', protocol: { name: 'trojan', network: 'tcp', security: 'tls' }, topology: { region: 'US', provider: null, residential: false, dynamic: false, classification_source: 'node_catalog', kind: 'policy', source: 'unresolved', inferred: true, position_precision: 'region', front_node_id: null, points: [entry(null, null, null), { id: 'egress-tw', role: 'egress', region: 'TW', provider: null, node_id: null }, { id: 'egress-us', role: 'egress', region: 'US', provider: null, node_id: null }], edges: [{ from: 'entry', to: 'egress-tw', traffic: 'ai' }, { from: 'entry', to: 'egress-us', traffic: 'apple_news' }] }, status: { state: 'unknown', checked_at: null, source: 'unknown' }, rate: '0.5', timed_rate: false, latency: { monitored: false }, host: 'h2', network_scope: 'egress_unverified', plans: [{ name: '标准', current: true }, { name: '高级', current: false }], access: true },
]
const hosts = [
  { key: 'h1', region: 'HK', provider: 'Example', residential_declared: false, online: true, checked_at: now - 30, source: 'apsp', load: { cpu: 12.5, memory: 40, disk: 92 }, uptime_secs: 90000, latency: { ...legacyLatency, source: 'apsp_probe' }, lines: [11, 12],
    network: { available: true, checked_at: now - 3600, families: [{ family: 'IPv4', summary: { asn: 64500, organization: 'Example Network', country: 'HK' }, datasets: [
      { source: 'DB-IP', database: 'dbip-v2', checked_at: now - 3600, historical: false, fields: [{ label: 'ASN', value: 64500, kind: 'asn' }, { label: '网络组织', value: 'Example Network', kind: 'text' }, { label: '用途类型', value: 'isp', kind: 'text' }, { label: '代理', value: false, kind: 'boolean' }] },
      { source: 'ipapi', database: 'ipapi', checked_at: now - 3600, historical: false, fields: [{ label: '数据中心', value: true, kind: 'boolean' }, { label: '滥用评分（上游原值）', value: 0.001, kind: 'score' }] },
    ] }] } },
  { key: 'h2', region: null, provider: null, residential_declared: false, online: false, checked_at: null, source: 'unknown', load: {}, uptime_secs: null, latency: { monitored: false }, lines: [13], network: { available: false, reason: 'no_agent' } },
]

let browser
try {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  const origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  for (const width of [1440, 390, 320]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } }), errors = [], unexpected = []
    page.on('pageerror', error => errors.push(error.message))
    const account = '00000000-0000-4000-8000-000000000041'
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname
      let value
      if (path === `/api/plugins/sing-box/portal/${account}`) value = { authenticated: true, name: '测试成员', configuration: { enabled: false, reason: '夹具未启用认证器' }, subscription_url: 'https://legacy.example.com/assets/fixture-token', legacy_compatibility: { last_sync_at: 1 }, usage: { uplink: '20', downlink: '30' }, keys: [], subscription_status: { status: 'ready', message: '', entitlement: { package_name: '标准', used_bytes: '50', monthly_bytes: '1000', expires_at: null, next_reset: null } } }
      else if (path === `/api/plugins/sing-box/portal/${account}/lines`) value = { generated_at: now, package: '标准', lines, hosts }
      else if (path === `/api/plugins/sing-box/portal/${account}/commerce`) value = { enabled: false }
      else if (path === '/api/dashboard/access') value = { authenticated: false, public_dashboard: false }
      else { unexpected.push(path); return route.fulfill({ status: 404, json: { error: 'TEST_ONLY 未知请求' } }) }
      return route.fulfill({ json: value })
    })
    await page.goto(`${origin}/#/plugins/sing-box/account/${account}`)
    const panel = page.locator('[data-slot=member-lines]')
    await panel.getByRole('heading', { name: '线路与探针', exact: true }).waitFor()
    await panel.getByText('共 3 条线路，2 条在线；当前套餐「标准」可用 2 条。', { exact: true }).waitFor()
    const cards = panel.locator('[data-slot=member-line]')
    assert.equal(await cards.count(), 3)
    const relay = cards.filter({ hasText: '日本家宽中转' })
    for (const text of ['转发线路', '动态家宽', '入口在线', '不在当前套餐', 'Hysteria2 · UDP · TLS', '倍率 2×（分时）']) await relay.getByText(text, { exact: true }).waitFor()
    assert.match(await relay.locator('[data-slot=line-route]').getAttribute('aria-label'), /香港 → 日本/)
    await panel.getByLabel('范围', { exact: true }).selectOption('mine')
    assert.deepEqual(await cards.evaluateAll(items => items.map(item => item.dataset.line)), ['11', '13'])
    await panel.getByLabel('排序', { exact: true }).selectOption('rate')
    assert.deepEqual(await cards.evaluateAll(items => items.map(item => item.dataset.line)), ['13', '11'])
    await panel.getByLabel('范围', { exact: true }).selectOption('all')
    await panel.getByLabel('类型', { exact: true }).selectOption('residential')
    assert.equal(await cards.count(), 1)
    await panel.getByLabel('搜索', { exact: true }).fill('不存在的线路')
    await panel.getByRole('button', { name: '清除筛选', exact: true }).click()
    assert.equal(await cards.count(), 3)

    await relay.getByRole('button', { name: '查看详情', exact: true }).click()
    const dialog = page.getByRole('dialog')
    await dialog.getByText('前置入口由目录信息推断，仅供参考。', { exact: true }).waitFor()
    await dialog.getByText('落地出口的网络属性未经面板测量；探针中的数据代表前置入口。', { exact: true }).waitFor()
    await dialog.getByText('高级', { exact: true }).waitFor()
    await dialog.getByRole('button', { name: '在探针中查看前置入口', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })

    const probe = panel.locator('[data-slot=probe-host]')
    await panel.locator('[data-slot=probe-host][data-focused]').waitFor()
    assert.equal(await probe.count(), 2)
    const hk = probe.filter({ hasText: 'AS64500' })
    assert.equal(await hk.getAttribute('data-focused'), 'true')
    for (const text of ['在线', '数据中心', '用途类型：isp', 'Example Network', '92.0%']) await hk.getByText(text, { exact: true }).first().waitFor()
    assert.equal(await hk.locator('[data-tone]').filter({ hasText: /^代理$/ }).count(), 0, 'unset flags stay out of the badges')
    await hk.getByText('数据源明细（2 个）', { exact: true }).click()
    await hk.getByText('滥用评分（上游原值）', { exact: true }).waitFor()
    await probe.filter({ hasText: '承载线路：美国 AI 分流' }).getByText('该主机尚未接入 apsp 设备，IP 质量暂未测量。', { exact: true }).waitFor()
    const text = await panel.innerText()
    assert.doesNotMatch(text, /\b\d{1,3}(?:\.\d{1,3}){3}\b|(?:[0-9a-f]{1,4}:){3,}|::[0-9a-f]{1,4}/i, 'addresses never reach the member view')

    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1), `viewport ${width} overflows`)
    assert.deepEqual(unexpected, [])
    assert.deepEqual(errors, [])
    await page.close()
    console.log(`member lines / probes passed at ${width}px`)
  }
} finally { if (browser) await browser.close(); await new Promise(resolve => server.close(resolve)) }
