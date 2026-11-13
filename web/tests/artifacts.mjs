import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'

// Exercise the shipped bundle against loopback fixtures, never production Agents.
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
const packageFor = (name, version, arch) => ({ name, version, arch, bytes: 1024, sha256: 'a'.repeat(64) })
const packages = [
  packageFor('sing-box', '1.14.2', 'arm64'), packageFor('sing-box', '1.14.2', 'amd64'), packageFor('sing-box', '1.13.0', 'amd64'),
  packageFor('nodequality', `${'a'.repeat(40)}-r12`, 'amd64'), packageFor('nodequality', `${'a'.repeat(40)}-r12`, 'arm64'),
  packageFor('tcpquality', '0.3.0-TEST_ONLY-r1', 'amd64'), packageFor('tcpquality', '0.3.0-TEST_ONLY-r1', 'arm64'),
  packageFor('agent', '0.3.0', 'amd64'), packageFor('agent', '0.3.0', 'arm64'),
  packageFor('custom-component', '1', 'amd64'), packageFor('custom-component', '1', 'arm64'),
]

try {
  for (const width of [1440, 390]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 } })
    const errors = [], mutations = []
    let inventory = packages, artifactFailure = false, metadataFailure = false
    const metadata = { id: 2, name: '所选服务器', enabled: false, online: true, agent_supported: true, read_only: false, source: null, installation: { state: 'not_enabled', reason: '尚未安装 sing-box。', target_rev: 0, applied_rev: 0 } }
    const servers = [
      { id: 1, name: '另一台服务器', online: false, device_public_key: null, static_info: {} },
      { id: 2, name: metadata.name, online: true, device_public_key: 'TEST_ONLY', static_info: {} },
    ]
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname
      let value
      if (request.method() !== 'GET') {
        mutations.push({ path, method: request.method(), body: request.postDataJSON() })
        if (path !== '/api/traffic/servers/2/enable' || request.method() !== 'POST') {
          errors.push(`Unexpected mutation: ${path}`); await route.fulfill({ status: 400, json: { error: 'UNEXPECTED_MUTATION' } }); return
        }
        Object.assign(metadata, { enabled: true, source: 'administrator', installation: { state: 'queued', reason: '安装请求已保存，正在生成初始运行配置', target_rev: 0, applied_rev: 0 } }); value = metadata
      } else if (path === '/api/dashboard/access') value = { authenticated: true, public_dashboard: false }
      else if (path === '/api/me') value = { authenticated: true }
      else if (path === '/api/artifacts') {
        if (artifactFailure) { await route.fulfill({ status: 403, json: { error: 'ARTIFACT_READ_FAILED' } }); return }
        value = inventory
      } else if (path === '/api/servers/2') value = servers[1]
      else if (path === '/api/traffic/servers/2') {
        if (metadataFailure) { await route.fulfill({ status: 403, json: { error: 'RUNTIME_STATE_UNKNOWN' } }); return }
        value = metadata
      }
      // Once installed, the network services tab also reads this server's deployment and tools.
      else if (path === '/api/traffic/servers/2/deployments') value = { status: null, history: [] }
      else if (path === '/api/traffic/servers/2/profile') value = { definition: { outbounds: [], endpoints: [], services: [], cloudflared: [], dns: null, route: null }, revision: 0 }
      else if (path === '/api/traffic/servers/2/runtime-operations') value = { supported: false, online: false, retiring: false, operations: [] }
      else if (path === '/api/traffic/servers/2/operations-view') value = {
        runtime: { supported_versions: [], selected_version: 'TEST_ONLY', reason: 'TEST_ONLY 未读取真实运行时版本。', compatibility_metadata: { upstream_release: 'https://example.com/TEST_ONLY', upstream_commit: 'TEST_ONLY', protocols: [], acceptance: [] } },
        preflight: { id: null, ready: false, confirmed: false, device_checks_pending: false, checks: [] },
        drift: { state: 'unknown', target_revision: null, applied_revision: null, last_observed_at: null, reason: 'TEST_ONLY 无真实配置检查点。', checkpoint_supported: false, checkpoint: { state: 'unknown', observed_at: null, reason: 'TEST_ONLY 未取得检查点。' } },
        changes: [], history: [], paths: [], hop_observations: [], path_diagnosis: 'TEST_ONLY 无真实路径证据。',
      }
      else if (path === '/api/traffic/nodes') value = []
      else { errors.push(`Unexpected read: ${path}`); await route.fulfill({ status: 404, json: {} }); return }
      await route.fulfill({ json: value })
    })

    const runtime = page.locator('[data-artifact="sing-box"]')
    const openArtifacts = async (path = '/settings/artifacts') => {
      await installControlCenterFixtures(page)
      await page.goto(`${origin}/#${path}`)
      await page.getByRole('heading', { name: '制品与版本', exact: true }).waitFor()
      await runtime.locator('summary').waitFor()
    }
    // The plugin centre's address opens the same page for one release.
    for (const path of ['/settings/artifacts', '/settings/plugins']) {
      await openArtifacts(path)
      assert.equal(await page.title(), '制品与版本 · apsp')
    }
    assert.equal(await page.locator('[data-artifact-group]').count(), 4)
    assert.equal(await page.locator('[data-artifact]').count(), 6)
    assert.equal(await page.getByRole('button', { name: /导入|安装|启用|选择服务器/ }).count(), 0)
    assert.equal(await page.locator('[data-slot=artifact-groups]').locator('form, input').count(), 0)
    assert.equal(await page.locator('[data-slot=artifact-groups]').getByText('插件', { exact: false }).count(), 0)
    assert.equal(await runtime.locator('[data-slot=artifact-architectures] [data-slot=badge]').count(), 2)
    await runtime.locator('summary').click()
    assert.equal(await runtime.locator('[data-slot=artifact-version]').count(), 2)
    const oldVersion = runtime.getByRole('region', { name: '版本 1.13.0' })
    assert.equal(await oldVersion.locator('tbody tr').count(), 1)
    assert.equal((await oldVersion.innerText()).includes('arm64'), false)
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
    if (process.env.APSP_UI_SCREENSHOT_DIR) await page.screenshot({ path: resolve(process.env.APSP_UI_SCREENSHOT_DIR, `artifacts-${width}.png`), fullPage: true })

    // A failed refresh keeps the versions it read before and says so.
    artifactFailure = true
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await page.getByText('ARTIFACT_READ_FAILED', { exact: true }).waitFor()
    assert.equal(await runtime.getByText('以下是上次读取的版本信息，当前状态未知。', { exact: true }).count(), 1)
    artifactFailure = false
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await page.getByText('ARTIFACT_READ_FAILED', { exact: true }).waitFor({ state: 'hidden' })

    // Known components stay listed when nothing has been imported.
    inventory = []
    await page.getByRole('button', { name: '刷新', exact: true }).click()
    await page.getByText('暂无已验证的版本。', { exact: true }).first().waitFor()
    assert.equal(await page.locator('[data-artifact]').count(), 5)
    inventory = packages
    assert.deepEqual(mutations, [])

    // The runtime is installed in the server's network services tab, under the old address too.
    for (const path of ['/servers/2/plugins', '/servers/2/services']) {
      await page.goto(`${origin}/#${path}`)
      await page.getByRole('heading', { name: metadata.name, exact: true }).waitFor()
      await page.getByRole('button', { name: '安装 sing-box', exact: true }).waitFor()
      assert.equal(await page.getByRole('navigation', { name: '服务器导航' }).getByRole('link', { name: '网络服务', exact: true }).getAttribute('aria-current'), 'page')
    }
    metadataFailure = true
    await page.locator('section').filter({ has: page.getByRole('heading', { name: /^sing-box/ }) }).getByRole('button', { name: '刷新', exact: true }).click()
    await page.getByText('RUNTIME_STATE_UNKNOWN', { exact: true }).waitFor()
    assert.equal(await page.getByRole('button', { name: '安装 sing-box', exact: true }).isDisabled(), true)
    metadataFailure = false
    await page.getByRole('button', { name: '重试', exact: true }).click()
    await page.getByText('RUNTIME_STATE_UNKNOWN', { exact: true }).waitFor({ state: 'hidden' })
    await page.getByRole('button', { name: '安装 sing-box', exact: true }).click()
    await page.getByText('安装已安排', { exact: true }).waitFor()
    assert.equal(await page.getByText('已安装并运行', { exact: true }).count(), 0)
    assert.deepEqual(mutations, [{ path: '/api/traffic/servers/2/enable', method: 'POST', body: {} }])
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
    assert.deepEqual(errors, [])
    await page.close()
    console.log(`PASS: ${width}px artifact groups, old address, failure and empty states, and runtime installation in the server's network services tab`)

    // A public dashboard must not make the artifact page or server controls public.
    const anonymous = await browser.newPage({ viewport: { width, height: 1000 } })
    const privateReads = []
    await anonymous.route('**/api/**', async route => {
      const path = new URL(route.request().url()).pathname
      if (path === '/api/dashboard/access') await route.fulfill({ json: { authenticated: false, public_dashboard: true } })
      else { privateReads.push(path); await route.fulfill({ status: 403, json: {} }) }
    })
    for (const path of ['/settings/artifacts', '/settings/plugins', '/servers/2/services']) {
      await anonymous.goto(`${origin}/#${path}`)
      await anonymous.getByRole('heading', { name: '欢迎回来', exact: true }).waitFor()
      assert.equal(await anonymous.locator('[data-artifact]').count(), 0)
    }
    assert.deepEqual(privateReads, [])
    await anonymous.close()
    console.log(`PASS: ${width}px public dashboard does not expose artifacts or server runtime controls`)
  }
} finally { await browser.close(); await new Promise(resolve => http.close(resolve)) }
