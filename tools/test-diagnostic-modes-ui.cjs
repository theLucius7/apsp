// Validate the built UI through real Chromium, using only loopback API fixtures.
const assert = require('node:assert/strict')
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright')

async function main() {
  const origin = new URL(process.argv[2] || 'http://127.0.0.1:4176')
  assert(['127.0.0.1', 'localhost', '[::1]'].includes(origin.hostname))
  const browser = await chromium.launch({ headless: true, executablePath: process.env.CHROMIUM_PATH || undefined })
  try {
    const page = await browser.newPage({ viewport: { width: 1440, height: 1100 } })
    const errors = [], requests = [], refreshes = []
    let activity = 'unknown', ready = true, reports = []
    page.on('pageerror', error => errors.push(error.message))
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname
      let data = []
      if (path === '/api/me') data = {}
      else if (path === '/api/servers/1') data = {
        id: 1, name: '检查入口验收夹具', device_public_key: 'TEST_ONLY', static_info: {},
        latest_metrics: {}, manifest_rev: 0, capabilities: [], online: true, last_seen: 1700000000,
      }
      else if (path === '/api/traffic/servers/1') data = { id: 1, name: '检查入口验收夹具', enabled: false, source: null, read_only: false, online: true, agent_supported: false }
      else if (path.endsWith('/deployments')) data = { status: null, history: [] }
      else if (path.endsWith('/agent-settings')) data = { sample_interval_secs: 1, upload_interval_secs: 3, auto_update: false, discover_public_ips: false }
      else if (path.endsWith('/node-quality/reports') && request.method() === 'GET') data = {
        plugin_ready: ready, cancel_supported: true, plugin_reason: ready ? null : '请升级支持日常检查入口的 Linux Agent', reports,
        proxy_activity: { state: activity, reason: activity === 'active' ? '最近一分钟记录到代理流量，完整验机会影响连接。' : '代理流量状态未知，不能确认当前无活跃连接。', checked_at: 1700000000, last_positive_at: null },
      }
      else if (path.endsWith('/node-quality/reports') && request.method() === 'POST') {
        const body = request.postDataJSON(); requests.push(body)
        data = { id: 'TEST_ONLY', status: 'queued', job: { options: body }, report: null, error: null, created_at: 1700000000, updated_at: 1700000000, expires_at: 1700000090 }
      }
      else if (path.endsWith('/ip-quality/refresh')) {
        refreshes.push(path)
        await route.fulfill({ status: 403, json: { error: '查询源返回403，历史结果继续保留' } }); return
      }
      await route.fulfill({ json: data })
    })
    await page.goto(`${origin}/#/servers/1/node-quality`)
    const full = page.getByRole('button', { name: '完整验机', exact: true })
    await full.waitFor()
    assert(await full.isDisabled())
    await page.getByRole('checkbox', { name: /我已确认完整验机/ }).check()
    assert(await full.isDisabled())
    await page.getByRole('checkbox', { name: /我已知悉活跃代理流量/ }).check()
    assert(await full.isEnabled())
    await full.click()
    assert.equal(requests.length, 1)
    assert.equal(requests[0].mode, 'full')
    assert.equal(requests[0].confirm_full, true)
    assert.equal(requests[0].acknowledge_traffic_warning, true)
    await page.reload()
    await page.getByRole('button', { name: '日常检查', exact: true }).waitFor()
    await page.getByRole('combobox', { name: '网络测试流量' }).selectOption('normal')
    await page.getByRole('combobox', { name: '公开报告上传' }).selectOption('true')
    await page.getByRole('button', { name: '日常检查', exact: true }).click()
    await page.getByText('查询源返回403，历史结果继续保留', { exact: true }).waitFor()
    assert.equal(requests.length, 2)
    assert.equal(requests[1].mode, 'daily')
    assert.equal(requests[1].network_mode, 'low')
    assert.equal(requests[1].upload_report, false)
    assert.equal(requests[1].confirm_full, false)
    assert.equal(refreshes.length, 1)
    activity = 'active'; await page.reload()
    await page.getByText('最近一分钟记录到代理流量，完整验机会影响连接。', { exact: true }).waitFor()
    assert(await full.isDisabled())
    ready = false; await page.setViewportSize({ width: 390, height: 844 }); await page.reload()
    await page.getByText('请升级支持日常检查入口的 Linux Agent', { exact: true }).waitFor()
    assert(await page.getByRole('button', { name: '日常检查', exact: true }).isDisabled())
    assert(await full.isDisabled())
    ready = true
    reports = [{ id: 'TEST_ONLY_CANCEL', status: 'cancel_requested', agent_completed: false,
      cancel_requested_at: 1700000000, cancel_error: null, job: { plugin: 'nodequality', options: { mode: 'daily', ip_version: 'ipv4', network_mode: 'low', upload_report: 'false' } },
      report: null, error: null, created_at: 1700000000, updated_at: 1700000000, expires_at: 1700000090,
      expected_sections: ['environment', 'net_quality'], report_completeness: 'partial', sections: [
        { name: 'environment', text: 'MemoryMax: 67108864; load_one: 0.750', complete: true, revision: 1, collected_at: 1700000000 },
      ] }]
    await page.reload()
    await page.getByText('取消请求已保存，等待设备确认取消。只有设备确认进程与挂载已清理后才显示已取消；设备断连或重启后会继续处理。', { exact: true }).waitFor()
    assert(await page.getByRole('button', { name: '日常检查', exact: true }).isDisabled())
    assert(await page.getByRole('button', { name: '报告正在执行', exact: true }).isDisabled())
    await page.locator('summary').filter({ hasText: '资源限制与开始负载' }).click()
    await page.getByText('MemoryMax: 67108864; load_one: 0.750', { exact: true }).waitFor()
    assert.deepEqual(errors, [])
    console.log(JSON.stringify({ passed: ['full-confirmation', 'unknown-traffic-confirmation', 'active-traffic-warning', 'daily-forces-bounded-profile', 'ip-403-visible', 'old-agent-gate-mobile', 'cancel-request-blocks-new-modes-keeps-environment'], browser_errors: 0 }))
  } finally { await browser.close() }
}
main().catch(error => { console.error(error); process.exitCode = 1 })
