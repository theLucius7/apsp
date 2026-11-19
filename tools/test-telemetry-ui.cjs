// Run against a local built preview; all API data is an isolated loopback fixture.
const assert = require('node:assert/strict')
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright')

async function main() {
  const origin = new URL(process.argv[2] || 'http://127.0.0.1:4175')
  assert(['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname))
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH || undefined })
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } })
    const errors = []
    page.on('pageerror', error => errors.push(error.message))
    let mode = 'stale'
    const sampled = 1700000000123
    const heartbeat = 1700000600
    await page.route('**/api/**', async route => {
      const path = new URL(route.request().url()).pathname
      let data = []
      if (path === '/api/dashboard/access') data = { authenticated: true, public_dashboard: false }
      else if (path === '/api/servers/1') data = {
        id: 1, name: '遥测隔离验收夹具', device_public_key: 'TEST_ONLY_device',
        static_info: { hostname: 'fixture-host', memory_total: 1024 ** 3 },
        latest_metrics: { cpu_percent: 7.5, memory_used: 64 * 1024 ** 2, uptime_secs: 42 },
        manifest_rev: 0, capabilities: [], online: mode !== 'offline',
        last_seen: heartbeat + 2, last_heartbeat_at: mode === 'unknown' ? null : heartbeat,
        metrics_sampled_at: mode === 'unknown' ? null : sampled,
        metrics_stale: mode === 'stale' || mode === 'offline',
      }
      else if (path === '/api/traffic/servers/1') data = {
        id: 1, name: '遥测隔离验收夹具', configured: false, online: mode !== 'offline',
        agent_supported: true, read_only: false, source: null, runtime_version: null,
        status: { state: 'unconfigured', reason: '', target_rev: 0, applied_rev: 0, current_healthy: false },
      }
      else if (path.endsWith('/deployments')) data = { status: null, history: [] }
      else if (path.endsWith('/node-quality')) data = { ip_addresses: [], quality: [], tool_ready: false, tool_reason: null, full_ready: false, full_reason: null, reports: [] }
      else if (path.endsWith('/agent-settings')) data = { sample_interval_secs: 1, upload_interval_secs: 3, auto_update: false, discover_public_ips: false }
      await route.fulfill({ json: data })
    })
    const { pathToFileURL } = require('node:url')
    const { resolve } = require('node:path')
    const { installControlCenterFixtures } = await import(pathToFileURL(resolve(__dirname, '../web/tests/control-center-fixtures.mjs')).href)
    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/servers/1`)
    const row = label => page.locator('[data-slot="info-group"] dl > div').filter({ has: page.locator('dt', { hasText: label }) }).locator('dd')
    await page.getByText('指标过期', { exact: true }).waitFor()
    assert.equal(await page.getByText('在线', { exact: true }).count(), 1)
    assert.equal(await page.getByText('7.5%', { exact: true }).count(), 1)
    const expected = await page.evaluate(({ sampled, heartbeat }) => ({
      sample: new Date(sampled).toLocaleString('zh-CN', { hour12: false }),
      heartbeat: new Date(heartbeat * 1000).toLocaleString('zh-CN', { hour12: false }),
    }), { sampled, heartbeat })
    assert.equal(await row('最后采样').innerText(), expected.sample)
    assert.equal(await row('最后心跳').innerText(), expected.heartbeat)
    assert.notEqual(await row('最近设备消息').innerText(), expected.heartbeat)
    assert((await page.getByText('指标已过期；在线心跳不代表指标仍在采集。', { exact: false }).innerText()).includes('心跳不代表指标仍在采集'))
    mode = 'fresh'
    await page.reload()
    await row('最后采样').waitFor()
    assert.equal(await page.getByText('指标过期', { exact: true }).count(), 0)
    assert.equal(await page.getByText('7.5%', { exact: true }).count(), 1)
    mode = 'offline'
    await page.reload()
    await page.getByText('离线', { exact: true }).waitFor()
    await page.getByText('指标过期', { exact: true }).waitFor()
    assert.equal(await page.getByText('7.5%', { exact: true }).count(), 1)
    mode = 'unknown'
    await page.reload()
    await page.getByText('尚未上报', { exact: true }).waitFor()
    assert.equal(await row('最后采样').innerText(), '时间未知')
    assert.equal(await page.getByText('指标过期', { exact: true }).count(), 0)
    assert.deepEqual(errors, [])
    console.log(JSON.stringify({ passed: ['online-stale-history', 'milliseconds-conversion', 'separate-heartbeat-message', 'fresh', 'offline-history', 'unknown-legacy-times'], browser_errors: 0 }))
  } finally { await browser.close() }
}
main().catch(error => { console.error(error); process.exitCode = 1 })
