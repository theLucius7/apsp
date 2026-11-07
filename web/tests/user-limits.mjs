import assert from 'node:assert/strict'
import { createServer } from 'node:http'
import { readFile } from 'node:fs/promises'
import { resolve, extname, sep } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'
import { installControlCenterFixtures } from './control-center-fixtures.mjs'
const { chromium } = await import(process.env.APSP_PLAYWRIGHT_MODULE ? pathToFileURL(process.env.APSP_PLAYWRIGHT_MODULE).href : 'playwright')
const dist = fileURLToPath(new URL('../dist/',import.meta.url))
const server = createServer(async (request,response) => {
  const path = new URL(request.url,'http://127.0.0.1').pathname
  const file = resolve(dist,path === '/' ? 'index.html' : `.${path}`)
  if (!file.startsWith(dist.endsWith(sep) ? dist : `${dist}${sep}`)) return response.writeHead(400).end()
  try { const body=await readFile(file);response.writeHead(200,{'Content-Type':({'.html':'text/html','.js':'text/javascript','.css':'text/css','.svg':'image/svg+xml'})[extname(file)] ?? 'application/octet-stream'}).end(body) } catch {response.writeHead(404).end()}
})
let browser
try {
  await new Promise(done=>server.listen(0,'127.0.0.1',done))
  browser=await chromium.launch({headless:true,...(process.env.APSP_CHROME_PATH?{executablePath:process.env.APSP_CHROME_PATH}:{})})
  const origin=`http://127.0.0.1:${server.address().port}`
  for (const width of [1440,390,320]) {
    const page=await browser.newPage({viewport:{width,height:1000}})
    page.setDefaultTimeout(7000)
    await page.emulateMedia({reducedMotion:'reduce'})
    await page.clock.install()
    const errors=[],writes=[]
    page.on('pageerror',error=>errors.push(error.message))
    let limits={user_id:1,revision:0,upload_mbps:0,download_mbps:0,max_connections:0,servers:[{server_id:1,name:'测试主机',state:'pending'}]},failure=false
    const user={id:1,name:'限制测试用户',subscription_url:'',subscription_token:'',credentials_redacted:true,credential_access_reason:'需要验证身份'}
    await page.route('**/api/**',async route=>{
      const path=new URL(route.request().url()).pathname,method=route.request().method()
      let value=[]
      if (path.endsWith('/limits')) {
        if (method==='PUT') {const body=route.request().postDataJSON();writes.push(body); limits={...limits,...body,revision:limits.revision+1};return route.fulfill({json:limits})}
        if(failure)return route.fulfill({status:503,json:{error:'测试读取失败'}})
        value=limits
      } else if(path==='/api/dashboard/access')value={authenticated:true,public_dashboard:false}
      else if(path==='/api/me')value={authenticated:true}
      else if(path.endsWith('/users'))value=[user]
      else if(path.endsWith('/users/overview'))value=[]
      else if(path.endsWith('/entitlement'))value={user_id:1,status:'unmetered',allowed:true,package_group_id:null,package_name:null,reset_day:null,reset_hour:null,reset_minute:null,timezone:null,starts_at:null,expires_at:null,cycle_start:null,next_reset:null,used_bytes:'0',monthly_bytes:null}
      else if(path.endsWith('/usage'))value={total:'0',uplink:'0',downlink:'0',by_node:[],by_user:[]}
      else if(path.endsWith('/users/1/policy-groups'))value={group_ids:[]}
      else if(path.endsWith('/policy-groups'))value=[]
      else if(path.endsWith('/diagnosis') || path.endsWith('/portal') || path.endsWith('/client-template') || path.includes('/invitations')) return route.fulfill({status:503,json:{error:'该测试未提供此页面数据'}})
      return route.fulfill({json:value})
    })
    await installControlCenterFixtures(page)
    await page.goto(`${origin}/#/users`)
    const edit=page.getByRole('button',{name:'设置限制',exact:true})
    await edit.click().catch(async error => { console.error(errors, (await page.locator("body").innerText()).slice(-4000)); throw error })
    const dialog=page.getByRole('dialog')
    await dialog.getByLabel('上传限速（Mbps）').fill('8')
    await dialog.getByLabel('下载限速（Mbps）').fill('16')
    await dialog.getByLabel('最大连接数',{exact:true}).fill('2')
    await dialog.getByRole('button',{name:'保存限制',exact:true}).click()
    await dialog.waitFor({state:'hidden'})
    assert.deepEqual(writes,[{expected_revision:0,upload_mbps:8,download_mbps:16,max_connections:2}])
    await page.getByText('上传：8 Mbps · 下载：16 Mbps · 连接数：2',{exact:true}).waitFor()
    await page.getByText('测试主机：待服务器应用或确认',{exact:true}).waitFor()
    await edit.click()
    await dialog.getByLabel('上传限速（Mbps）').fill('9')
    failure=true
    await page.clock.runFor(11000)
    await dialog.getByText(/测试读取失败|最新数据|刷新|不可用/).first().waitFor()
    assert.equal(await dialog.getByLabel('上传限速（Mbps）').inputValue(),'9')
    assert.equal(await dialog.getByRole('button',{name:'保存限制',exact:true}).isDisabled(),true)
    assert.equal(writes.length,1)
    failure=false;limits={...limits,revision:2}
    await page.clock.runFor(11000)
    await dialog.getByText(/限制已被其他操作修改/).waitFor()
    assert.equal(await dialog.getByLabel('上传限速（Mbps）').inputValue(),'9')
    assert.equal(await dialog.getByRole('button',{name:'保存限制',exact:true}).isDisabled(),true)
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false)
    assert.deepEqual(errors,[])
    await page.close()
    console.log(`user limits: ${width}px save, pending, stale read, conflict, draft preservation passed`)
  }
} finally {await browser?.close();await new Promise(done=>server.close(done))}
