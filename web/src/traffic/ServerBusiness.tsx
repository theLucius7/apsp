import { useState } from 'react'
import { api } from '../api'
import { Badge, Confirm, DialogPresence, ErrorNotice, Loading, Refresh } from '../components'
import { time } from '../format'
import { resourceWriteError, useAction, useResource, settingsPoll } from '../hooks'
import type { Deployment, Node, RuntimeServer } from '../types'
import RuntimeOperations from './RuntimeOperations'
import ServerProfile from './ServerProfile'
import { canConfigureServer, runtimeStatusView } from './Runtimes'
import { Button, buttonVariants } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ArrowRightIcon, WaypointsIcon } from 'lucide-react'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { cn } from '@/lib/utils'

type Progress = Deployment & { pending?: boolean; enabled_nodes?: number; authorized_nodes?: number }
type Readiness = { ready: boolean; checks: { name: string; passed: boolean; detail: string }[] }

/** Target and applied configuration side by side: the deployment's state at a glance. */
function Revision({ label, value, tone }: { label: string; value: number | null; tone?: 'good' | 'warm' | 'bad' }) {
  return <div className="bg-muted/50 min-w-0 rounded-md px-3 py-2">
    <span className="text-muted-foreground block text-xs">{label}</span>
    <strong className={cn('mt-0.5 block text-lg leading-tight font-semibold tabular-nums', tone === 'good' && 'text-success', tone === 'warm' && 'text-warning', tone === 'bad' && 'text-destructive')}>{value ? `版本 ${value}` : '—'}</strong>
  </div>
}

/**
 * Service configuration and built-in sing-box runtime status on one server. Adding
 * nodes, chains or server-side profile settings expresses desired service state; the
 * self-contained Agent starts and recovers the runtime without a separate install step.
 */
