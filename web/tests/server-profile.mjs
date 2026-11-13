import { installControlCenterFixtures } from './control-center-fixtures.mjs'
import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { resolve, extname, sep } from 'node:path'

// Exercise the actual dist with loopback fixtures; no Agent, release, or production access.
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/', import.meta.url))
// Read-only runtime operations view shown on the server's network services tab once sing-box is enabled.
const operationsViewFixture = {
  runtime: { supported_versions: [], selected_version: 'TEST_ONLY', reason: 'TEST_ONLY 未读取真实运行时版本。', compatibility_metadata: { upstream_release: 'https://example.com/TEST_ONLY', upstream_commit: 'TEST_ONLY', protocols: [], acceptance_scope: 'TEST_ONLY 只读夹具' } },
  preflight: { id: null, ready: false, confirmed: false, device_checks_pending: false, checks: [] },
  drift: { state: 'unknown', target_revision: null, applied_revision: null, last_observed_at: null, reason: 'TEST_ONLY 无真实配置检查点。', checkpoint_supported: false, checkpoint: { state: 'unknown', observed_at: null, reason: 'TEST_ONLY 未读取进程或文件。' } },
  changes: [], history: [], paths: [], hop_observations: [], path_diagnosis: 'TEST_ONLY 无真实路径证据。',
}
const mime = { '.html': 'text/html', '.js': 'text/javascript', '.css': 'text/css', '.svg': 'image/svg+xml' }
const server = createServer(async (request, response) => {
  const pathname = new URL(request.url, 'http://127.0.0.1').pathname
  const file = resolve(dist, pathname === '/' ? 'index.html' : `.${pathname}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) { response.writeHead(400).end(); return }
  try { const body = await readFile(file); response.writeHead(200, { 'Content-Type': mime[extname(file)] ?? 'application/octet-stream' }); response.end(body) }
  catch { response.writeHead(404).end() }
})

let browser
try {
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
  browser = await chromium.launch({ headless: true, ...(process.env.APSP_CHROME_PATH ? { executablePath: process.env.APSP_CHROME_PATH } : {}) })
  const origin = `http://127.0.0.1:${server.address().port}`
  for (const width of [1440, 390]) {
    const context = await browser.newContext({ viewport: { width, height: 1000 }, serviceWorkers: 'block' })
    context.setDefaultTimeout(10000)
    const page = await context.newPage()
    const errors = [], unexpected = [], reads = [], writes = []
    const prefix = '/api/traffic/servers'
    const name='TEST_ONLY 服务端配置'
    const selected=()=>({id:1,name,enabled:true,online:true,agent_supported:true,read_only:false,source:'administrator',installation:{state:'ready',reason:'',target_rev:1,applied_rev:1}})
    let definition={outbounds:[],endpoints:[],services:[],cloudflared:[],dns:null,route:null},revision=0,conflict=false
    page.on('pageerror', error => errors.push(error.message))
    page.on('console', message => { if (message.type() === 'error' && !message.text().includes('409 (Conflict)')) errors.push(message.text()) })
    await context.route('**/*', async route => {
      const request = route.request(), url = new URL(request.url()), method = request.method(), pathname = url.pathname
      if (url.origin !== origin) {
        unexpected.push(`External request refused: ${method} ${url.origin}${pathname}`)
        await route.abort('blockedbyclient'); return
      }
      if (!pathname.startsWith('/api/')) { await route.continue(); return }
      let value
      if (method === 'GET') {
        reads.push(pathname)
        if (pathname === '/api/dashboard/access') value = { authenticated: true, public_dashboard: false }
        else if (pathname === '/api/me') value = { authenticated: true }
        else if (pathname === prefix) value = [selected()]
        else if (pathname === `${prefix}/1`) value = selected()
        else if (pathname === `${prefix}/2`) value = selected()
        // The server's network services tab shares the server header and shows the proxy business once enabled.
        else if (pathname === '/api/servers/1') value = { id: 1, name, online: true, device_public_key: 'TEST_ONLY', capabilities: ['singbox'], static_info: {}, latest_metrics: {}, manifest_rev: 1 }
        else if (pathname === `${prefix}/1/profile`) value = {definition,revision}
        else if (pathname === `${prefix}/1/deployments`) value = { status: null, history: [] }
        else if (pathname === `${prefix}/1/runtime-operations`) value = { supported: false, online: false, retiring: false, operations: [] }
        else if (pathname === '/api/traffic/nodes') value = []
        else if (pathname === `${prefix}/1/operations-view` && !url.search) value = operationsViewFixture
      } else if (pathname === `${prefix}/1/profile` && method === 'PUT') {
        if(conflict) return route.fulfill({status:409,json:{error:'配置已变化，请刷新后重试'}})
        const body=request.postDataJSON();writes.push(body);assert.equal(body.expected_revision,revision);definition=body.definition;revision++;value={revision}
      }

      if (value === undefined) {
        unexpected.push(`${method} ${pathname}`)
        await route.fulfill({ status: 404, json: { error: '测试拒绝未知接口' } }); return
      }
      await route.fulfill({ json: value })
    })

    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/servers/1/services`)
    const editor=page.getByLabel('服务端配置 JSON')
    await editor.waitFor()
    await page.getByText('填入配置示例',{exact:true}).click()
    await page.getByRole('button',{name:'Cloudflared 隧道',exact:true}).click()
    const draft=JSON.parse(await editor.inputValue());assert.equal(draft.cloudflared[0].origin_node_id,1)
    draft.cloudflared[0].token='TEST_ONLY local dummy token000000000000000000'
    await editor.fill(JSON.stringify(draft))
    await page.getByRole('button',{name:'保存并发布',exact:true}).click()
    await page.waitForFunction(()=>document.querySelector('button')!==null)
    for(let i=0;i<30 && writes.length<1;i++)await page.waitForTimeout(100)
    assert.equal(writes.length,1)
    await page.getByRole('button',{name:'保存并发布',exact:true}).waitFor()
    conflict=true
    draft.cloudflared[0].origin_node_id=2
    await editor.fill(JSON.stringify(draft))
    await page.getByRole('button',{name:'保存并发布',exact:true}).click()
    await page.getByText('配置已变化，请刷新后重试',{exact:true}).waitFor()
    assert.equal(JSON.parse(await editor.inputValue()).cloudflared[0].origin_node_id,2)
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false)
    assert.deepEqual(errors,[]);assert.deepEqual(unexpected,[])
    await context.close()
    console.log(`Server profile: ${width}px edit, save and conflict draft passed`)

  }
  console.log('PASS: dist 1440/390, profile edit/example/save, revision conflict preserves draft, no horizontal overflow or unexpected errors')
} finally {
  try { await browser?.close() }
  finally { await new Promise(resolve => server.close(resolve)) }
}
