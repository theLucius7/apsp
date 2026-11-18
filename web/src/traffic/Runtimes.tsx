import { Badge, Empty, ErrorNotice, Loading, Refresh } from '../components'
import { useResource } from '../hooks'
import type { RuntimeServer, ServiceRuntimeState } from '../types'
import { buttonVariants } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ServerIcon } from 'lucide-react'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

const runtimeStates: Record<ServiceRuntimeState, { label: string; tone: 'neutral' | 'good' | 'bad' | 'warm' }> = {
  unconfigured: { label: '未配置服务', tone: 'neutral' },
  waiting_agent: { label: '等待设备接入', tone: 'neutral' },
  upgrade_required: { label: '待升级 Agent', tone: 'warm' },
  offline: { label: '设备离线', tone: 'neutral' },
  pending: { label: '等待应用配置', tone: 'warm' },
  ready: { label: '运行中', tone: 'good' },
  sync_blocked: { label: '配置同步受阻', tone: 'warm' },
  failed: { label: '应用失败', tone: 'bad' },
}

export function canConfigureServer(server: RuntimeServer): boolean {
  return Boolean(server && Number.isSafeInteger(server.id) && server.id > 0 && server.read_only === false)
}

export function runtimeStatusView(server: RuntimeServer): { label: string; tone: 'neutral' | 'good' | 'bad' | 'warm'; reason: string } {
  const status = server.status
  const view = status ? runtimeStates[status.state] : undefined
  if (!status || !view) return { label: '状态待确认', tone: 'warm', reason: '运行状态暂时无法识别，请刷新确认。' }
  return { ...view, reason: status.reason }
}

/**
 * Every server's service configuration and runtime state at a glance, for the network
 * services workspace. Configuring services or inspecting deployments happens in the
 * server's own 网络服务 tab or when adding nodes.
 */
export default function Runtimes() {
  const servers = useResource<RuntimeServer[]>('/api/traffic/servers')
  return <Panel><PanelHeader><PanelTitle>各服务器的运行时</PanelTitle><Refresh onClick={servers.reload} /></PanelHeader>
    <ErrorNotice message={servers.error} retry={servers.reload} />
    {servers.loading && !servers.data ? <Loading /> : !servers.data?.length ? !servers.error && <Empty icon={ServerIcon} title="还没有服务器" description="先添加并接入服务器，再创建节点或配置网络服务。"><a className={buttonVariants()} href="#/servers">添加服务器</a></Empty> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>服务器</TableHead><TableHead>服务与运行状态</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{servers.data.map(server => {
      const status = runtimeStatusView(server)
      // Only what differs between rows: a healthy running service needs no explanation.
      const notes = [
        server.status?.state === 'ready' ? '' : status.reason,
        server.runtime_version ? `sing-box ${server.runtime_version}` : '',
        server.read_only ? '只读保留主机' : '',
      ].filter(Boolean)
      return <TableRow key={server.id}><TableCell><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={`#/servers/${server.id}`}>{server.name}</a></TableCell><TableCell><Badge tone={status.tone}>{status.label}</Badge>{server.status?.current_healthy && server.status.state !== 'ready' && <Badge tone="good">当前配置健康</Badge>}{notes.map(note => <small key={note}>{note}</small>)}</TableCell><TableCell><a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={`#/servers/${server.id}/services`}>{server.configured ? '查看部署' : '配置服务'}</a></TableCell></TableRow>
    })}</TableBody></Table></div>}
  </Panel>
}
