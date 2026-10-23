import ServerListView, { defaultServerView } from '../control-center/ServerListView'
import type { ServerView } from '../control-center/ServerListView'
import { Fragment, useState } from 'react'
import { AnimatePresence, m } from 'motion/react'
import { api } from '../api'
import { Badge, Confirm, Empty, ErrorNotice, Loading, Meter, PageHeader, Refresh, Stat } from '../components'
import { ago, bytes, navigate, percent, time } from '../format'
import { useAction, useResource } from '../hooks'
import type { Server } from '../types'
import ServerSetup from './ServerSetup'
import ServerEnrollment from './ServerEnrollment'
import ServerEdit from './ServerEdit'
import { assetDate, assetPrice, defaultAssets, expiryState, trafficSize } from '../server-assets'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ActivityIcon, CheckIcon, LockIcon, PlusIcon, ServerIcon } from 'lucide-react'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { Separator } from '@/components/ui/separator'
import { barMotion } from '@/lib/motion'

export default function Servers() {
  const resource = useResource<Server[]>('/api/servers')
  const actor = useResource<{ role: string; capabilities: string[] }>('/api/control-center/me')
  const canWrite = actor.data?.role === 'owner' || Boolean(actor.data?.capabilities.includes('servers:write'))
  const action = useAction()
  const [editor, setEditor] = useState<Server | 'new' | null>(null)
  const [deleting, setDeleting] = useState<Server | null>(null)
  const [installation, setInstallation] = useState<Server | null>(null)
  const [view, setView] = useState<ServerView>(defaultServerView)
  const [selected, setSelected] = useState<number[]>([])
  const [batchResults, setBatchResults] = useState<Array<{ id: number; status: string }>>([])
  const allServers = resource.data ?? []
  const groupKey = (server: Server) => view.groupBy === 'group' ? server.asset_settings?.group_name || '未分组' : view.groupBy === 'region' ? server.asset_settings?.region || '地区未知' : ''
  const servers = allServers.filter(server => {
    const asset = { ...defaultAssets, ...server.asset_settings }
    return (view.online === 'all' || (view.online === 'online' ? server.online : !server.online))
      && (!view.group || asset.group_name === view.group)
      && `${server.name} ${asset.region} ${asset.tags.join(' ')}`.toLocaleLowerCase().includes(view.query.toLocaleLowerCase())
  }).sort((left, right) => groupKey(left).localeCompare(groupKey(right), 'zh-CN') || (view.sort === 'name' ? left.name.localeCompare(right.name, 'zh-CN') : view.sort === 'cpu' ? (right.latest_metrics.cpu_percent ?? -1) - (left.latest_metrics.cpu_percent ?? -1) : view.sort === 'expiry' ? (left.asset_settings?.expires_at ?? Number.MAX_SAFE_INTEGER) - (right.asset_settings?.expires_at ?? Number.MAX_SAFE_INTEGER) : left.id - right.id))
  const column = (key: string) => view.columns.includes(key)
  const availableSelected = selected.filter(id => allServers.some(server => server.id === id))
  const visibleIds = servers.map(server => server.id)
  const visibleSelected = visibleIds.filter(id => selected.includes(id)).length
  const allVisible = visibleSelected === 0 ? false : visibleSelected === visibleIds.length ? true : 'indeterminate'
  const batchLifecycle = (lifecycle: string) => void action.run(async () => {
    const snapshot = [...availableSelected]
    const results: Array<{ id: number; status: string }> = []
    for (const id of snapshot) {
      try { await api(`/api/servers/${id}/fleet/lifecycle`, 'POST', { lifecycle, from: null, until: null, reason: '管理员列表批量操作' }); results.push({ id, status: '已保存' }) }
      catch (error) { results.push({ id, status: error instanceof Error ? error.message : '操作失败' }) }
      setBatchResults([...results])
    }
    resource.reload()
  })
  const edit = (value: Server | 'new') => { action.clearError(); setEditor(value) }
  return <>
    <PageHeader eyebrow="服务器" title="服务器列表" description="添加并接入服务器，按状态、地区和分组查看运行情况；点开一台服务器查看详情、资产和诊断。"><Refresh onClick={resource.reload} /><Button disabled={!canWrite} onClick={() => edit('new')}><PlusIcon aria-hidden="true" />添加服务器</Button></PageHeader>
    <div className="mb-6 grid grid-cols-3 gap-3 max-sm:gap-2"><Stat icon={ServerIcon} label="服务器总数" value={resource.data ? servers.length : '—'} note="已添加到面板的服务器" /><Stat icon={ActivityIcon} label="当前在线" value={resource.data ? servers.filter(server => server.online).length : '—'} note="最近 60 秒内收到设备消息" /><Stat icon={CheckIcon} label="已发布配置" value={resource.data ? servers.filter(server => server.manifest_rev > 0).length : '—'} note="部署结果可在服务器详情查看" /></div>
    <ServerListView value={view} onChange={setView} groups={[...new Set(allServers.map(server => server.asset_settings?.group_name ?? '').filter(Boolean))]} />
    {batchResults.length > 0 && <Notice><div>{batchResults.map(result => <p key={result.id}>服务器 #{result.id}：{result.status}</p>)}</div></Notice>}
    <ErrorNotice message={resource.error || action.error || actor.error} retry={resource.reload} />{actor.data && !canWrite && <p className="text-muted-foreground text-xs">当前账号只有服务器读取权限；修改、接入和维护操作需要服务器写入授权。</p>}
    <Panel><PanelHeader><PanelTitle>全部服务器 <span className="bg-muted text-muted-foreground min-w-5 rounded px-1.5 text-center text-xs font-medium tabular-nums">{servers.length}</span></PanelTitle><span className="text-muted-foreground flex items-center gap-1.5 text-xs"><span className="bg-success size-1.5 rounded-full" />每 5 秒刷新</span></PanelHeader>
      {resource.loading && !resource.data ? <Loading variant="table" rows={5} /> : !servers.length ? <Empty icon={ServerIcon} title="从第一台服务器开始" description="添加服务器后，在服务器上执行安装命令，即可自动接入。"><Button disabled={!canWrite} onClick={() => edit('new')}><PlusIcon aria-hidden="true" />添加服务器</Button></Empty> : <div className={`max-w-full min-w-0 ${view.density === 'compact' ? '[&_td]:py-1.5' : ''}`}><Table><TableHeader><TableRow><TableHead className="w-10"><Checkbox aria-label="选择当前筛选的全部服务器" checked={allVisible} disabled={!visibleIds.length} onCheckedChange={checked => setSelected(checked === true ? [...new Set([...selected, ...visibleIds])] : selected.filter(id => !visibleIds.includes(id)))} /></TableHead><TableHead>服务器</TableHead><TableHead>状态</TableHead>{column('cpu') && <TableHead>处理器</TableHead>}{column('memory') && <TableHead>内存</TableHead>}{column('cost') && <TableHead>成本 / 到期</TableHead>}{column('traffic') && <TableHead>本期观测 / 额度</TableHead>}{column('version') && <TableHead>设备版本</TableHead>}<TableHead className="text-right">操作</TableHead></TableRow></TableHeader><TableBody>{servers.map((server, index) => {
        const asset = { ...defaultAssets, ...server.asset_settings }, expiry = expiryState(asset)
        const cpu = server.latest_metrics.cpu_percent
        const used = server.latest_metrics.memory_used, total = server.static_info.memory_total
        return <Fragment key={server.id}>{view.groupBy !== 'none' && (index === 0 || groupKey(servers[index - 1]) !== groupKey(server)) && <TableRow><TableHead className="bg-accent text-accent-foreground text-left" colSpan={4 + view.columns.length}>{groupKey(server)}</TableHead></TableRow>}<TableRow><TableCell><Checkbox aria-label={`选择${server.name}`} checked={selected.includes(server.id)} onCheckedChange={checked => setSelected(checked === true ? [...new Set([...selected, server.id])] : selected.filter(id => id !== server.id))} /></TableCell><TableCell><button className="group/entity focus-visible:ring-ring/50 flex items-center gap-3 rounded-md border-0 bg-transparent p-0 text-left outline-none focus-visible:ring-3" onClick={() => navigate(`/servers/${server.id}`)}><span className="bg-muted text-muted-foreground grid size-9 shrink-0 place-items-center rounded-lg border"><ServerIcon className="size-4" aria-hidden="true" /></span><span><strong className="group-hover/entity:text-primary block text-sm font-medium transition-colors">{server.name}</strong><small className="text-muted-foreground mt-0.5 block max-w-52 truncate text-xs">{[asset.region, asset.group_name, server.static_info.hostname ?? `服务器 #${server.id}`].filter(Boolean).join(" · ")}{asset.hidden ? " · 展示隐藏" : ""}</small>{asset.tags.length > 0 && <small className="text-muted-foreground mt-0.5 block max-w-52 truncate text-xs">{asset.tags.join(" · ")}</small>}</span></button></TableCell><TableCell><Badge tone={server.online ? 'good' : 'neutral'}>{server.online ? '在线' : server.device_public_key ? '离线' : '待接入'}</Badge>{!server.online && server.device_public_key && server.last_seen ? <small className="text-muted-foreground mt-1 block text-xs whitespace-nowrap" title={time(server.last_seen)}>最后联络 {ago(server.last_seen)}</small> : null}</TableCell>{column('cpu') && <TableCell><div className="min-w-28 text-sm"><span>{percent(cpu)}</span>{cpu !== undefined && <Meter value={cpu} label={`${server.name} 处理器使用率`} />}</div></TableCell>}{column('memory') && <TableCell><div className="min-w-28 text-sm"><span>{bytes(used)}{used !== undefined && total !== undefined ? <small className="text-muted-foreground text-xs"> / {bytes(total)}</small> : null}</span>{used !== undefined && total ? <Meter value={used / total * 100} label={`${server.name} 内存使用率`} /> : null}</div></TableCell>}{column('cost') && <TableCell><span>{assetPrice(asset)}</span><small>{assetDate(asset.expires_at)} · <span className={expiry.tone === "bad" ? "text-destructive" : ""}>{expiry.label}</span></small></TableCell>}{column('traffic') && <TableCell><span>{(server.traffic?.observed_from != null || server.traffic?.corrected) ? trafficSize(server.traffic.used) : "等待采样"} / {asset.traffic_limit === "0" ? "未设额度" : trafficSize(asset.traffic_limit)}</span>{server.traffic?.exceeded && <small className="text-destructive">额度已用尽</small>}</TableCell>}{column('version') && <TableCell><span className="font-mono text-xs">{server.static_info.agent_version ?? '尚未上报'}</span></TableCell>}<TableCell><div className="flex flex-wrap items-center justify-end gap-2"><Button variant="ghost" size="sm" onClick={() => navigate(`/servers/${server.id}`)}>详情</Button><Button variant="ghost" size="sm" disabled={!canWrite} onClick={() => edit(server)}>编辑</Button><Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={!canWrite} onClick={() => { action.clearError(); setDeleting(server) }}>删除</Button></div></TableCell></TableRow></Fragment>
      })}</TableBody></Table></div>}
    </Panel>
    <AnimatePresence>{availableSelected.length > 0 && <m.div key="bulk" {...barMotion} role="region" aria-label="批量操作" data-slot="bulk-bar"
      className="bg-popover text-popover-foreground ring-foreground/10 sticky bottom-4 z-30 mx-auto mb-4 flex w-fit max-w-full flex-wrap items-center gap-2 rounded-xl px-3 py-2 shadow-lg ring-1">
      <span className="text-sm font-medium tabular-nums">已选择 {availableSelected.length} 台</span><span className="text-muted-foreground text-xs tabular-nums">当前筛选 {servers.length} 台</span>
      <Separator orientation="vertical" className="mx-1 h-5 max-sm:hidden" />
      <Button variant="outline" size="sm" disabled={!canWrite || action.busy || Boolean(resource.error)} onClick={() => batchLifecycle('maintenance')}>进入维护</Button><Button variant="outline" size="sm" disabled={!canWrite || action.busy || Boolean(resource.error)} onClick={() => batchLifecycle('draining')}>停止接收新任务</Button><Button variant="outline" size="sm" disabled={!canWrite || action.busy || Boolean(resource.error)} onClick={() => batchLifecycle('active')}>恢复接收任务</Button>
      <Button variant="ghost" size="sm" onClick={() => setSelected([])}>清空选择</Button>
    </m.div>}</AnimatePresence>
    <div className="text-muted-foreground my-5 flex items-center justify-center gap-2 text-center text-xs leading-relaxed"><LockIcon className="size-3.5" aria-hidden="true" />设备主动连接面板；面板离线时，已应用的节点配置继续运行。</div>
    {editor === 'new' && <ServerSetup onClose={() => setEditor(null)} onCreated={server => { setEditor(null); setInstallation(server); resource.reload() }} />}
    {editor && editor !== 'new' && <ServerEdit key={editor.id} server={editor} onClose={() => setEditor(null)} onSaved={() => { setEditor(null); resource.reload() }} />}
    {deleting && <Confirm title={`删除「${deleting.name}」？`} busy={action.busy} error={action.error} onClose={() => setDeleting(null)} onConfirm={() => void action.run(() => api(`/api/servers/${deleting.id}`, 'DELETE'), () => { setDeleting(null); resource.reload() })}>此服务器将从面板和订阅中移除，历史流量仍会保留。设备在线时会先停止代理与诊断服务、清除本机凭证，确认后再删除；过程可能需要片刻。设备离线时仅移除面板记录，本地服务仍需手动停用。</Confirm>}
    {installation && <ServerEnrollment key={installation.id} server={installation} created onClose={() => setInstallation(null)} />}
  </>
}
