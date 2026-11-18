import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { resolve, extname, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'

// TEST_ONLY browser responses. No live account, subscription, bridge, or Agent is accessed.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) return response.writeHead(400).end()
  try { response.writeHead(200, { 'Content-Type': ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' })[extname(file)] ?? 'application/octet-stream' }).end(await readFile(file)) } catch { response.writeHead(404).end() }
})
const entitlement = { user_id: 1, status: 'active', allowed: true, package_group_id: null, package_name: 'TEST_ONLY', monthly_bytes: '1000', used_bytes: '10', starts_at: null, expires_at: null, cycle_start: 1, next_reset: 2000000000 }
let browser
try {
  await new Promise(done => server.listen(0, '127.0.0.1', done))
  const origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  for (const width of [1440, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } })
    page.setDefaultTimeout(10000)
    await page.emulateMedia({ reducedMotion: 'reduce' })
    const errors = [], adopted = [], unexpected = []
    let legacy = true, blocked = false, blockers = ['TEST_ONLY 一条线路尚未就绪'], previewCalls = 0, authenticated = false, conflict = true, lastPreview
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname, method = request.method()
      let value
      if (path.endsWith('/native-adoption/preview')) {
        previewCalls += 1
        assert.equal(method, 'POST')
        if (!authenticated) return route.fulfill({ status: 403, json: { error: '请先再次验证管理员 TEST_ONLY' } })
        lastPreview = { digest: (previewCalls % 2 ? 'a' : 'b').repeat(64), adopted: !legacy, blocked,
          comparison_modes: blocked ? ['blocked'] : ['singbox', 'clash', 'links'],
          blockers, old_subscription_url: 'https://legacy.example.invalid/sub/TEST_ONLY_SUBSCRIPTION_SECRET',
          route_confirmation_required: true, entitlement, granted_nodes: 1, managed_nodes: 1, external_nodes: 0,
          formats: blocked ? {} : { singbox: '{"TEST_ONLY_CREDENTIAL":"fixture"}', clash: 'TEST_ONLY_CREDENTIAL: fixture', links: 'vless://TEST_ONLY_CREDENTIAL@example.invalid:443' },
          unavailable_formats: blocked ? { singbox: 'TEST_ONLY 当前暂停', clash: 'TEST_ONLY 当前暂停', links: 'TEST_ONLY 当前暂停' } : {},
        }
        value = lastPreview
      } else if (path.endsWith('/native-adoption/adopt')) {
        assert.equal(method, 'POST')
        const body = request.postDataJSON()
        adopted.push(body)
        assert.equal(body.expected_digest, lastPreview.digest)
        assert.equal(body.comparison_confirmed, true)
        assert.equal(body.old_subscription_route_ready, true)
        if (conflict) { conflict = false; return route.fulfill({ status: 409, json: { error: 'TEST_ONLY 授权已改变，请重新比较预览' } }) }
        legacy = false
        value = { user_id: 1, adopted: true, adopted_at: 1790000000, already_applied: false }
      } else if (path === '/api/control-center/reauth') {
        assert.equal(method, 'POST'); assert.equal(request.postDataJSON().password, 'TEST_ONLY_PASSWORD')
        authenticated = true; value = {}
      } else if (path === '/api/security/passkeys') value = { configuration: { enabled: false }, keys: [] }
      else if (path === '/api/me') value = { authenticated: true }
      else if (path === '/api/dashboard/access') value = { authenticated: true, public_dashboard: false }
      else if (path === '/api/access/users') value = [{ id: 1, name: 'TEST_ONLY 接管用户', subscription_url: '', subscription_token: '', credentials_redacted: true }]
      else if (path.endsWith('/entitlement')) value = entitlement
      else if (path === '/api/access/usage') value = { total: '0', uplink: '0', downlink: '0', by_user: [], by_node: [] }
      else if (path === '/api/access/users/1/policy-groups') value = { group_ids: [] }
      else if (path === '/api/access/policy-groups' || path === '/api/access/package-groups') value = []
      else if (path.endsWith('/limits')) value = { user_id: 1, revision: 0, upload_mbps: 0, download_mbps: 0, max_connections: 0, servers: [] }
      else if (path.endsWith('/portal')) value = { configuration: { enabled: false, reason: 'TEST_ONLY', origin }, keys: 0, url: null, activation_expires_at: null }
      else if (path.endsWith('/control')) value = { paused: false, revision: 0 }
      else if (path.endsWith('/sessions')) value = { servers: [] }
      else if (path.endsWith('/diagnosis')) value = {
        account: { name: 'TEST_ONLY', portal_created: false, keys: 0, active_sessions: 0, activation_expires_at: null },
        subscription: { status: legacy ? 'legacy' : 'ready', message: 'TEST_ONLY', granted_nodes: 1, ready_managed_nodes: 1, ready_external_nodes: 0 },
        legacy: legacy ? { package_name: 'TEST_ONLY', allowed: true, status: 'active', legacy_group_id: 1, lines: [] } : null,
        permissions: [], ledger: [], rotations: [], events: [], limitations: {},
      }
      else if (path.endsWith('/client-template')) value = { template: null, definition_redacted: false, limitations: 'TEST_ONLY', credential_access_reason: 'TEST_ONLY' }
      else { unexpected.push(`${method} ${path}`); return route.fulfill({ status: 404, json: { error: 'TEST_ONLY 未提供该请求' } }) }
      return route.fulfill({ json: value })
    })
    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/users`)
    await page.getByRole('button', { name: '诊断与安全', exact: true }).click()
    const open = page.getByRole('button', { name: '比较并接管订阅', exact: true })
    await open.waitFor()
    assert.equal(previewCalls, 0, 'mounting user diagnostics must not read subscription secrets')
    await open.click()
    const proof = page.getByRole('dialog', { name: '再次验证身份', exact: true })
    await proof.getByLabel('当前管理员密码', { exact: true }).fill('TEST_ONLY_PASSWORD')
    await proof.getByRole('button', { name: '验证并继续', exact: true }).click()
    await proof.waitFor({ state: 'hidden' })
    const dialog = page.getByRole('dialog', { name: '比较并接管原生订阅', exact: true })
    await dialog.getByText('TEST_ONLY 一条线路尚未就绪', { exact: true }).waitFor()
    assert.equal(await dialog.getByRole('button', { name: '确认接管此用户', exact: true }).isDisabled(), true)
    assert.equal(await dialog.getByLabel('原订阅完整地址', { exact: true }).count(), 0)
    assert.equal(adopted.length, 0)
    blockers = []
    await dialog.getByRole('button', { name: '重新预览', exact: true }).click()
    await dialog.getByRole('button', { name: '查看 sing-box 候选', exact: true }).waitFor()
    await dialog.getByText(/套餐「TEST_ONLY」（使用中） · 本期已用 10 B \/ 1000 B/).waitFor()
    const compareAll = async () => {
      for (const name of ['sing-box', 'Clash Meta', '分享链接']) {
        const comparison = dialog.getByRole('checkbox', { name: `已在客户端比较 ${name} 的节点、入口与凭据`, exact: true })
        assert.equal(await comparison.isChecked(), false)
        assert.equal(await comparison.isDisabled(), true)
        await dialog.getByRole('button', { name: `查看 ${name} 候选`, exact: true }).click()
        assert.equal(await comparison.isChecked(), false, 'opening a candidate must never confirm comparison')
        await comparison.check()
      }
      const oldRoute = dialog.getByRole('checkbox', { name: '已验证原订阅入口代理到 apsp，旧地址继续可用', exact: true })
      assert.equal(await oldRoute.isDisabled(), true)
      await dialog.getByRole('button', { name: '显示原订阅地址', exact: true }).click()
      await oldRoute.check()
      await dialog.getByRole('checkbox', { name: '已核对选中格式及影响，确认由 apsp 接管此用户', exact: true }).check()
    }
    await compareAll()
    assert.equal(await dialog.getByRole('button', { name: '确认接管此用户', exact: true }).isEnabled(), true)
    assert.equal(await dialog.getByLabel('原订阅完整地址', { exact: true }).count(), 1)
    assert.equal(await dialog.getByLabel('分享链接 候选内容', { exact: true }).count(), 1)
    const stored = await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage }))
    assert(!stored.includes('TEST_ONLY_SUBSCRIPTION_SECRET') && !stored.includes('TEST_ONLY_CREDENTIAL'))
    await page.keyboard.press('Escape')
    await dialog.waitFor({ state: 'hidden' })
    assert.equal(await page.getByText('TEST_ONLY_SUBSCRIPTION_SECRET', { exact: false }).count(), 0)
    assert.equal(await page.getByText('TEST_ONLY_CREDENTIAL', { exact: false }).count(), 0)
    const callsBeforeReopen = previewCalls
    await open.click()
    await dialog.getByRole('button', { name: '查看 sing-box 候选', exact: true }).waitFor()
    assert.equal(previewCalls, callsBeforeReopen + 1, 'reopening must request a fresh preview')
    assert.equal(await dialog.getByLabel('原订阅完整地址', { exact: true }).count(), 0)
    assert.equal(await dialog.locator('textarea').count(), 0)
    assert.equal(await dialog.getByRole('checkbox', { name: '已在客户端比较 sing-box 的节点、入口与凭据', exact: true }).isChecked(), false)
    assert.equal(await dialog.getByRole('checkbox', { name: '已验证原订阅入口代理到 apsp，旧地址继续可用', exact: true }).isChecked(), false)
    assert.equal(await dialog.getByRole('checkbox', { name: '已核对选中格式及影响，确认由 apsp 接管此用户', exact: true }).isChecked(), false)
    assert.equal(await dialog.getByRole('button', { name: '确认接管此用户', exact: true }).isDisabled(), true)
    await compareAll()
    await dialog.getByRole('button', { name: '重新预览', exact: true }).click()
    await dialog.getByRole('button', { name: '查看 sing-box 候选', exact: true }).waitFor()
    assert.equal(await dialog.getByRole('checkbox', { name: '已核对选中格式及影响，确认由 apsp 接管此用户', exact: true }).isChecked(), false)
    await compareAll()
    await dialog.getByRole('button', { name: '确认接管此用户', exact: true }).click()
    await dialog.getByText('TEST_ONLY 授权已改变，请重新比较预览', { exact: true }).waitFor()
    assert.equal(await dialog.getByRole('button', { name: '确认接管此用户', exact: true }).isDisabled(), true)
    assert.equal(await dialog.getByLabel('原订阅完整地址', { exact: true }).count(), 0)
    await dialog.getByRole('button', { name: '重新预览', exact: true }).click()
    await dialog.getByRole('button', { name: '查看 sing-box 候选', exact: true }).waitFor()
    await compareAll()
    await dialog.getByRole('button', { name: '确认接管此用户', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(adopted[1].compared_formats, ['singbox', 'clash', 'links'])
    await page.getByText('已由 apsp 接管此用户的订阅与用量周期；请继续核对客户端和设备应用状态。', { exact: true }).waitFor()
    await open.waitFor({ state: 'hidden' })

    // A paused/expired/exhausted migration user must explicitly compare the blocked state.
    legacy = true; blocked = true
    await page.reload()
    await page.getByRole('button', { name: '诊断与安全', exact: true }).click()
    await open.click()
    const limited = dialog.getByRole('checkbox', { name: '已确认原订阅与 apsp 同样不可用，暂停、到期或额度限制继续生效', exact: true })
    await limited.waitFor()
    assert.equal(await limited.isChecked(), false)
    await limited.check()
    await dialog.getByRole('button', { name: '显示原订阅地址', exact: true }).click()
    await dialog.getByRole('checkbox', { name: '已验证原订阅入口代理到 apsp，旧地址继续可用', exact: true }).check()
    await dialog.getByRole('checkbox', { name: '已核对选中格式及影响，确认由 apsp 接管此用户', exact: true }).check()
    await dialog.getByRole('button', { name: '确认接管此用户', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(adopted.at(-1).compared_formats, ['blocked'])
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false)
    assert.deepEqual(errors, [])
    assert.deepEqual(unexpected, [])
    await page.close()
    console.log(`native adoption: ${width}px explicit reveal, reauthentication, comparisons, conflict reset and blocked adoption passed`)
  }
} finally { await browser?.close(); await new Promise(done => server.close(done)) }
