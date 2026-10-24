// Existing browser suites model the original owner session. Supply the new
// shared shell reads explicitly; keep all feature writes in each suite's router.
// Older node-page suites have no imported inventory. Nexus-specific suites pass
// nexusNodes: false and own the endpoint, including failures and malformed data.
const installed = new WeakSet()
export async function installControlCenterFixtures(page, { nexusNodes = [] } = {}) {
  if (installed.has(page)) return
  installed.add(page)
  if (nexusNodes !== false) await page.route('**/api/plugins/sing-box/nexus-nodes', async route => {
    const request = route.request(), url = new URL(request.url())
    if (request.method() !== 'GET' || !['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)) return route.fallback()
    return route.fulfill({ json: nexusNodes })
  })
  await page.route('**/api/control-center/**', async route => {
    const request = route.request()
    if (request.method() !== 'GET') return route.fallback()
    const path = new URL(request.url()).pathname
    if (path === '/api/control-center/me') return route.fulfill({ json: {
      id: 1, role: 'owner', display_name: 'TEST_ONLY original owner',
      all_servers: true, capabilities: [], token_capabilities: null, token_servers: null,
    } })
    if (path.startsWith('/api/control-center/preferences/')) return route.fulfill({ json: {
      value: path.endsWith('/recent') || path.endsWith('/favorite') ? [] : null,
      revision: 0, updated_at: 0,
    } })
    return route.fallback()
  })
}
