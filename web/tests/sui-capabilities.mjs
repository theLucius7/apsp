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
    let control={paused:false,revision:0}, snapshot=null, sessionError=false
    const connections=[{id:"connection-a",principal:"u1_n1",source:"192.0.2.1:1000",network:"tcp",upload:1024,download:2048}]
    page.on('pageerror',error=>errors.push(error.message))
    let limits={user_id:1,revision:0,upload_mbps:0,download_mbps:0,max_connections:0,servers:[{server_id:1,name:'测试主机',state:'pending'}]},failure=false
    const user={id:1,name:'限制测试用户',subscription_url:'',subscription_token:'',credentials_redacted:true,credential_access_reason:'需要验证身份'}
    await page.route('**/api/**',async route=>{
      const path=new URL(route.request().url()).pathname,method=route.request().method()
      let value=[]
      if (path.endsWith('/control')) {
        if(method==='PUT'){const body=route.request().postDataJSON();writes.push({control:body});control={paused:body.paused,revision:control.revision+1}}
        value=control
      } else if(path.endsWith('/sessions/1') && method==='POST') {
        const body=route.request().postDataJSON();writes.push({sessions:body});snapshot={observed_at:Math.floor(Date.now()/1000),revision:5,truncated:false,closed:body.close?1:0,connections:body.close?[]:connections};value={id:'TEST_ONLY operation'}
      } else if(path.endsWith('/sessions')) {
        if(sessionError)return route.fulfill({status:503,json:{error:'测试会话读取失败'}})
        value={servers:[{server_id:1,name:'测试主机',online:true,supported:true,revision:5,operation:snapshot?{expires_at:Math.floor(Date.now()/1000)+30,result:{error:null,sessions:snapshot}}:null}]}
      } else if(path.endsWith('/client-template')) {value={template:null,definition_redacted:false,limitations:'',credential_access_reason:''}}
      else if (path.endsWith('/limits')) {
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
    await page.getByRole('button',{name:'诊断与安全',exact:true}).click()
    await page.getByRole('button',{name:'暂停使用',exact:true}).click()
    await page.getByRole('button',{name:'恢复使用',exact:true}).waitFor()
    await page.getByRole('button',{name:'恢复使用',exact:true}).click()
    await page.getByRole('button',{name:'暂停使用',exact:true}).waitFor()
    assert.deepEqual(writes.slice(0,2),[{control:{expected_revision:0,paused:true}},{control:{expected_revision:1,paused:false}}])
    await page.getByRole('button',{name:'查询连接',exact:true}).click()
    await page.getByText(/192.0.2.1:1000/).waitFor()
    await page.getByRole('button',{name:'断开',exact:true}).click()
    await page.getByText(/本次确认断开 1 条/).waitFor()
    assert.deepEqual(writes[3],{sessions:{expected_revision:5,close:true,ids:['connection-a']}})
    await page.getByText('高级：sing-box 完整配置模板',{exact:true}).click()
    await page.getByRole('button',{name:'填入双格式自动测速模板',exact:true}).click()
    const draft=JSON.parse(await page.getByLabel('sing-box 1.14.4 模板').inputValue())
    assert.equal(draft.selection_groups[0].type,'urltest');assert.equal(draft.clash['proxy-groups'][0].type,'url-test')
    sessionError=true
    await page.clock.runFor(4000)
    await page.getByText('测试会话读取失败',{exact:true}).waitFor()
    assert.equal(await page.getByRole('button',{name:'查询连接',exact:true}).isDisabled(),true)
    assert.equal(await page.evaluate(()=>document.documentElement.scrollWidth>innerWidth+1),false)
    assert.deepEqual(errors,[])
    await page.close()
    console.log(`S-UI: ${width}px pause/resume, session close, template presets and stale-read protection passed`)
  }
} finally {await browser?.close();await new Promise(done=>server.close(done))}
