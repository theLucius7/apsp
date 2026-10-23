// Exercise the built plugin UI in Chromium against isolated, stateful API fixtures.
const assert = require('node:assert/strict')
const fs = require('node:fs/promises')
const path = require('node:path')
const { createServer } = require('node:http')
const { pathToFileURL } = require('node:url')

async function main() {
  const dependency = process.env.APSP_PLAYWRIGHT_MODULE || process.env.PLAYWRIGHT_MODULE || 'playwright'
  const { chromium } = await import(path.isAbsolute(dependency) ? pathToFileURL(dependency).href : dependency)
  const { flatResourceFixtures, proxyResourceFixtures, orderedResourceFixture, pathFixtureUuid } = await import('../web/tests/proxy-resource-fixtures.mjs')
  const screenshots = process.env.APSP_UI_SCREENSHOT_DIR || process.env.APSP_GROUPS_SCREENSHOTS
  let server, browser
  try {
    let origin
    if (process.argv[2]) origin = new URL(process.argv[2])
    else {
      const dist = path.resolve(__dirname, '../web/dist')
      const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml', '.png': 'image/png', '.woff2': 'font/woff2' }
      server = createServer(async (request, response) => {
        const pathname = new URL(request.url, 'http://127.0.0.1').pathname
        const file = path.resolve(dist, pathname === '/' ? 'index.html' : `.${pathname}`)
        if (!file.startsWith(`${dist}${path.sep}`)) { response.writeHead(400).end(); return }
        try {
          const body = await fs.readFile(file)
          response.writeHead(200, { 'Content-Type': mime[path.extname(file)] || 'application/octet-stream', 'Cache-Control': 'no-store' }).end(body)
        } catch { response.writeHead(404).end() }
      })
      await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve) })
      origin = new URL(`http://127.0.0.1:${server.address().port}`)
    }
    assert(['http:', 'https:'].includes(origin.protocol) && ['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname))
    assert.equal(origin.username, ''); assert.equal(origin.password, '')
    browser = await chromium.launch({ headless: true, executablePath: process.env.APSP_CHROME_PATH || process.env.CHROMIUM_PATH || undefined })
    const page = await browser.newPage({ viewport: { width: 1440, height: 1100 } })
    page.setDefaultTimeout(10000)
    const errors = [], writes = [], assignments = []
    const root = '/api/plugins/sing-box'
    const nodes = [{ id: 1, name: '标准节点', server_id: 1 }, { id: 2, name: '入口节点', server_id: 1 }, { id: 3, name: '出口节点', server_id: 2 }, { id: 4, name: '备用入口', server_id: 1 }].map(n => ({ ...n, port: 20000 + n.id, protocol: 'vless-reality', public_host: 'proxy.example.com', sni: 'www.example.com' }))
    nodes.push({ ...nodes[0], id: 5, name: '现代节点', server_id: 2, protocol: 'shadowsocks2022' })
    nodes.push({ ...nodes[0], id: 6, name: '混合入口', port: 20006 })
    const chains = [{ id: 1, name: '两跳示例', entry_node_id: 2, exit_node_id: 3, available: true }]
    const servers = [1, 2].map(id => ({ id, name: `测试服务器 ${id}`, enabled: true, online: false, agent_supported: true, read_only: false, source: 'administrator' }))
    const chainReceipts = new Map()
    const policies = [{ id: 1, name: '常用节点', node_ids: [1], chain_ids: [1], member_count: 1 }]
    const plans = [{ id: 1, name: '月度套餐', monthly_bytes: '536870912000', reset_day: 31, reset_hour: 12, reset_minute: 30, timezone: 'Asia/Taipei', duration_days: 365 }]
    let groupIds = [1], assigned = false, exhausted = false, legacyChainReads = 0
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/api/**', async route => {
      const request = route.request(), pathname = new URL(request.url()).pathname, method = request.method()
      const payload = method === 'GET' ? null : request.postDataJSON()
      if (method !== 'GET') writes.push({ pathname, method, payload })
      let data = []
      if (pathname === '/api/dashboard/access') data = { authenticated: true, public_dashboard: false }
      else if (pathname === '/api/me') data = {}
      else if (pathname === `${root}/nodes`) data = nodes
      else if (method === 'GET' && pathname === `${root}/servers`) data = servers
      else if (method === 'GET' && [`${root}/subscription-sources`, `${root}/ordered-subscription-sources`].includes(pathname)) data = []
      else if (method === 'GET' && pathname === `${root}/proxy-resources`) data = flatResourceFixtures(nodes.filter(node => node.id !== 6), servers, chains)
      else if (method === 'GET' && pathname === `${root}/ordered-proxy-resources`) {
        const resources = proxyResourceFixtures(nodes, servers, chains).map(resource => {
          if (resource.kind !== 'chain' || resource.id === 1) return resource
          const ordered = orderedResourceFixture({ id: resource.id, name: resource.name, entry: resource.entry,
            hops: [{ kind: 'managed', position: 1, node_id: resource.exit.id, endpoint_version_id: pathFixtureUuid(1000 + resource.exit.id), endpoint: resource.exit }] })
          ordered.path_state.capabilities = { tcp: true, udp: true }
          return ordered
        })
        const entry = resources.find(resource => resource.kind === 'direct' && resource.id === 6).entry
        data = [...resources.filter(resource => resource.kind !== 'direct' || resource.id !== 6), orderedResourceFixture({
          id: 100, name: '混合示例', entry, hops: [{ kind: 'subscription', position: 1, source_id: 9, source_name: '测试来源', identity_epoch: 1,
            external_node_id: pathFixtureUuid(901), node_version_id: pathFixtureUuid(902), source_revision_id: pathFixtureUuid(903),
            update_mode: 'pinned', name: '外部中间段', protocol: 'shadowsocks', server: 'external.example.net', server_port: 23456,
            sni: null, transport: null, capabilities: { tcp: true, udp: true }, source_archived: false, node_present: true, update_error: null },
            { kind: 'managed', position: 2, node_id: 3, endpoint_version_id: pathFixtureUuid(3), endpoint: resources.find(resource => resource.kind === 'direct' && resource.id === 3).entry }],
        })]
      }
      else if (method === 'POST' && pathname === `${root}/chains/ordered-batch`) {
        assert.match(payload.request_id, /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/)
        assert.deepEqual(payload.items, [{ name: '测试链路', entry: { mode: 'existing', node_id: 4 }, hops: [{ kind: 'managed', node_id: 3 }] }])
        const previous = chainReceipts.get(payload.request_id)
        if (previous) {
          assert.deepEqual(payload, previous.body)
          data = previous.receipt
        } else {
          const chain = { id: Math.max(...chains.map(chain => chain.id)) + 1, name: payload.items[0].name, entry_node_id: 4, exit_node_id: 3, available: true, path_kind: 'ordered' }
          data = { request_id: payload.request_id, chain_ids: [chain.id], entry_node_ids: [4] }
          chains.push(chain)
          chainReceipts.set(payload.request_id, { body: structuredClone(payload), receipt: data })
          await route.fulfill({ status: 201, json: data }); return
        }
      }
      else if (pathname === `${root}/chains`) {
        legacyChainReads += 1
        assert.equal(method, 'GET', 'The resource editor must use one atomic batch POST')
        data = chains
      } else if (pathname === `${root}/policy-groups`) {
        if (method === 'POST') { data = { ...payload, id: policies.length + 1, member_count: 0 }; policies.push(data) }
        else data = policies
      } else if (pathname === `${root}/package-groups`) {
        if (method === 'POST') { data = { ...payload, id: plans.length + 1 }; plans.push(data) }
        else data = plans
      } else if (pathname === `${root}/users`) data = [{ id: 1, name: '测试用户', subscription_token: 'TEST_ONLY', subscription_url: 'https://panel.example.com/s/TEST_ONLY' }]
      else if (pathname === `${root}/usage`) data = { uplink: '40', downlink: '60', total: '100', by_user: [{ user_id: 1, name: '测试用户', deleted: false, uplink: '40', downlink: '60' }], by_node: [] }
      else if (pathname === `${root}/users/1/accesses`) data = groupIds.includes(1) ? [{ user_id: 1, node_id: 1, uuid: 'TEST_ONLY', stat_name: 'u1_n1', direct_grant: false }, { user_id: 1, node_id: 2, uuid: 'TEST_ONLY_CHAIN', stat_name: 'u1_n2', direct_grant: false }] : []
      else if (pathname === `${root}/users/1/portal`) data = { configuration: { enabled: false, reason: 'TEST_ONLY 未启用', origin: origin.origin }, keys: 0, url: null, activation_expires_at: null }
      else if (pathname === `${root}/users/1/policy-groups`) {
        if (method === 'PUT') groupIds = payload.group_ids
        data = { group_ids: groupIds }
      } else if (pathname === `${root}/users/1/entitlement`) data = {
        user_id: 1, package_group_id: assigned ? 1 : null, package_name: assigned ? '月度套餐' : null,
        monthly_bytes: assigned ? '536870912000' : null, reset_day: 31, reset_hour: 12, reset_minute: 30, timezone: 'Asia/Taipei',
        starts_at: 1790812800, expires_at: 1822348800, cycle_start: 1790742600, next_reset: 1793421000,
        used_bytes: exhausted ? '536870912000' : '100', status: assigned ? exhausted ? 'exhausted' : 'active' : 'unmetered', allowed: !exhausted,
      }
      else if (pathname === `${root}/users/1/package`) {
        assignments.push(payload)
        if (assignments.length === 1) { await route.fulfill({ status: 503, json: { error: '测试：响应丢失，请重试' } }); return }
        assigned = true; data = { id: 1, user_id: 1, package_group_id: 1, starts_at: 1790812800, expires_at: 1822348800 }
      }
      await route.fulfill({ json: data })
    })
    await page.goto(`${origin.origin}/#/plugins/sing-box/groups`)
    await page.getByRole('button', { name: '创建策略组', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('测试策略')
    await page.getByRole('checkbox', { name: /标准节点/ }).check()
    await page.getByRole('checkbox', { name: /现代节点/ }).check()
    await page.getByRole('checkbox', { name: /两跳示例/ }).check()
    await page.getByRole('checkbox', { name: /混合示例/ }).check()
    assert.equal(await page.locator('input[name="node_ids"][value="2"]').count(), 0)
    assert.equal(await page.locator('input[name="node_ids"][value="6"]').count(), 0)
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await page.getByText('测试策略', { exact: true }).waitFor()
    assert.deepEqual(writes.at(-1).payload, { name: '测试策略', node_ids: [1, 5], chain_ids: [1, 100] })
    assert.equal(legacyChainReads, 0, 'Policy groups must read complete resources instead of legacy chains')
    await page.getByRole('button', { name: '套餐组', exact: true }).click()
    await page.getByRole('button', { name: '创建套餐组', exact: true }).click()
    await page.getByRole('textbox', { name: '名称', exact: true }).fill('精确额度')
    await page.locator('input[name="amount"]').fill('512')
    await page.locator('input[name="reset_day"]').fill('31')
    await page.locator('input[name="reset_time"]').fill('02:30')
    await page.locator('input[name="timezone"]').fill('Asia/Taipei')
    await page.locator('input[name="duration_days"]').fill('90')
    await page.getByRole('button', { name: '保存', exact: true }).click()
    await page.getByText('精确额度', { exact: true }).waitFor()
    assert.deepEqual(writes.at(-1).payload, { name: '精确额度', monthly_bytes: '549755813888', reset_day: 31, reset_hour: 2, reset_minute: 30, timezone: 'Asia/Taipei', duration_days: 90 })
    await page.goto(`${origin.origin}/#/plugins/sing-box/nodes?kind=chains`)
    await page.getByRole('link', { name: '链路', exact: true }).waitFor()
    await page.getByRole('button', { name: '创建两跳链路', exact: true }).click()
    await page.locator('select[name="entry_mode"]').selectOption('existing')
    await page.locator('input[name="name"]').fill('测试链路')
    assert.equal(await page.locator('select[name="entry_node_id"] option[value="5"]').count(), 0)
    assert.equal(await page.locator('select[name="exit_node_id"] option[value="5"]').count(), 0)
    await page.locator('select[name="entry_node_id"]').selectOption('4')
    await page.locator('select[name="exit_node_id"]').selectOption('3')
    await page.getByRole('button', { name: '创建未授权链路', exact: true }).click()
    await page.getByText('测试链路', { exact: true }).waitFor()
    const chainWrites = writes.filter(write => write.pathname === `${root}/chains/ordered-batch`)
    assert.equal(chainWrites.length, 1)
    assert.equal(chainWrites[0].method, 'POST')
    assert.deepEqual(chainWrites[0].payload.items, [{ name: '测试链路', entry: { mode: 'existing', node_id: 4 }, hops: [{ kind: 'managed', node_id: 3 }] }])
    assert.equal(writes.filter(write => write.pathname === `${root}/chains`).length, 0)
    assert.equal(chains.filter(chain => chain.name === '测试链路').length, 1)
    await page.goto(`${origin.origin}/#/plugins/sing-box/users`)
    await page.getByRole('heading', { name: '可用范围与套餐', exact: true }).waitFor()
    await page.getByRole('button', { name: '订阅链接', exact: true }).click()
    assert.equal(await page.getByRole('combobox', { name: '订阅格式' }).inputValue(), 'singbox')
    await page.getByRole('button', { name: '完成', exact: true }).click()
    await page.getByText('来自策略组', { exact: true }).waitFor()
    assert.equal(await page.getByRole('checkbox', { name: '授权 入口节点', exact: true }).count(), 0)
    assert.equal(await page.getByRole('checkbox', { name: '授权 标准节点', exact: true }).isChecked(), false)
    await page.getByRole('checkbox', { name: /常用节点/ }).uncheck()
    await page.getByRole('button', { name: '保存策略组分配', exact: true }).click()
    await page.getByText('策略组分配已保存。单独授权仍保留，设备应用配置后更新可用节点。', { exact: true }).waitFor()
    assert.deepEqual(writes.at(-1).payload, { group_ids: [] })
    await page.getByRole('button', { name: '分配或更换套餐', exact: true }).click()
    await page.locator('select[name="package_group_id"]').selectOption('1')
    await page.getByRole('button', { name: '确认分配', exact: true }).click()
    await page.getByText('测试：响应丢失，请重试', { exact: true }).waitFor()
    await page.getByRole('button', { name: '确认分配', exact: true }).click()
    await page.getByText('套餐已分配，按分配时刻计算有效期。本期历史用量没有清空。', { exact: true }).waitFor()
    assert.equal(assignments.length, 2)
    assert.deepEqual(assignments[0], assignments[1])
    assert.match(assignments[0].request_id, /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/)
    await page.getByText('本期已用 / 每月额度', { exact: true }).waitFor()
    exhausted = true
    await page.reload()
    await page.getByText('本期流量已用完', { exact: true }).waitFor()
    if (screenshots) {
      await fs.mkdir(screenshots, { recursive: true })
      await page.screenshot({ path: path.join(screenshots, 'user-desktop.png'), fullPage: true })
    }
    await page.setViewportSize({ width: 390, height: 844 })
    assert(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth))
    if (screenshots) await page.screenshot({ path: path.join(screenshots, 'user-mobile.png'), fullPage: true })
    assert.deepEqual(errors, [])
    console.log('PASS: policy/package/chain forms, precise quota, scoped grants, stable assignment retry, expiry display, desktop/mobile Chromium')
  } finally {
    try { await browser?.close() }
    finally { if (server?.listening) await new Promise(resolve => server.close(resolve)) }
  }
}
main().catch(error => { console.error(error); process.exitCode = 1 })
