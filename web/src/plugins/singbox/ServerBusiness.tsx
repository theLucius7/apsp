import { api } from '../../api'
import { Badge, ErrorNotice, Loading, Refresh } from '../../components'
import { time } from '../../format'
import { resourceWriteError, useAction, useResource, settingsPoll } from '../../hooks'
import type { Deployment, Node, PluginServer } from '../../types'
import RuntimeOperations from './RuntimeOperations'
import { installationView, sourceLabel } from './Settings'
import { Button, buttonVariants } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ArrowRightIcon, WaypointsIcon } from 'lucide-react'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { cn } from '@/lib/utils'

/** Target and applied configuration side by side: the deployment's state at a glance. */
function Revision({ label, value, tone }: { label: string; value: number | null; tone?: 'good' | 'warm' | 'bad' }) {
  return <div className="bg-muted/50 min-w-0 rounded-md px-3 py-2">
    <span className="text-muted-foreground block text-xs">{label}</span>
    <strong className={cn('mt-0.5 block text-lg leading-tight font-semibold tabular-nums', tone === 'good' && 'text-success', tone === 'warm' && 'text-warning', tone === 'bad' && 'text-destructive')}>{value ? `版本 ${value}` : '—'}</strong>
  </div>
}

/**
 * sing-box on one server, in one card: whether it is installed and running, the
 * deployment the panel wants against what the device applied, and the nodes it
 * serves. Runtime operations and the publish history follow when it is enabled.
 * The collection view of every server stays in the plugin centre.
 */
export default function ServerBusiness({ serverId, runtimeVersion }: { serverId: number; runtimeVersion?: string }) {
  const metadata = useResource<PluginServer>(`/api/plugins/sing-box/servers/${serverId}`, 15_000)
  const enabled = metadata.data?.enabled === true
  const deployment = useResource<Deployment>(enabled ? `/api/plugins/sing-box/servers/${serverId}/deployments` : null, 15_000)
  const nodes = useResource<Node[]>(enabled ? '/api/plugins/sing-box/nodes' : null, settingsPoll, { keep: true })
  const action = useAction()
  const status = deployment.data?.status
  const refresh = () => { metadata.reload(); deployment.reload(); nodes.reload() }
  const enable = () => {
    if (resourceWriteError(metadata)) return
    const current = metadata.getCurrent()
    if (!current || current.read_only || current.enabled) return
    void action.run(() => api(`/api/plugins/sing-box/servers/${serverId}/enable`, 'POST', {}), refresh)
  }
  const server = metadata.data
  const installation = server ? installationView(server) : undefined
  const own = nodes.data?.filter(node => node.server_id === serverId) ?? []
  const health = !status ? undefined : status.healthy ? { label: '当前配置健康', tone: 'good' as const } : status.applied_rev ? { label: '健康检查未通过', tone: 'bad' as const } : { label: '等待应用', tone: 'warm' as const }
  return <div data-plugin={enabled ? 'sing-box' : undefined}>
    <Panel>
      <PanelHeader>
        <PanelTitle>sing-box{installation && <Badge tone={installation.tone}>{installation.label}</Badge>}</PanelTitle>
        <div className="flex items-center gap-1">
          <Refresh onClick={refresh} />
          {enabled ? <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={`#/plugins/sing-box/nodes?server=${serverId}`}>管理节点</a>
            : server && <Button size="sm" disabled={action.busy || Boolean(resourceWriteError(metadata)) || server.read_only} onClick={enable}>启用并安装 sing-box</Button>}
        </div>
      </PanelHeader>
      <div className="grid gap-3 p-4">
        <ErrorNotice message={metadata.error || deployment.error || nodes.error || action.error} retry={refresh} />
        {metadata.loading && !server ? <Loading /> : server && installation && <>
          <p className="text-muted-foreground text-[13px] leading-relaxed">{installation.reason}</p>
          <p className="text-muted-foreground flex flex-wrap gap-x-3 gap-y-1 text-xs"><span>{server.agent_supported ? '设备支持 sing-box' : '设备尚未声明支持'}</span>{server.enabled && <span>{sourceLabel(server.source)}</span>}{enabled && runtimeVersion && <span>运行时 {runtimeVersion}</span>}{server.read_only && <span>保留已有启用记录</span>}</p>
          {enabled && (deployment.loading && !deployment.data ? <Loading /> : status ? <div data-slot="deployment" className="grid gap-2">
            <div className="grid grid-cols-[1fr_auto_1fr] items-center gap-2 sm:max-w-md">
              <Revision label="目标版本" value={status.target_rev} />
              <ArrowRightIcon className="text-muted-foreground size-4" aria-hidden="true" />
              <Revision label="已应用版本" value={status.applied_rev || null} tone={health?.tone} />
            </div>
            <p className="text-muted-foreground flex flex-wrap items-center gap-2 text-xs">{health && <Badge tone={health.tone}>{health.label}</Badge>}最后更新 {time(status.updated_at)}</p>
            {status.last_error && <Notice tone="error" className="mb-0"><div><strong className="font-medium">最近一次部署失败</strong><p className="wrap-anywhere">{status.last_error}</p>{status.healthy && <p>旧配置仍健康，节点继续使用上次成功应用的版本。</p>}</div></Notice>}
            {status.target_rev > status.applied_rev && !status.last_error && <Notice className="mb-0">正在等待设备应用最新配置。设备离线时会在重连后继续。</Notice>}
          </div> : <p className="text-muted-foreground text-[13px]">还没有发布记录。启用后先安装空运行时；新增有效节点须在下方运维入口采集并确认完整预检。</p>)}
          {enabled && <div className="grid gap-1.5">
            <span className="text-muted-foreground text-xs">此服务器的节点</span>
            <div className="flex flex-wrap gap-1.5">{own.map(node => <a key={node.id} href={`#/plugins/sing-box/nodes?server=${serverId}`} className="bg-muted/60 hover:border-foreground/25 flex max-w-full items-center gap-1.5 rounded-md border px-2.5 py-1 text-[13px]"><WaypointsIcon className="text-muted-foreground size-3.5 shrink-0" aria-hidden="true" /><span className="truncate">{node.name}</span><code className="text-muted-foreground text-xs">:{node.port}</code></a>)}
              {!own.length && <span className="text-muted-foreground text-[13px]">尚未创建节点。</span>}</div>
          </div>}
        </>}
      </div>
    </Panel>
    {enabled && <>
      <Panel><div className="p-4"><RuntimeOperations serverId={serverId} status={status} deploymentError={() => resourceWriteError(metadata, deployment)} getStatus={() => deployment.getCurrent()?.status} /></div></Panel>
      <Panel><PanelHeader><PanelTitle>部署历史</PanelTitle><span className="text-muted-foreground text-xs">最近 6 次发布</span></PanelHeader>{deployment.data?.history.length ? <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>版本</TableHead><TableHead>配置包摘要</TableHead><TableHead>发布时间</TableHead></TableRow></TableHeader><TableBody>{deployment.data.history.slice(0, 6).map(item => <TableRow key={item.rev}><TableCell><strong>版本 {item.rev}</strong>{item.rev === status?.applied_rev && <span className="bg-muted text-muted-foreground ml-2 inline-block rounded px-1.5 py-0.5 align-middle text-xs">已应用</span>}</TableCell><TableCell><code title={item.bundle_sha256}>{item.bundle_sha256.slice(0, 16)}…</code></TableCell><TableCell>{time(item.created_at)}</TableCell></TableRow>)}</TableBody></Table></div> : <div className="text-muted-foreground px-4 py-6 text-sm">还没有发布记录。</div>}</Panel>
    </>}
  </div>
}
