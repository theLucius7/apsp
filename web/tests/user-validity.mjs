import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { resolve, extname, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'

// TEST_ONLY fixtures: traffic reset and expiry changes go through preview and apply against
// private API fixtures; nothing reaches a device or the legacy panel.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
const server = createServer(async (request, response) => {
  const path = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) return response.writeHead(400).end()
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': ({ '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' })[extname(file)] ?? 'application/octet-stream' }).end(body) } catch { response.writeHead(404).end() }
})
// Wall-clock moments in Asia/Shanghai (UTC+8, no daylight saving).
const shanghai = (year, month, day, hour, minute) => Date.UTC(year, month - 1, day, hour - 8, minute) / 1000
const now = shanghai(2026, 11, 8, 12, 0)
const expiry = shanghai(2027, 1, 31, 23, 59)

let browser
try {
  await new Promise(done => server.listen(0, '127.0.0.1', done))
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  const origin = `http://127.0.0.1:${server.address().port}`
  for (const width of [1440, 390, 320]) {
    const page = await browser.newPage({ viewport: { width, height: 1000 }, timezoneId: 'Asia/Shanghai', locale: 'zh-CN' })
    page.setDefaultTimeout(7000)
    await page.emulateMedia({ reducedMotion: 'reduce' })
    await page.clock.setFixedTime(new Date(now * 1000))
    const errors = [], previews = [], applied = []
    page.on('pageerror', error => errors.push(error.message))
    let entitlement = { user_id: 1, status: 'active', allowed: true, package_group_id: 1, package_name: '测试套餐', monthly_bytes: '107374182400', reset_day: 1, reset_hour: 0, reset_minute: 0, timezone: 'UTC', starts_at: shanghai(2026, 10, 31, 23, 59), expires_at: expiry, cycle_start: shanghai(2026, 11, 1, 8, 0), next_reset: shanghai(2026, 12, 1, 8, 0), used_bytes: '1073741824' }
    const user = { id: 1, name: '有效期测试用户', subscription_url: '', subscription_token: '', credentials_redacted: true, credential_access_reason: '需要验证身份' }
    const stored = new Map()
    await page.route('**/api/**', async route => {
      const request = route.request(), path = new URL(request.url()).pathname, method = request.method()
      let value = []
      if (path.endsWith('/operations/preview') && method === 'POST') {
        const body = request.postDataJSON(), id = `00000000-0000-4000-8000-${String(previews.length + 1).padStart(12, '0')}`
        previews.push(body); stored.set(id, body)
        const summary = body.operation === 'reset_quota'
          ? { effect: 'TEST_ONLY 补偿本期已用流量，历史记录保留', credit_bytes: entitlement.used_bytes }
          : { effect: 'TEST_ONLY 只改到期时间', previous_expires_at: entitlement.expires_at, new_expires_at: body.until === 'never' ? null : body.until.at }
        value = { id, expires_at: now + 600, summary }
      } else if (/\/operations\/[^/]+\/apply$/.test(path) && method === 'POST') {
        const operation = stored.get(path.split('/').at(-2))
        assert(operation, 'apply must name an existing preview'); assert.deepEqual(request.postDataJSON(), { confirm: true })
        applied.push(operation)
        if (operation.operation === 'reset_quota') entitlement = { ...entitlement, used_bytes: '0' }
        else entitlement = { ...entitlement, expires_at: operation.until === 'never' ? null : operation.until.at }
        value = { status: 'saved' }
      } else if (path === '/api/dashboard/access') value = { authenticated: true, public_dashboard: false }
      else if (path === '/api/me') value = { authenticated: true }
      else if (path.endsWith('/users')) value = [user]
      else if (path.endsWith('/users/overview')) value = []
      else if (path.endsWith('/entitlement')) value = entitlement
      else if (path.endsWith('/limits')) value = { user_id: 1, revision: 0, upload_mbps: 0, download_mbps: 0, max_connections: 0, servers: [] }
      else if (path.endsWith('/usage')) value = { total: '0', uplink: '0', downlink: '0', by_node: [], by_user: [] }
      else if (path.endsWith('/users/1/policy-groups')) value = { group_ids: [] }
      else if (path.endsWith('/package-groups')) value = [{ id: 1, name: '测试套餐', monthly_bytes: '107374182400', reset_day: 1, reset_hour: 0, reset_minute: 0, timezone: 'UTC', duration_days: 30 }]
      else if (path.endsWith('/policy-groups')) value = []
      else if (path.endsWith('/diagnosis') || path.endsWith('/portal') || path.endsWith('/client-template') || path.includes('/invitations')) return route.fulfill({ status: 503, json: { error: '该测试未提供此页面数据' } })
      return route.fulfill({ json: value })
    })
    const local = seconds => page.evaluate(value => new Date(value * 1000).toLocaleString('zh-CN', { hour12: false }), seconds)
    const review = async (title, text) => {
      const modal = page.getByRole('dialog', { name: title })
      await modal.waitFor()
      if (text) await modal.getByText(text, { exact: false }).waitFor()
      return modal
    }
    const apply = async modal => {
      await modal.getByRole('checkbox', { name: '已核对目标与影响，确认执行' }).check()
      await modal.getByRole('button', { name: '确认应用', exact: true }).click()
      await modal.waitFor({ state: 'hidden' })
    }
    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/users`)
    const section = page.getByRole('region', { name: '权限与套餐' })
    await section.getByText(await local(expiry), { exact: true }).waitFor().catch(async error => { console.error(errors, (await page.locator('body').innerText()).slice(-3000)); throw error })

    // Reset: preview names the reset, apply clears this cycle's usage and the button rests.
    await section.getByRole('button', { name: '重置流量', exact: true }).click()
    await apply(await review('本期流量重置预览', 'TEST_ONLY 补偿本期已用流量'))
    assert.deepEqual(previews.at(-1), { operation: 'reset_quota', user_id: 1 })
    await page.getByText(/变更已确认保存/).waitFor()
    await section.getByText(/本期已用 0 B/).waitFor()
    assert.equal(await section.getByRole('button', { name: '重置流量', exact: true }).isDisabled(), true)

    // Extend by calendar months from the current expiry; a month-end expiry stays at month end.
    await section.getByRole('button', { name: '调整', exact: true }).click()
    const dialog = page.getByRole('dialog', { name: '调整有效期' })
    await dialog.getByText(`当前：${await local(expiry)}`, { exact: true }).waitFor()
    await dialog.getByText(await local(shanghai(2027, 2, 28, 23, 59)), { exact: true }).waitFor()
    await dialog.getByRole('button', { name: '3 个月', exact: true }).click()
    await dialog.getByText(await local(shanghai(2027, 4, 30, 23, 59)), { exact: true }).waitFor()
    await dialog.getByRole('button', { name: '预览', exact: true }).click()
    let modal = await review('有效期调整预览', `${await local(expiry)} → ${await local(shanghai(2027, 4, 30, 23, 59))}`)
    assert.deepEqual(previews.at(-1), { operation: 'set_validity', user_id: 1, until: { at: shanghai(2027, 4, 30, 23, 59) } })
    await modal.getByRole('button', { name: '返回草稿', exact: true }).click()
    await modal.waitFor({ state: 'hidden' })

    // An exact moment on the administrator's clock: earlier is allowed, the past and no change are not.
    await dialog.getByRole('button', { name: '指定到期时间', exact: true }).click()
    const field = dialog.getByLabel('到期时间')
    assert.equal(await field.inputValue(), '2027-01-31T23:59')
    await dialog.getByText('到期时间没有变化', { exact: true }).waitFor()
    assert.equal(await dialog.getByRole('button', { name: '预览', exact: true }).isDisabled(), true)
    await field.fill('2026-11-01T00:00')
    await dialog.getByText('到期时间必须晚于当前时间', { exact: true }).waitFor()
    assert.equal(await dialog.getByRole('button', { name: '预览', exact: true }).isDisabled(), true)
    await field.fill('2026-12-01T08:00')
    await dialog.getByRole('button', { name: '预览', exact: true }).click()
    modal = await review('有效期调整预览')
    assert.deepEqual(previews.at(-1), { operation: 'set_validity', user_id: 1, until: { at: shanghai(2026, 12, 1, 8, 0) } })
    await modal.getByRole('button', { name: '返回草稿', exact: true }).click()
    await modal.waitFor({ state: 'hidden' })

    // No time limit: applying closes both dialogs and the section says so.
    await dialog.getByRole('button', { name: '不限期', exact: true }).click()
    await dialog.getByRole('button', { name: '预览', exact: true }).click()
    await apply(await review('有效期调整预览', `${await local(expiry)} → 不限期`))
    await dialog.waitFor({ state: 'hidden' })
    assert.deepEqual(applied.at(-1), { operation: 'set_validity', user_id: 1, until: 'never' })
    await section.getByText('不限期', { exact: true }).waitFor()

    // Without an expiry there is nothing to extend: the dialog starts at an exact moment.
    await section.getByRole('button', { name: '调整', exact: true }).click()
    await dialog.getByText('当前：不限期', { exact: true }).waitFor()
    assert.equal(await dialog.getByRole('button', { name: '延长', exact: true }).count(), 0)
    assert.equal(await dialog.getByLabel('到期时间').inputValue(), '2026-12-08T12:00')
    await dialog.getByRole('button', { name: '取消', exact: true }).click()
    await dialog.waitFor({ state: 'hidden' })

    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth > innerWidth + 1), false)
    assert.deepEqual(errors, [])
    assert.equal(applied.length, 2)
    await page.close()
    console.log(`user validity: ${width}px reset, month-end extension, exact moment, no limit passed`)
  }
} finally { await browser?.close(); await new Promise(done => server.close(done)) }
