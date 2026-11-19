import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'

// Exercise the shipped bundle against loopback fixtures for Agent releases (`/settings/agent-updates`),
// per-server built-in component versions (`static_info.components`), and monitor-only service states.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const root = fileURLToPath(new URL('../dist/', import.meta.url))
const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }
const http = createServer(async (request, response) => {
  const pathname = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(root, pathname === '/' ? 'index.html' : `.${pathname}`)
  if (!file.startsWith(root.endsWith(sep) ? root : `${root}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': mime[extname(file)] ?? 'application/octet-stream' }); response.end(body) }
  catch { response.writeHead(404).end() }
})
await new Promise(resolve => http.listen(0, '127.0.0.1', resolve))
const origin = `http://127.0.0.1:${http.address().port}`
const browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
const releasePolicy = {
  default_version: '0.4.0',
  selection: 'highest_stable_signed_protocol_compatible_for_target',
  minimum_version: '0.4.0',
}
const signedReleases = [
  { version: '0.4.0', tag: 'agent-v0.4.0', targets: ['linux-gnu-amd64', 'linux-musl-arm64', 'windows-amd64'], protocol_min: 1, protocol_max: 1 },
  { version: '0.3.2', tag: 'agent-v0.3.2', targets: ['macos-arm64'], protocol_min: 1, protocol_max: 2 },
]

try {
  for (const width of [1440, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } })
    const errors = [], mutations = []
    let versions = signedReleases, releaseFailure = false, metadataFailure = false
    const metadata = {
      id: 2,
      name: '一体化验收服务器',
      configured: false,
      online: true,
      agent_supported: true,
      read_only: false,
      source: null,
      runtime_version: '1.14.4-apsp.2',
      status: { state: 'unconfigured', reason: '', target_rev: 0, applied_rev: 0, current_healthy: false },
    }
    const servers = [
      { id: 1, name: '旧版待升级服务器', online: false, device_public_key: 'LEGACY_KEY', capabilities: [], static_info: { agent_version: '0.2.9' }, latest_metrics: {}, manifest_rev: 0 },
      {
        id: 2,
        name: metadata.name,
        online: true,
        device_public_key: 'TEST_ONLY',
        capabilities: ['agent:self-contained:v1'],
        static_info: {
          agent_version: '0.4.0',
          runtime_version: '1.14.4-apsp.2',
          components: {
            'sing-box': '1.14.4-apsp.2',
            tcpquality: '0.1.0',
            ipquality: '87397e2c3196-node-r1',
            nodequality: 'a92fca6c0067-native-r2',
          },
        },
        latest_metrics: {},
        manifest_rev: 0,
      },
    ]
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname
      let value
      if (request.method() !== 'GET') {
        mutations.push({ path, method: request.method(), body: request.postData() ? request.postDataJSON() : null })
        if (path === '/api/traffic/servers/2/deployments/check' && request.method() === 'POST') {
          value = { ready: true, checks: [{ name: '内置运行时能力', passed: true, detail: '一体化 Agent 已内置 sing-box 1.14.4-apsp.2' }] }
        } else {
          errors.push(`Unexpected mutation: ${path}`); await route.fulfill({ status: 400, json: { error: 'UNEXPECTED_MUTATION' } }); return
        }
      } else if (path === '/api/dashboard/access') value = { authenticated: true, public_dashboard: false }
      else if (path === '/api/me') value = { authenticated: true }
      else if (path === '/api/system/agent-releases') {
        if (releaseFailure) { await route.fulfill({ status: 503, json: { error: 'AGENT_RELEASES_READ_FAILED' } }); return }
        value = { policy: releasePolicy, versions }
      } else if (path === '/api/servers/1') value = servers[0]
      else if (path === '/api/servers/2') value = servers[1]
      else if (/^\/api\/servers\/[12]\/(metrics|probes|probe-results|commands)$/.test(path)) value = []
      else if (path === '/api/servers/1/agent-settings') value = { sample_interval_secs: 1, upload_interval_secs: 3, auto_update: false, discover_public_ips: false }
      else if (path === '/api/servers/1/telemetry-settings') value = { persist_interval_secs: 60 }
      else if (path === '/api/traffic/servers/2') {
        if (metadataFailure) { await route.fulfill({ status: 403, json: { error: 'RUNTIME_STATE_UNKNOWN' } }); return }
        value = metadata
      }
      else if (path === '/api/traffic/servers/2/profile') value = { definition: { outbounds: [], endpoints: [], services: [], cloudflared: [], dns: null, route: null }, revision: 0 }
      else { errors.push(`Unexpected read: ${path}`); await route.fulfill({ status: 404, json: {} }); return }
      await route.fulfill({ json: value })
    })

    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/settings/agent-updates`)
    await page.getByRole('heading', { name: 'Agent 版本', exact: true }).waitFor()
    assert.equal(await page.title(), 'Agent 版本 · apsp')
    const policyPanel = page.locator('[data-slot="agent-release-policy"]')
    const releasesPanel = page.locator('[data-slot="agent-releases"]')
    await policyPanel.getByRole('heading', { name: '发布与升级策略', exact: true }).waitFor()
    await policyPanel.getByText('按目标平台选择最高已签名且协议兼容的稳定版本', { exact: false }).waitFor()
    await policyPanel.getByText('0.4.0', { exact: true }).first().waitFor()
    await releasesPanel.getByRole('heading', { name: '已收录签名版本', exact: true }).waitFor()
    await releasesPanel.getByText('共 2 个版本', { exact: true }).waitFor()
    assert.equal(await releasesPanel.locator('[data-slot="agent-release"]').count(), 2)
    const v040 = releasesPanel.locator('[data-agent-version="0.4.0"]')
    await v040.getByText('agent-v0.4.0', { exact: true }).waitFor()
    await v040.getByText('Linux AMD64（GNU / glibc）', { exact: true }).waitFor()
    await v040.getByText('Windows AMD64', { exact: true }).waitFor()
    await v040.getByText('v1', { exact: true }).waitFor()
    const v032 = releasesPanel.locator('[data-agent-version="0.3.2"]')
    await v032.getByText('macOS ARM64', { exact: true }).waitFor()
    await v032.getByText('v1–v2', { exact: true }).waitFor()
    assert.equal(await page.getByRole('link', { name: '前往服务器列表', exact: true }).getAttribute('href'), '#/servers')
    assert.equal(await page.getByRole('button', { name: /导入|安装|启用|选择服务器/ }).count(), 0)
    assert.equal(await releasesPanel.locator('form, input').count(), 0)
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
    if (process.env.APSP_UI_SCREENSHOT_DIR) await page.screenshot({ path: resolve(process.env.APSP_UI_SCREENSHOT_DIR, `agent-updates-${width}.png`), fullPage: true })

    // A failed refresh surfaces the error while retaining previously read signed releases.
    releaseFailure = true
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await page.getByText('AGENT_RELEASES_READ_FAILED', { exact: true }).waitFor()
    assert.equal(await releasesPanel.locator('[data-slot="agent-release"]').count(), 2)
    releaseFailure = false
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await page.getByText('AGENT_RELEASES_READ_FAILED', { exact: true }).waitFor({ state: 'hidden' })

    // Empty signed release catalogue shows clear guidance and links to servers.
    versions = []
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await releasesPanel.getByText('暂无已收录的签名版本', { exact: true }).waitFor()
    assert.equal(await releasesPanel.getByRole('link', { name: '查看服务器', exact: true }).getAttribute('href'), '#/servers')
    versions = signedReleases
    assert.deepEqual(mutations, [])

    // Retired artifact and plugin addresses render not-found without requesting removed APIs.
    for (const retired of ['/settings/artifacts', '/artifacts', '/settings/plugins', '/servers/2/plugins']) {
      await page.goto(`${origin}/#${retired}`)
      await page.getByRole('heading', { name: '这个页面不存在', exact: true }).waitFor()
    }

    // Built-in component versions reported via `static_info.components` appear on the server detail page.
    await page.goto(`${origin}/#/servers/2`)
    await page.getByRole('heading', { name: metadata.name, exact: true }).waitFor()
    const components = page.locator('[data-slot="agent-components"]')
    await components.getByText('一体化 Agent', { exact: true }).waitFor()
    await components.getByText('Agent v0.4.0', { exact: true }).waitFor()
    await components.getByText('sing-box 运行时', { exact: true }).waitFor()
    await components.getByText('1.14.4-apsp.2', { exact: true }).waitFor()
    await components.getByText('TCP 连接诊断', { exact: true }).waitFor()
    await components.getByText('IP 质量检测', { exact: true }).waitFor()
    await components.getByText('节点验机', { exact: true }).waitFor()

    await page.goto(`${origin}/#/servers/1/agent`)
    const emptyComponents = page.locator('[data-slot="agent-components"]')
    await emptyComponents.getByText('当前设备尚未上报内置组件版本；升级到一体化 Agent 并连接后将在此显示内置运行时与诊断能力版本。', { exact: true }).waitFor()
    assert.equal(await emptyComponents.getByText('一体化 Agent', { exact: true }).count(), 0)

    // Network services tab on a monitor-only server has no install button and guards readiness checks on fresh state.
    await page.goto(`${origin}/#/servers/2/services`)
    const card = page.locator('section').filter({ has: page.getByRole('heading', { name: /^sing-box/ }) })
    await card.getByText('未配置服务', { exact: true }).waitFor()
    assert.equal(await card.getByRole('button', { name: '安装 sing-box', exact: true }).count(), 0)
    assert.equal(await card.getByRole('link', { name: '添加节点', exact: true }).getAttribute('href'), '#/nodes?server=2')
    metadataFailure = true
    await card.getByRole('button', { name: '刷新', exact: true }).click()
    await page.getByText('RUNTIME_STATE_UNKNOWN', { exact: true }).waitFor()
    assert.equal(await card.getByRole('button', { name: '检查运行条件', exact: true }).isDisabled(), true)
    metadataFailure = false
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await page.getByText('RUNTIME_STATE_UNKNOWN', { exact: true }).waitFor({ state: 'hidden' })
    await card.getByRole('button', { name: '检查运行条件', exact: true }).click()
    await card.getByText('一体化 Agent 已内置 sing-box 1.14.4-apsp.2', { exact: true }).waitFor()
    assert.deepEqual(mutations, [{ path: '/api/traffic/servers/2/deployments/check', method: 'POST', body: null }])
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
    assert.deepEqual(errors, [])
    await page.close()
    console.log(`PASS: ${width}px Agent release policy/versions, retired artifact/plugin addresses, static_info.components, and monitor-only runtime readiness`)

    // A public dashboard must not expose Agent release policies or server runtime controls.
    const anonymous = await browser.newPage({ viewport: { width, height: 1000 } })
    const privateReads = []
    await anonymous.route('**/api/**', async route => {
      const path = new URL(route.request().url()).pathname
      if (path === '/api/dashboard/access') await route.fulfill({ json: { authenticated: false, public_dashboard: true } })
      else { privateReads.push(path); await route.fulfill({ status: 403, json: {} }) }
    })
    for (const path of ['/settings/agent-updates', '/settings/artifacts', '/servers/2/services']) {
      await anonymous.goto(`${origin}/#${path}`)
      await anonymous.getByRole('heading', { name: '欢迎回来', exact: true }).waitFor()
      assert.equal(await anonymous.locator('[data-slot="agent-releases"], [data-slot="agent-components"]').count(), 0)
    }
    assert.deepEqual(privateReads, [])
    await anonymous.close()
    console.log(`PASS: ${width}px public dashboard does not expose Agent release settings or server runtime controls`)
  }
} finally { await browser.close(); await new Promise(resolve => http.close(resolve)) }
