import { useState } from 'react'
import { api } from '../../api'
import { Badge, Confirm, DialogPresence, ErrorNotice, Loading, Refresh } from '../../components'
import { time } from '../../format'
import { resourceWriteError, useAction, useResource, settingsPoll } from '../../hooks'
import type { Deployment, Node, PluginServer } from '../../types'
import RuntimeOperations from './RuntimeOperations'
import ServerProfile from './ServerProfile'
import { installationView } from './Runtimes'
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
 * sing-box on one server, in one card: whether it is installed and running, the
 * deployment the panel wants against what the device applied, and the nodes it
 * serves. Runtime operations and the publish history follow when it is enabled.
 * The list of every server's runtime is a view of the network services workspace.
 */
export default function ServerBusiness({ serverId, runtimeVersion }: { serverId: number; runtimeVersion?: string }) {
  const metadata = useResource<PluginServer>(`/api/plugins/sing-box/servers/${serverId}`, 15_000)
  const enabled = metadata.data?.enabled === true
  const deployment = useResource<Progress>(enabled ? `/api/plugins/sing-box/servers/${serverId}/deployments` : null, 15_000)
  const nodes = useResource<Node[]>(enabled ? '/api/plugins/sing-box/nodes' : null, settingsPoll, { keep: true })
  const action = useAction()
  const [redeploying, setRedeploying] = useState(false)
  const [notice, setNotice] = useState('')
  const [check, setCheck] = useState<Readiness | null>(null)
  const status = deployment.data?.status
  const refresh = () => { metadata.reload(); deployment.reload(); nodes.reload() }
  // The Agent reports this when the service manager restarted the runtime outside it, such
  // as on a host reboot; it then waits for a higher revision instead of restarting on its own.
  const outsideRestart = Boolean(status?.last_error?.includes('controlled runtime instance differs from the successfully applied activation'))
  const redeploy = () => {
    if (resourceWriteError(metadata, deployment)) return
    void action.run(() => api<{ revision: number }>(`/api/plugins/sing-box/servers/${serverId}/deployments/redeploy`, 'POST'), value => { setRedeploying(false); setNotice(`已生成版本 ${value.revision}，等待设备应用。`); refresh() })
  }
  const readiness = () => {
    if (resourceWriteError(metadata, deployment)) return
    setCheck(null)
    void action.run(() => api<Readiness>(`/api/plugins/sing-box/servers/${serverId}/deployments/check`, 'POST'), setCheck)
  }
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
  return <div data-runtime={enabled ? 'sing-box' : undefined}>
    <Panel>
      <PanelHeader>
        <PanelTitle>sing-box{installation && <Badge tone={installation.tone}>{installation.label}</Badge>}</PanelTitle>
        <div className="flex items-center gap-1">
          <Refresh onClick={refresh} />
          {enabled ? <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={`#/nodes?server=${serverId}`}>管理节点</a>
            : server && <Button size="sm" disabled={action.busy || Boolean(resourceWriteError(metadata)) || server.read_only} onClick={enable}>安装 sing-box</Button>}
        </div>
      </PanelHeader>
      <div className="grid gap-3 p-4">
        <ErrorNotice message={metadata.error || deployment.error || nodes.error || action.error} retry={refresh} />
        {metadata.loading && !server ? <Loading /> : server && installation && <>
          {installation.reason && <p className="text-muted-foreground text-[13px] leading-relaxed">{installation.reason}</p>}
          <p className="text-muted-foreground flex flex-wrap gap-x-3 gap-y-1 text-xs">{!server.agent_supported && <span>设备尚未声明支持 sing-box</span>}{enabled && runtimeVersion && <span>运行时 {runtimeVersion}</span>}{server.read_only && <span>保留已有安装记录</span>}</p>
          {enabled && (deployment.loading && !deployment.data ? <Loading /> : status ? <div data-slot="deployment" className="grid gap-2">
            <div className="grid grid-cols-[1fr_auto_1fr] items-center gap-2 sm:max-w-md">
              <Revision label="目标版本" value={status.target_rev} />
              <ArrowRightIcon className="text-muted-foreground size-4" aria-hidden="true" />
              <Revision label="已应用版本" value={status.applied_rev || null} tone={health?.tone} />
            </div>
            <p className="text-muted-foreground flex flex-wrap items-center gap-2 text-xs">{health && <Badge tone={health.tone}>{health.label}</Badge>}最后更新 {time(status.updated_at)}</p>
            {status.last_error && <Notice tone="error" className="mb-0"><div>
              {outsideRestart ? <>
                <strong className="font-medium">运行时在 Agent 之外被重新启动过</strong>
                <p>例如服务器重启后由系统自动拉起。为了不擅自中断流量，Agent 不会自己接管，需要重新部署一次：用同一份配置生成新版本，Agent 受控重启运行时并恢复健康。</p>
                <details className="mt-1"><summary className="cursor-pointer text-xs">原始错误</summary><p className="wrap-anywhere text-xs">{status.last_error}</p></details>
              </> : <><strong className="font-medium">最近一次部署失败</strong><p className="wrap-anywhere">{status.last_error}</p></>}
              {status.healthy && <p>旧配置仍健康，节点继续使用上次成功应用的版本。</p>}
            </div></Notice>}
            {status.applied_rev > 0 && (status.last_error || !status.healthy) && <div><Button size="sm" variant={outsideRestart ? 'default' : 'outline'} disabled={action.busy || Boolean(resourceWriteError(metadata, deployment))} onClick={() => { action.clearError(); setRedeploying(true) }}>重新部署</Button></div>}
            {notice && <Notice tone="success" role="status" className="mb-0">{notice}</Notice>}
            {status.target_rev > status.applied_rev && !status.last_error && <Notice className="mb-0">正在等待设备应用最新配置。设备离线时会在重连后继续。</Notice>}
            {deployment.data?.authorized_nodes === 0 && <p className="text-muted-foreground text-[13px] leading-relaxed">还没有节点授权给有效用户，节点在授权前不监听。到<a className="text-primary underline-offset-4 hover:underline" href="#/groups">策略与套餐</a>把节点放进用户所在的策略组；只做链路出口的服务器使用内部连接凭据，不需要放进策略组。</p>}
          </div> : <p className="text-muted-foreground text-[13px]">还没有发布记录。安装时先部署空运行时；新增有效节点须在下方运维入口采集并确认完整预检。</p>)}
          {enabled && <div className="grid gap-2">
            <div className="flex flex-wrap items-center gap-2"><Button size="sm" variant="outline" disabled={action.busy || Boolean(resourceWriteError(metadata, deployment))} onClick={readiness}>{action.busy && !check ? '正在检查…' : '检查安装条件'}</Button><span className="text-muted-foreground text-xs">设备在线、运行时能力与签名制品</span></div>
            {check && <ul className="grid list-none gap-2 p-0 text-[13px]">{check.checks.map(item => <li className="grid items-start gap-2 leading-relaxed sm:grid-cols-[120px_1fr]" key={item.name}><Badge tone={item.passed ? 'good' : 'warm'}>{item.name}</Badge><span>{item.detail}</span></li>)}</ul>}
          </div>}
          {enabled && <div className="grid gap-1.5">
            <span className="text-muted-foreground text-xs">此服务器的节点</span>
            <div className="flex flex-wrap gap-1.5">{own.map(node => <a key={node.id} href={`#/nodes?server=${serverId}`} className="bg-muted/60 hover:border-foreground/25 flex max-w-full items-center gap-1.5 rounded-md border px-2.5 py-1 text-[13px]"><WaypointsIcon className="text-muted-foreground size-3.5 shrink-0" aria-hidden="true" /><span className="truncate">{node.name}</span><code className="text-muted-foreground text-xs">:{node.port}</code></a>)}
              {!own.length && <span className="text-muted-foreground text-[13px]">尚未创建节点。</span>}</div>
          </div>}
        </>}
      </div>
    </Panel>
    <DialogPresence>{redeploying && <Confirm title="重新部署当前配置？" busy={action.busy} error={action.error} confirmLabel="重新部署" busyLabel="正在生成新版本…" tone="default" onClose={() => setRedeploying(false)} onConfirm={redeploy}>
      用同一份配置生成新版本并下发到这台服务器。运行时已脱离 Agent 的受控实例时（例如服务器重启后），Agent 会受控重启一次，期间这台服务器的线路中断几秒；运行时正常时不会重启。需要再次验证身份。
    </Confirm>}</DialogPresence>
    {enabled && <>
      <ServerProfile serverId={serverId} />
      <Panel><div className="p-4"><RuntimeOperations serverId={serverId} status={status} deploymentError={() => resourceWriteError(metadata, deployment)} getStatus={() => deployment.getCurrent()?.status} /></div></Panel>
      <Panel><PanelHeader><PanelTitle>部署历史</PanelTitle><span className="text-muted-foreground text-xs">最近 6 次发布</span></PanelHeader>{deployment.data?.history.length ? <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>版本</TableHead><TableHead>配置包摘要</TableHead><TableHead>发布时间</TableHead></TableRow></TableHeader><TableBody>{deployment.data.history.slice(0, 6).map(item => <TableRow key={item.rev}><TableCell><strong>版本 {item.rev}</strong>{item.rev === status?.applied_rev && <span className="bg-muted text-muted-foreground ml-2 inline-block rounded px-1.5 py-0.5 align-middle text-xs">已应用</span>}</TableCell><TableCell><code title={item.bundle_sha256}>{item.bundle_sha256.slice(0, 16)}…</code></TableCell><TableCell>{time(item.created_at)}</TableCell></TableRow>)}</TableBody></Table></div> : <div className="text-muted-foreground px-4 py-6 text-sm">还没有发布记录。</div>}</Panel>
    </>}
  </div>
}
