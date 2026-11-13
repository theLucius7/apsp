import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile, mkdir } from 'node:fs/promises'
import { extname, resolve, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

// TEST_ONLY: the line quality overview (ADR 0130) — one row per server with its three
// carriers and China reachability, filters, details, and what a non-owner sees.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const root = fileURLToPath(new URL('../dist/', import.meta.url))
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(root, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(root.endsWith(sep) ? root : `${root}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' })[extname(file)] ?? 'application/octet-stream' }).end(body) }
  catch { response.writeHead(404).end() }
})
await new Promise(done => server.listen(0, '127.0.0.1', done))
const origin = `http://127.0.0.1:${server.address().port}`
const browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
const screenshots = process.env.APSP_UI_SCREENSHOT_DIR
if (screenshots) await mkdir(screenshots, { recursive: true })

const now = Date.now(), second = Math.floor(now / 1000)
const id = n => `00000000-0000-0000-0000-${String(n).padStart(12, '0')}`
// name, region, group, base latency in ms; server 4 is offline.
const fleet = [['TEST_ONLY 香港', '🇭🇰', 'TEST_ONLY 云 A', 20], ['TEST_ONLY 东京', '🇯🇵', 'TEST_ONLY 云 B', 60], ['TEST_ONLY 孟买', '🇮🇳', 'TEST_ONLY 云 B', 180], ['TEST_ONLY 离线', '🇸🇬', 'TEST_ONLY 云 C', 90], ['TEST_ONLY 法兰克福', '🇩🇪', 'TEST_ONLY 云 D', 240], ['TEST_ONLY 仅可达性', '🇹🇼', 'TEST_ONLY 云 E', 0]]
const servers = fleet.map(([name, region, group], index) => ({ id: index + 1, name, online: index !== 3, asset_settings: { region, group_name: group, tags: [], hidden: false } }))
const carriers = [['telecom', '电信', '192.0.2.1'], ['unicom', '联通', '192.0.2.2'], ['mobile', '移动', '192.0.2.3']]
const identity = target => ({ kind: 'icmp', target, port: null, address_family: 'any' })
const spec = ([network, label, target], n) => ({ id: id(n), name: `TEST_ONLY 广东${label}`, kind: 'icmp', target, port: null, interval_secs: 60, carrier: '', enabled: true, execution_authorized: true, monitor: { network, region: '广东', address_family: 'any', authorization: { kind: 'owned', source: 'TEST_ONLY', scope: 'TEST_ONLY', enabled: true, expires_at: null, identity: identity(target) } } })
const tasks = carriers.map((carrier, index) => ({ id: id(index + 1), spec: spec(carrier, index + 1), default_enabled: true, server_ids: [1, 2, 3, 4, 5], revision: 1 }))
const overview = servers.slice(0, 5).flatMap(item => carriers.map((carrier, index) => {
  const probe = spec(carrier, 100 + item.id * 10 + index), base = fleet[item.id - 1][3] + index * 5
  const results = Array.from({ length: 20 }, (_, k) => ({ id: id(1000 + item.id * 100 + index * 20 + k), probe_id: probe.id, sampled_at: (item.id === 4 ? now - 3_600_000 : now - 10_000) - k * 60_000, latency_ms: base + (k % 3), loss_percent: item.id === 3 && index === 1 && k % 4 === 0 ? 25 : 0, error: null, attempts: 4, address_family: 'ipv4' }))
  return { server_id: item.id, probe, results }
}))
const node = serverId => ({ kind: 'native', id: serverId, name: `${servers[serverId - 1].name} 节点`, server_id: serverId, server_name: servers[serverId - 1].name, protocol: 'vless-reality' })
const china = connected => ({ probes: ['telecom', 'unicom', 'mobile'].map(carrier => ({ carrier, network: carrier, city: 'TEST_ONLY', connected, avg_ms: connected ? 40 : null })), error: null })
const endpoint = (key, serverId, state) => ({ key, host: `${key}.example.invalid`, port: 443, tcp: true, state, blocked: state.endsWith('blocked'), pending_state: null, failing_streak: 0, observed_state: state, resolved_ip: '192.0.2.10', detail: { china: china(state === 'reachable'), overseas: { probes: [{ carrier: 'other', network: 'other', city: 'TEST_ONLY', connected: true, avg_ms: 30 }], error: null } }, checked_at: second - 600, state_changed_at: null, deferred_at: null, nodes: [node(serverId)], risks: [] })
const reachability = {
  settings: { enabled: true, interval_minutes: 60, minimum_interval_minutes: 15, maximum_interval_minutes: 1440, token_configured: false, revision: 1, next_run_at: second + 600, run_requested: false, running: false, last_run: { started_at: second - 700, finished_at: second - 600, measured: 5, tests_used: 20 } },
  targets: [endpoint('hk', 1, 'reachable'), endpoint('tokyo', 2, 'port_blocked'), endpoint('mumbai', 3, 'reachable'), endpoint('frankfurt', 5, 'reachable'), endpoint('only', 6, 'partial')],
  events: [{ id: id(900), kind: 'blocked', from_state: 'reachable', to_state: 'port_blocked', message: 'TEST_ONLY 东京 443 端口在国内连续两轮无法连接', created_at: second - 3600, deliveries: [{ channel: 'telegram', state: 'sent', attempts: 1, last_error: null }] }],
}

const results = []
try {
  for (const [width, role] of [[1440, 'owner'], [390, 'owner'], [320, 'owner'], [1440, 'operator']]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 } })
    const page = await context.newPage(), errors = [], unexpected = [], external = [], requested = []
    page.on('pageerror', error => errors.push(error.message))
    await context.route('**/*', async route => {
      const request = route.request(), url = new URL(request.url()), path = url.pathname, method = request.method()
      if (url.origin !== origin) { external.push(request.url()); return route.abort() }
      if (!path.startsWith('/api/')) return route.continue()
      requested.push(path)
      const respond = json => route.fulfill({ json })
      if (method !== 'GET') { unexpected.push(`${method} ${path}`); return route.fulfill({ status: 500, json: { error: 'TEST_ONLY read only' } }) }
      if (path === '/api/dashboard/access') return respond({ authenticated: true, public_dashboard: false })
      if (path === '/api/servers') return respond(servers)
      if (path === '/api/probes/overview') return respond(overview)
      if (path === '/api/latency-tasks') return respond(tasks)
      if (path === '/api/observability/reachability') return respond(reachability)
      const probes = path.match(/^\/api\/servers\/(\d+)\/probes$/)
      if (probes) return respond(overview.filter(row => row.server_id === Number(probes[1])).map(row => row.probe))
      const history = path.match(/^\/api\/servers\/(\d+)\/probe-results$/)
      if (history) return respond(overview.filter(row => row.server_id === Number(history[1]) && row.probe.id === url.searchParams.get('probe_id')).flatMap(row => row.results))
      unexpected.push(`${method} ${path}`); return route.fulfill({ status: 500, json: { error: 'Unexpected fixture API' } })
    })
    await installControlCenterFixtures(page)
    if (role !== 'owner') await page.route('**/api/control-center/me', route => route.fulfill({ json: { admin_id: 2, name: 'TEST_ONLY 运维', role, global_servers: true, server_ids: [], capabilities: ['proxy:read', 'probe:read'] } }))
    // The old reachability address opens the overview.
    await page.goto(`${origin}/#/latency/reachability`)
    await page.getByRole('heading', { name: '质量概况', exact: true }).waitFor()
    const list = page.getByRole('list', { name: '服务器线路列表' })
    const line = name => list.locator('[data-slot=server-line]').filter({ has: page.getByRole('link', { name, exact: true }) })
    const names = async () => (await list.locator('[data-slot=server-line] a[href^="#/servers/"]').allInnerTexts()).filter(text => text.startsWith('TEST_ONLY')).filter((value, index, all) => all.indexOf(value) === index)
    await line('TEST_ONLY 香港').getByRole('group', { name: '电信', exact: true }).getByText('20.0 ms', { exact: true }).waitFor()
    const owner = role === 'owner'
    // Reachability arrives after the probes for the owner; wait for it before counting.
    if (owner) await line('TEST_ONLY 东京').getByRole('group', { name: '中国可达性' }).getByText('端口被封锁', { exact: true }).waitFor()

    // Figures: what needs attention, each carrier's median, reachable endpoints.
    const strip = page.getByRole('region', { name: '线路质量' })
    const figure = label => strip.locator('[data-slot=overview-item]').filter({ has: page.locator('[data-slot=overview-label]', { hasText: label }) })
    assert.match(await figure('需要关注').innerText(), owner ? /4\s*台[\s\S]*共 6 台服务器/ : /2\s*台[\s\S]*共 6 台服务器/)
    assert.match(await figure('电信 · 广东').innerText(), /120\.0\s*ms/)
    assert.match(await figure('联通 · 广东').innerText(), /1 台丢包偏高/)
    assert.equal(await figure('中国可达性').count(), owner ? 1 : 0)
    if (owner) assert.match(await figure('中国可达性').innerText(), /3\/5[\s\S]*1 个端点被封锁/)
    assert.equal(requested.includes('/api/observability/reachability'), owner, 'Only the owner reads reachability')

    // Problems first: offline, loss and blocked endpoints; distance alone is not a problem.
    const problemRows = owner ? ['TEST_ONLY 东京', 'TEST_ONLY 离线', 'TEST_ONLY 孟买', 'TEST_ONLY 仅可达性'] : ['TEST_ONLY 离线', 'TEST_ONLY 孟买']
    assert.deepEqual((await names()).slice(0, problemRows.length).sort(), [...problemRows].sort())
    assert.equal(await line('TEST_ONLY 法兰克福').locator('[data-slot=line-problems]').count(), 0)
    assert.match(await line('TEST_ONLY 离线').locator('[data-slot=line-problems]').innerText(), /服务器离线/)
    assert.match(await line('TEST_ONLY 孟买').locator('[data-slot=line-problems]').innerText(), /联通丢包 \d+\.\d%/)
    for (const label of ['电信', '联通', '移动']) assert.match(await line('TEST_ONLY 离线').getByRole('group', { name: label, exact: true }).innerText(), /采样已过期/)
    if (owner) {
      assert.match(await line('TEST_ONLY 东京').locator('[data-slot=line-problems]').innerText(), /中国可达性：端口被封锁/)
      // A server measured only from China keeps its row.
      assert.match(await line('TEST_ONLY 仅可达性').getByRole('group', { name: '中国可达性' }).innerText(), /部分运营商可达/)
      for (const label of ['电信', '联通', '移动']) assert.match(await line('TEST_ONLY 仅可达性').getByRole('group', { name: label, exact: true }).innerText(), /未分配/)
    } else assert.equal(await list.getByRole('group', { name: '中国可达性' }).count(), 0)

    // Filter, sort and search.
    await page.getByRole('button', { name: /^需要关注/ }).click()
    assert.deepEqual((await names()).sort(), [...problemRows].sort())
    await page.getByRole('button', { name: /^全部/ }).click()
    await page.getByLabel('排序').selectOption('latency')
    assert.deepEqual((await names()).slice(0, 2), ['TEST_ONLY 香港', 'TEST_ONLY 东京'])
    assert.deepEqual((await names()).slice(-2).sort(), ['TEST_ONLY 仅可达性', 'TEST_ONLY 离线'].sort(), 'Rows without readings go last')
    await page.getByLabel('排序').selectOption('loss')
    assert.equal((await names())[0], 'TEST_ONLY 孟买')
    await page.getByLabel('搜索服务器').fill('云 b')
    assert.deepEqual((await names()).sort(), ['TEST_ONLY 东京', 'TEST_ONLY 孟买'])
    await page.getByLabel('搜索服务器').fill('不存在')
    await page.getByText('没有匹配的服务器。', { exact: true }).waitFor()
    await page.getByLabel('搜索服务器').fill('')

    // Details: what is measured, the history and the endpoints, with links onward.
    await line('TEST_ONLY 东京').getByRole('button', { name: 'TEST_ONLY 东京 详情' }).click()
    const targets = line('TEST_ONLY 东京').getByRole('region', { name: '检测目标' })
    await targets.waitFor()
    assert.match(await targets.innerText(), /TEST_ONLY 广东电信 · ICMP 回显 · 192\.0\.2\.1 · IPv4/)
    await line('TEST_ONLY 东京').getByRole('combobox', { name: '拨测目标' }).waitFor()
    assert.equal(await line('TEST_ONLY 东京').getByRole('link', { name: '服务器详情' }).getAttribute('href'), '#/servers/2')
    assert.equal(await line('TEST_ONLY 东京').getByRole('link', { name: '线路诊断' }).getAttribute('href'), '#/servers/2/tcp-quality')
    if (owner) {
      const endpoints = line('TEST_ONLY 东京').getByRole('list', { name: '端点可达性' })
      assert.match(await endpoints.innerText(), /tokyo\.example\.invalid:443[\s\S]*端口被封锁/)
      await page.getByRole('region', { name: '可达性变化' }).getByText('TEST_ONLY 东京 443 端口在国内连续两轮无法连接', { exact: true }).waitFor()
    } else {
      assert.equal(await line('TEST_ONLY 东京').getByRole('list', { name: '端点可达性' }).count(), 0)
      assert.equal(await page.getByRole('region', { name: '可达性变化' }).count(), 0)
    }
    await line('TEST_ONLY 东京').getByRole('button', { name: 'TEST_ONLY 东京 详情' }).click()
    await targets.waitFor({ state: 'detached' })

    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `Page overflows at ${width}`)
    if (screenshots) await page.screenshot({ fullPage: true, animations: 'disabled', path: resolve(screenshots, `line-quality-${width}-${role}.png`) })
    assert.deepEqual(errors, []); assert.deepEqual(unexpected, []); assert.deepEqual(external, [])
    results.push({ width, role, requests: requested.length })
    await context.close()
  }
  console.log(JSON.stringify({ suite: 'line-quality', results }))
} finally { await browser.close(); await new Promise(done => server.close(done)) }