export default function ServerBusiness({ serverId }: { serverId: number }) {
  const metadata = useResource<RuntimeServer>(`/api/traffic/servers/${serverId}`, 15_000)
  const server = metadata.data
  // Desired service intent comes strictly from `configured`, never inferred from past deployments or device health.
  const configured = server?.configured === true
  const writable = server ? canConfigureServer(server) : false
  const deployment = useResource<Progress>(configured ? `/api/traffic/servers/${serverId}/deployments` : null, 15_000)
  const nodes = useResource<Node[]>(configured ? '/api/traffic/nodes' : null, settingsPoll, { keep: true })
  const action = useAction()
  const [redeploying, setRedeploying] = useState(false)
  const [notice, setNotice] = useState('')
  const [check, setCheck] = useState<Readiness | null>(null)
  const status = deployment.data?.status
  const refresh = () => { metadata.reload(); if (configured) { deployment.reload(); nodes.reload() } }
  const readinessError = configured ? resourceWriteError(metadata, deployment) : resourceWriteError(metadata)
  const redeploy = () => {
    if (resourceWriteError(metadata, deployment)) return
    void action.run(() => api<{ revision: number }>(`/api/traffic/servers/${serverId}/deployments/redeploy`, 'POST'), value => { setRedeploying(false); setNotice(`已生成版本 ${value.revision}，等待设备应用。`); refresh() })
  }
  const readiness = () => {
    if (readinessError) return
    setCheck(null)
    void action.run(() => api<Readiness>(`/api/traffic/servers/${serverId}/deployments/check`, 'POST'), setCheck)
  }
  const runtime = server ? runtimeStatusView(server) : undefined
  const own = nodes.data?.filter(node => node.server_id === serverId) ?? []
  const canRedeploy = Boolean(writable && status && (status.applied_rev > 0 || deployment.data?.history.length))
  const health = !status ? undefined : !server?.online ? { label: '当前健康待确认', tone: 'warm' as const } : status.healthy ? { label: '当前配置健康', tone: 'good' as const } : status.applied_rev ? { label: '健康检查未通过', tone: 'bad' as const } : { label: '等待应用', tone: 'warm' as const }
  const metaNotes = [
    server?.online && !server.agent_supported ? '当前 Agent 尚未声明内置运行时能力，请升级 Agent' : '',
    server?.runtime_version ? `运行时 ${server.runtime_version}` : '',
    server?.read_only ? '只读保留主机' : '',
  ].filter(Boolean)
  return <div data-runtime={configured ? 'sing-box' : undefined}>
    <Panel>
      <PanelHeader>
        <PanelTitle>sing-box{runtime && <Badge tone={runtime.tone}>{runtime.label}</Badge>}{!status && server?.status?.current_healthy && server.status.state !== 'ready' && <Badge tone="good">当前配置健康</Badge>}</PanelTitle>
        <div className="flex items-center gap-1">
          <Refresh onClick={refresh} />
          {configured
            ? <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={`#/nodes?server=${serverId}`}>管理节点</a>
            : writable && <a className={buttonVariants({ size: 'sm' })} href={`#/nodes?server=${serverId}`}>添加节点</a>}
        </div>
      </PanelHeader>
      <div className="grid gap-3 p-4">
        <ErrorNotice message={metadata.error || deployment.error || nodes.error || action.error} retry={refresh} />
        {metadata.loading && !server ? <Loading /> : server && runtime && <>
          {!configured ? <div className="grid gap-1.5">
            <p className="text-muted-foreground text-[13px] leading-relaxed">{writable
              ? '当前服务器尚未配置代理节点或链路，纯监控模式不运行代理进程。添加节点、选入转发链路或保存下方服务端配置后，在线一体化 Agent 会自动启动并应用服务。'
              : '当前服务器处于只读保留状态，未在此面板配置新的代理服务。'}</p>
            {runtime.reason && <p className="text-muted-foreground text-xs leading-relaxed">{runtime.reason}</p>}
          </div> : runtime.reason && <p className="text-muted-foreground text-[13px] leading-relaxed">{runtime.reason}</p>}
          {metaNotes.length > 0 && <p className="text-muted-foreground flex flex-wrap gap-x-3 gap-y-1 text-xs">{metaNotes.map(item => <span key={item}>{item}</span>)}</p>}
          {configured && (deployment.loading && !deployment.data ? <Loading /> : status ? <div data-slot="deployment" className="grid gap-2">
            <div className="grid grid-cols-[1fr_auto_1fr] items-center gap-2 sm:max-w-md">
              <Revision label="目标版本" value={status.target_rev} />
              <ArrowRightIcon className="text-muted-foreground size-4" aria-hidden="true" />
              <Revision label="已应用版本" value={status.applied_rev || null} tone={health?.tone} />
            </div>
            <p className="text-muted-foreground flex flex-wrap items-center gap-2 text-xs">{health && <Badge tone={health.tone}>{health.label}</Badge>}最后更新 {time(status.updated_at)}</p>
            {status.last_error && <Notice tone="error" className="mb-0"><div>
              <strong className="font-medium">最近一次部署失败</strong>
              <p className="wrap-anywhere">{status.last_error}</p>
              {status.healthy && <p>旧配置仍健康，节点继续使用上次成功应用的版本。</p>}
            </div></Notice>}
            {canRedeploy && <div><Button size="sm" variant="outline" disabled={action.busy || Boolean(resourceWriteError(metadata, deployment))} onClick={() => { action.clearError(); setRedeploying(true) }}>重新部署</Button></div>}
            {notice && <Notice tone="success" role="status" className="mb-0">{notice}</Notice>}
            {status.target_rev > status.applied_rev && !status.last_error && !['waiting_agent', 'sync_blocked', 'upgrade_required'].includes(server.status?.state ?? '') && <Notice className="mb-0">正在等待设备应用最新配置。设备离线时会在重连后自动同步。</Notice>}
            {deployment.data?.authorized_nodes === 0 && <p className="text-muted-foreground text-[13px] leading-relaxed">还没有节点授权给有效用户，节点在授权前不监听。到<a className="text-primary underline-offset-4 hover:underline" href="#/groups">策略与套餐</a>把节点放进用户所在的策略组；只做链路出口的服务器使用内部连接凭据，不需要放进策略组。</p>}
          </div> : <div className="grid gap-2">
            {(server.status.target_rev > 0 || server.status.applied_rev > 0) && <div className="grid grid-cols-[1fr_auto_1fr] items-center gap-2 sm:max-w-md">
              <Revision label="目标版本" value={server.status.target_rev || null} />
              <ArrowRightIcon className="text-muted-foreground size-4" aria-hidden="true" />
              <Revision label="已应用版本" value={server.status.applied_rev || null} tone={server.status.current_healthy ? 'good' : undefined} />
            </div>}
            <p className="text-muted-foreground text-[13px]">还没有发布记录。保存节点、链路或服务端配置后会自动生成部署并下发到在线 Agent。</p>
          </div>)}
          {(writable || configured) && <div className="grid gap-2">
            <div className="flex flex-wrap items-center gap-2"><Button size="sm" variant="outline" disabled={action.busy || Boolean(readinessError)} onClick={readiness}>{action.busy && !check ? '正在检查…' : '检查运行条件'}</Button><span className="text-muted-foreground text-xs">Agent 接入、设备在线与内置运行时能力</span></div>
            {check && <ul className="grid list-none gap-2 p-0 text-[13px]">{check.checks.map(item => <li className="grid items-start gap-2 leading-relaxed sm:grid-cols-[120px_1fr]" key={item.name}><Badge tone={item.passed ? 'good' : 'warm'}>{item.name}</Badge><span>{item.detail}</span></li>)}</ul>}
          </div>}
          {configured && <div className="grid gap-1.5">
            <span className="text-muted-foreground text-xs">此服务器的节点</span>
            <div className="flex flex-wrap gap-1.5">{own.map(node => <a key={node.id} href={`#/nodes?server=${serverId}`} className="bg-muted/60 hover:border-foreground/25 flex max-w-full items-center gap-1.5 rounded-md border px-2.5 py-1 text-[13px]"><WaypointsIcon className="text-muted-foreground size-3.5 shrink-0" aria-hidden="true" /><span className="truncate">{node.name}</span><code className="text-muted-foreground text-xs">:{node.port}</code></a>)}
              {!own.length && <span className="text-muted-foreground text-[13px]">尚未创建节点。</span>}</div>
          </div>}
        </>}
      </div>
    </Panel>
    <DialogPresence>{redeploying && <Confirm title="重新部署当前配置？" busy={action.busy} error={action.error} confirmLabel="重新部署" busyLabel="正在生成新版本…" tone="default" onClose={() => setRedeploying(false)} onConfirm={redeploy}>
      用同一份服务配置生成新版本并重新下发到这台服务器。日常宿主机重启或进程退出由一体化 Agent 自动受控恢复，仅在需要强制重发当前配置时使用。需要再次验证身份。
    </Confirm>}</DialogPresence>
    {writable && <ServerProfile serverId={serverId} />}
    {configured && <>
      <Panel><div className="p-4"><RuntimeOperations serverId={serverId} status={status} deploymentError={() => resourceWriteError(metadata, deployment)} getStatus={() => deployment.getCurrent()?.status} /></div></Panel>
      <Panel><PanelHeader><PanelTitle>部署历史</PanelTitle><span className="text-muted-foreground text-xs">最近 6 次发布</span></PanelHeader>{deployment.data?.history.length ? <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>版本</TableHead><TableHead>配置包摘要</TableHead><TableHead>发布时间</TableHead></TableRow></TableHeader><TableBody>{deployment.data.history.slice(0, 6).map(item => <TableRow key={item.rev}><TableCell><strong>版本 {item.rev}</strong>{item.rev === status?.applied_rev && <span className="bg-muted text-muted-foreground ml-2 inline-block rounded px-1.5 py-0.5 align-middle text-xs">已应用</span>}</TableCell><TableCell><code title={item.bundle_sha256}>{item.bundle_sha256.slice(0, 16)}…</code></TableCell><TableCell>{time(item.created_at)}</TableCell></TableRow>)}</TableBody></Table></div> : <div className="text-muted-foreground px-4 py-6 text-sm">还没有发布记录。</div>}</Panel>
    </>}
  </div>
}
