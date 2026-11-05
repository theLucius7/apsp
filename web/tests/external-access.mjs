import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import { flatResourceFixtures, proxyResourceFixtures } from './proxy-resource-fixtures.mjs'
// TEST_ONLY: the policy group editor with isolated external-provider fixtures.
// Users get external nodes only through policy groups: the editor offers adopted
// nodes from subscription sources, keeps nodes the group already holds even while
// they are unavailable, sends exact node identities, refuses a draft whose chosen
// node changed identity (retaining the rest), survives a save conflict, and a
// user's subscription lists managed and external nodes side by side.
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { extname, resolve, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const diagnosisFixture = user => ({ account: { name: user.name, portal_created: false, keys: 0, active_sessions: 0, activation_expires_at: null },
  subscription: { status: 'ready', message: 'TEST_ONLY 真实设备状态未验证。', granted_nodes: 2, ready_managed_nodes: 1, ready_external_nodes: 1 },
  legacy: null, permissions: [], ledger: [], rotations: [], events: [],
  limitations: { credentials_read: { available: false, reason: 'TEST_ONLY 敏感内容未读取；此处仅为独立只读诊断快照。' } } })
const templateFixture = { template: null, definition_redacted: false, credential_access_reason: 'TEST_ONLY 完整模板未读取。', limitations: 'TEST_ONLY 没有保存的模板，未执行真实客户端验证。' }

const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = process.env.APSP_WEB_DIST ?? fileURLToPath(new URL('../dist/', import.meta.url))
const root = '/api/plugins/sing-box', candidatesPath = `${root}/external-node-candidates`, groupPath = `${root}/policy-groups/1`
const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': mime[extname(file)] ?? 'application/octet-stream' }); response.end(body) }
  catch { response.writeHead(404).end() }
})
const settle = () => new Promise(done => setTimeout(done, 40))
const wait = async (condition, description) => {
  const deadline = Date.now() + 8000
  while (!await condition()) { assert(Date.now() < deadline, description); await settle() }
}
const forceSubmit = dialog => dialog.locator('form').evaluate(form => form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true })))
const ref = entry => Object.fromEntries(['external_node_id', 'source_id', 'identity_epoch', 'node_version_id', 'update_mode', 'metadata_revision'].map(key => [key, entry[key]]))
const entry = (id, name, extra = {}) => ({ external_node_id: id, source_id: 8, identity_epoch: 2, node_version_id: id + 10, update_mode: 'follow_node', metadata_revision: 0, name, source_name: 'TEST_ONLY 外部来源', protocol: 'shadowsocks', server: 'provider.example.com', port: 443, available: true, reason: null, current_version_id: id + 10, resolved_version_id: id + 10, source_last_error: null, ...extra })
const node = { enabled: true, id: 1, name: 'TEST_ONLY 受管入口', server_id: 1, protocol: 'vless-reality', public_host: '127.0.0.1', port: 24443, sni: 'localhost', public_key: 'TEST_ONLY', short_id: '0123abcd' }
const servers = [{ id: 1, name: 'TEST_ONLY 服务器', enabled: true, online: true, read_only: false }]
const user = { id: 1, name: 'TEST_ONLY 用户', subscription_token: 'TEST_ONLY', subscription_url: 'http://127.0.0.1/sub/TEST_ONLY' }
let browser
try {
  await new Promise(done => server.listen(0, '127.0.0.1', done))
  const origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  for (const width of [1440, 390, 340]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } }), errors = [], writes = []
    const first = entry(1, 'TEST_ONLY 原分配'), second = entry(2, 'TEST_ONLY 新节点')
    const disabled = entry(3, 'TEST_ONLY 已停用节点', { available: false, reason: 'node_disabled' })
    const archived = entry(4, 'TEST_ONLY 已归档候选', { available: false, reason: 'source_archived' })
    let candidates = { available_nodes: [first, second, disabled, archived] }
    const group = { id: 1, name: 'TEST_ONLY 外部策略', node_ids: [1], chain_ids: [], member_count: 2, external_nodes: [first, disabled] }
    let putConflict = true, gate, release, subscriptionReads = 0
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/*', async route => {
      const request = route.request(), url = new URL(request.url()), path = url.pathname, method = request.method()
      assert.equal(url.origin, origin, 'Provider/external requests must never leave the browser fixture')
      if (!path.startsWith('/api/')) return route.continue()
      if (path === groupPath && method === 'PUT') {
        const body = request.postDataJSON(); writes.push(body)
        if (putConflict) { putConflict = false; return route.fulfill({ status: 409, json: { error: 'TEST_ONLY 策略组已变化' } }) }
        Object.assign(group, { name: body.name, node_ids: body.node_ids, chain_ids: body.chain_ids, external_nodes: body.external_nodes.map(value => ({ ...candidates.available_nodes.find(item => item.external_node_id === value.external_node_id), ...value })) })
        return route.fulfill({ json: group })
      }
      assert.equal(method, 'GET', `Unexpected write ${method} ${path}`)
      if (path === candidatesPath && gate) { const pending = gate; pending.reached++; await pending.promise }
      let value
      if (path === '/api/dashboard/access') value = { authenticated: true, public_dashboard: false }
      else if (path === `${root}/policy-groups`) value = [group]
      else if (path === `${root}/package-groups`) value = []
      else if (path === `${root}/proxy-resources`) value = flatResourceFixtures([node], servers)
      else if (path === `${root}/ordered-proxy-resources`) value = proxyResourceFixtures([node], servers)
      else if (path === candidatesPath) value = structuredClone(candidates)
      else if (path === `${root}/users`) value = [user]
      else if (path === `${root}/usage`) value = { uplink: '0', downlink: '0', total: '0', by_user: [], by_node: [] }
      else if (path === `${root}/users/1/policy-groups`) value = { group_ids: [1] }
      else if (path === `${root}/users/1/entitlement`) value = { user_id: 1, status: 'unlimited', allowed: true, monthly_bytes: null, used_bytes: '0', expires_at: null }
      else if (path === `${root}/users/1/portal`) value = { configuration: { enabled: false, reason: 'TEST_ONLY', origin }, keys: 0, url: null, activation_expires_at: null }
      else if (!url.search && path === `${root}/users/1/diagnosis`) value = diagnosisFixture(user)
      else if (!url.search && path === `${root}/users/1/client-template`) value = templateFixture
      else if (path === `${root}/users/1/subscription`) {
        subscriptionReads++
        const format = url.searchParams.get('format') ?? 'singbox'
        value = { format, status: format === 'singbox' ? 'ready' : 'format_unavailable', message: 'TEST_ONLY 订阅', subscription_url: `${origin}/sub/TEST_ONLY`, available_formats: ['singbox'], granted_nodes: 2, eligible_nodes: 2, managed_nodes: 1, external_nodes: 1, external_granted_nodes: 1, ready_nodes: [{ kind: 'managed', id: 1, name: 'TEST_ONLY 受管入口', protocol: 'vless-reality' }, { kind: 'external', id: 1, name: 'TEST_ONLY 外部入口', protocol: 'shadowsocks' }], content: format === 'singbox' ? '{"outbounds":[]}' : null, filename: 'TEST_ONLY.json', content_type: 'application/json', entitlement: { allowed: true, status: 'unlimited', monthly_bytes: null, used_bytes: '0', expires_at: null } }
      } else assert.fail(`Unexpected read ${path}`)
      await route.fulfill({ json: value }).catch(() => {})
    })
    try {
      await installControlCenterFixtures(page)
      await page.goto(`${origin}/#/plugins/sing-box/groups`)
      const row = page.getByRole('row').filter({ hasText: group.name })
      await row.getByText('TEST_ONLY 受管入口、TEST_ONLY 原分配（外部）、TEST_ONLY 已停用节点（外部）', { exact: true }).waitFor()
      const edit = row.getByRole('button', { name: '编辑', exact: true })
      await wait(() => edit.isEnabled(), 'Initial group snapshots complete')
      await edit.click()
      const dialog = page.getByRole('dialog'), save = dialog.getByRole('button', { name: '保存', exact: true })
      const externals = dialog.locator('fieldset[data-slot=group-choices]').filter({ has: page.getByText('订阅来源中的外部节点', { exact: true }) })
      const choice = name => externals.getByRole('checkbox', { name: new RegExp(name) })
      const mode = name => externals.getByRole('combobox', { name: `${name} 的更新方式`, exact: true })
      await externals.waitFor()
      assert.equal(await externals.getByRole('checkbox').count(), 4, 'held nodes and adopted candidates are listed once each')
      // A held node stays chosen while unavailable; an unavailable candidate cannot be added.
      assert.equal(await choice('TEST_ONLY 原分配').isChecked(), true)
      assert.equal(await choice('TEST_ONLY 已停用节点').isChecked(), true)
      assert.equal(await choice('TEST_ONLY 已停用节点').isEnabled(), true)
      await externals.getByText('TEST_ONLY 外部来源 · shadowsocks · 节点已停用', { exact: true }).waitFor()
      assert.equal(await choice('TEST_ONLY 已归档候选').isDisabled(), true)
      await externals.getByText('TEST_ONLY 外部来源 · shadowsocks · 来源已归档', { exact: true }).waitFor()
      assert.equal(await mode('TEST_ONLY 原分配').inputValue(), 'follow_node')
      await mode('TEST_ONLY 原分配').selectOption('pinned')
      await choice('TEST_ONLY 新节点').check()
      assert.equal(await mode('TEST_ONLY 新节点').inputValue(), 'follow_node')
      assert.equal(await dialog.evaluate(element => element.scrollWidth <= element.clientWidth), true, 'Group editor fits viewport')
      const refresh = () => page.locator('[data-slot=page-header]').getByRole('button', { name: '刷新', exact: true, includeHidden: true }).evaluate(button => button.click())
      const draftKept = async (label, listed = true) => {
        assert.equal(await mode('TEST_ONLY 原分配').inputValue(), 'pinned', `${label}: pinned mode kept`)
        if (listed) assert.equal(await choice('TEST_ONLY 新节点').isChecked(), true, `${label}: added node kept`)
        assert.equal(await dialog.locator('input[name="name"]').inputValue(), group.name)
      }
      // A candidate read in flight leaves the draft submittable (ADR 0102).
      gate = { reached: 0, promise: new Promise(done => { release = done }) }
      const pending = gate
      await refresh(); await wait(() => pending.reached > 0, 'Candidate refresh starts')
      await wait(() => save.isEnabled(), 'A held candidate read leaves the draft submittable')
      gate = undefined; release()
      const original = structuredClone(candidates)
      const blocked = '所选外部节点已缺失、停用或来源与版本已变化，请重新选择；其余草稿已保留。'
      for (const [label, mutate] of [
        ['node disappeared', value => { value.available_nodes = value.available_nodes.filter(item => item.external_node_id !== second.external_node_id) }],
        ['source replaced', value => { value.available_nodes[1].source_id = 9; value.available_nodes[1].identity_epoch++ }],
        ['node disabled', value => { value.available_nodes[1].available = false; value.available_nodes[1].reason = 'node_disabled' }],
        ['newer version not applied', value => { value.available_nodes[1].current_version_id++ }],
        ['metadata changed', value => { value.available_nodes[1].metadata_revision++ }],
      ]) {
        candidates = structuredClone(original); mutate(candidates)
        const changed = page.waitForResponse(response => new URL(response.url()).pathname === candidatesPath && response.status() === 200)
        await refresh(); await changed
        await wait(() => save.isDisabled(), `${label} blocks saving`)
        await dialog.getByText(blocked, { exact: true }).waitFor()
        await forceSubmit(dialog); assert.equal(writes.length, 0, `${label} must issue zero PUTs`)
        // A chosen node that vanished from the candidates has no row of its own.
        await draftKept(label, label !== 'node disappeared')
        candidates = structuredClone(original)
        const restored = page.waitForResponse(response => new URL(response.url()).pathname === candidatesPath && response.status() === 200)
        await refresh(); await restored
        await wait(() => save.isEnabled(), `${label}: original identity recovers`)
        await draftKept(`${label} recovered`)
      }
      // A save conflict keeps the draft; the next save sends exact identities.
      await save.click()
      await dialog.getByText('TEST_ONLY 策略组已变化', { exact: true }).waitFor()
      assert.equal(writes.length, 1)
      assert.deepEqual(writes[0], { name: group.name, node_ids: [1], chain_ids: [], external_nodes: [{ ...ref(first), update_mode: 'pinned' }, ref(disabled), ref(second)] })
      await draftKept('conflict')
      await save.click(); await dialog.waitFor({ state: 'hidden' })
      assert.equal(writes.length, 2)
      assert.deepEqual(writes[1], writes[0])
      await row.getByText('TEST_ONLY 受管入口、TEST_ONLY 原分配（外部）、TEST_ONLY 已停用节点（外部）、TEST_ONLY 新节点（外部）', { exact: true }).waitFor()
      // Reopening shows the saved pin from the group itself.
      await edit.click()
      assert.equal(await mode('TEST_ONLY 原分配').inputValue(), 'pinned')
      await dialog.getByRole('button', { name: '取消', exact: true }).click(); await dialog.waitFor({ state: 'hidden' })
      const overflow = await page.evaluate(() => [...document.querySelectorAll('body *')].filter(element => element.getBoundingClientRect().right > innerWidth + 1).map(element => ({ tag: element.tagName, class: element.className, text: element.textContent?.slice(0, 70) })))
      assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, `Page fits viewport: ${JSON.stringify(overflow.slice(0, 12))}`)

      // The user's subscription lists managed and external nodes without ID collisions.
      await page.goto(`${origin}/#/plugins/sing-box/users`)
      await page.getByRole('button', { name: '订阅详情', exact: true }).click()
      const subscription = page.getByRole('dialog')
      await subscription.getByText('TEST_ONLY 外部入口', { exact: true }).waitFor()
      await subscription.getByText('TEST_ONLY 受管入口', { exact: true }).waitFor()
      assert.equal(await subscription.locator('[data-slot=subscription-nodes] li').count(), 2, 'Managed and external IDs do not collide')
      assert.equal(await subscription.getByRole('combobox').inputValue(), 'singbox')
      assert.equal(await subscription.locator('option[value=links]').evaluate(option => option.disabled), true)
      assert(subscriptionReads > 0)
      assert.deepEqual(errors, [])
      console.log(`external nodes in policy groups ${width}: held/unavailable listing, exact identities, zero-write identity matrix, conflict recovery and mixed subscription passed`)
    } finally { release?.(); await page.close() }
  }
} finally { await browser?.close(); await new Promise(done => server.close(done)) }
