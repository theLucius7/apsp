import { Badge, Empty, ErrorNotice, Loading } from '../../components'
import { navigate, time } from '../../format'
import { resourceWriteError, useResource } from '../../hooks'
import type { Deployment, Node, PluginServer, Server } from '../../types'
import { installationView, sourceLabel } from './Settings'
import RuntimeOperations from './RuntimeOperations'
import { Button, buttonVariants } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ArrowRightIcon, WaypointsIcon } from 'lucide-react'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'

export default function ServerBusiness({ server }: { server: Server }) {
  const metadata = useResource<PluginServer>(`/api/plugins/sing-box/servers/${server.id}`)
  const enabled = metadata.data?.enabled === true
  const deployment = useResource<Deployment>(enabled ? `/api/plugins/sing-box/servers/${server.id}/deployments` : null)
  const nodes = useResource<Node[]>(enabled ? '/api/plugins/sing-box/nodes' : null)
  const status = deployment.data?.status
  const refresh = () => { metadata.reload(); deployment.reload(); nodes.reload() }
  if (metadata.error) return <ErrorNotice message={metadata.error} retry={metadata.reload} />
  if (!enabled) return null
  const installation = installationView(metadata.data!)
  return <div data-plugin="sing-box"><Notice><div><strong className="font-medium">sing-box 插件</strong> <Badge tone={installation.tone}>{installation.label}</Badge><p>{installation.reason}</p><p>{sourceLabel(metadata.data?.source)}{server.static_info.runtime_version && ` · 运行时 ${server.static_info.runtime_version}`}</p><div className="flex flex-wrap items-center justify-end gap-2"><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={`#/plugins/sing-box/nodes?server=${server.id}`}>创建代理节点</a><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href="#/plugins/sing-box/users">管理代理用户</a><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href="#/plugins/sing-box/nodes?kind=chains">创建链路</a></div></div></Notice>
    <ErrorNotice message={deployment.error || nodes.error} retry={refresh} />
<Panel><PanelHeader><PanelTitle>配置部署</PanelTitle>{status && <Badge tone={status.healthy ? 'good' : status.applied_rev ? 'bad' : 'warm'}>{status.healthy ? '当前配置健康' : status.applied_rev ? '健康检查未通过' : '等待应用'}</Badge>}</PanelHeader>{deployment.loading && !deployment.data ? <Loading /> : !status ? <Empty icon={WaypointsIcon} title="还没有发布记录" description="启用后先安装空运行时；新增有效节点须在下方运维入口采集并确认完整预检。尚无运行时账号时可明确首次安装空运行时，安装回执后再重新预检。" /> : <div className="p-4"><div className="mb-6 flex items-center gap-8"><div className="flex-1"><span className="text-muted-foreground block text-xs">目标版本</span><strong className="mt-2 block text-3xl font-medium tabular-nums"><small>版本</small> {status.target_rev}</strong></div><ArrowRightIcon className="size-4 text-muted-foreground" aria-hidden="true" /><div className="flex-1"><span className="text-muted-foreground block text-xs">已应用版本</span><strong className="mt-2 block text-3xl font-medium tabular-nums"><small>版本</small> {status.applied_rev || '—'}</strong></div></div>{status.last_error && <Notice tone="error"><div><strong className="font-medium">最近一次部署失败</strong><p className="wrap-anywhere">{status.last_error}</p>{status.healthy && <p>旧配置仍健康，节点继续使用上次成功应用的版本。</p>}</div></Notice>}{status.target_rev > status.applied_rev && !status.last_error && <Notice>正在等待设备应用最新配置。设备离线时会在重连后继续。</Notice>}<div className="text-muted-foreground text-xs">最后更新 {time(status.updated_at)}</div></div>}</Panel>
    <Panel><div className="p-4"><RuntimeOperations serverId={server.id} status={status} deploymentError={() => resourceWriteError(metadata, deployment)} getStatus={() => deployment.getCurrent()?.status} /></div></Panel>
    <Panel><PanelHeader><PanelTitle>部署历史</PanelTitle><span className="text-muted-foreground text-xs">最近 6 次发布</span></PanelHeader>{deployment.data?.history.length ? <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>版本</TableHead><TableHead>模块</TableHead><TableHead>配置包摘要</TableHead><TableHead>发布时间</TableHead></TableRow></TableHeader><TableBody>{deployment.data.history.slice(0, 6).map(item => <TableRow key={item.rev}><TableCell><strong>版本 {item.rev}</strong>{item.rev === status?.applied_rev && <span className="bg-muted text-muted-foreground ml-2 inline-block rounded px-1.5 py-0.5 align-middle text-xs">已应用</span>}</TableCell><TableCell>代理运行时</TableCell><TableCell><code title={item.bundle_sha256}>{item.bundle_sha256.slice(0, 16)}…</code></TableCell><TableCell>{time(item.created_at)}</TableCell></TableRow>)}</TableBody></Table></div> : <div className="text-muted-foreground px-4 py-6 text-sm">还没有发布记录。</div>}</Panel>
    <Panel><PanelHeader><PanelTitle>此服务器的节点</PanelTitle><Button variant="ghost" size="sm" onClick={() => navigate(`/plugins/sing-box/nodes?server=${server.id}`)}>管理节点 <ArrowRightIcon aria-hidden="true" /></Button></PanelHeader><div className="flex flex-wrap gap-2 p-4">{nodes.data?.filter(node => node.server_id === server.id).map(node => <span className="bg-muted/60 flex items-center gap-2 rounded-lg border px-3 py-2 text-sm" key={node.id}><WaypointsIcon className="size-4" aria-hidden="true" /><strong>{node.name}</strong><code className="text-muted-foreground text-xs">:{node.port}</code></span>)}{!nodes.data?.some(node => node.server_id === server.id) && <p className="text-muted-foreground text-xs">尚未创建节点。</p>}</div></Panel>
  </div>
}
