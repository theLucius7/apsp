import { Badge, Empty, ErrorNotice, Loading, Refresh } from '../../components'
import { useResource } from '../../hooks'
import type { PluginServer } from '../../types'
import { buttonVariants } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ServerIcon } from 'lucide-react'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

export function installationView(server: PluginServer): { label: string; tone: 'neutral' | 'good' | 'bad' | 'warm'; reason: string } {
  if (!server.enabled) return { label: '未安装', tone: 'neutral', reason: '' }
  const installation = server.installation
  if (!installation) return { label: '安装状态待确认', tone: 'warm', reason: '尚未收到安装结果，请刷新确认。设备支持运行时不代表已经安装。' }
  const states: Record<NonNullable<PluginServer['installation']>['state'], { label: string; tone: 'neutral' | 'good' | 'bad' | 'warm' }> = {
    not_enabled: { label: '未安装', tone: 'neutral' },
    queued: { label: '安装已安排', tone: 'warm' },
    waiting_agent: { label: '等待设备接入', tone: 'neutral' },
    offline: { label: '设备离线', tone: 'neutral' },
    pending: { label: '等待应用配置', tone: 'warm' },
    ready: { label: '已安装并运行', tone: 'good' },
    failed: { label: '安装或部署失败', tone: 'bad' },
  }
  const view = states[installation.state]
  if (!view) return { label: '安装状态待确认', tone: 'warm', reason: '安装状态暂时无法识别，请刷新确认。' }
  return { ...view, reason: installation.reason }
}

/**
 * Every server's runtime at a glance, for the network services workspace. Installing,
 * deploying and runtime operations happen in the server's own 网络服务 tab.
 */
export default function Runtimes() {
  const servers = useResource<PluginServer[]>('/api/plugins/sing-box/servers')
  return <Panel><PanelHeader><PanelTitle>各服务器的运行时</PanelTitle><Refresh onClick={servers.reload} /></PanelHeader>
    <ErrorNotice message={servers.error} retry={servers.reload} />
    {servers.loading && !servers.data ? <Loading /> : !servers.data?.length ? !servers.error && <Empty icon={ServerIcon} title="还没有服务器" description="先添加并接入服务器，再在服务器详情中安装运行时。"><a className={buttonVariants()} href="#/servers">添加服务器</a></Empty> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>服务器</TableHead><TableHead>安装与运行状态</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{servers.data.map(server => {
      const installation = installationView(server)
      // Only what differs between rows: a healthy install needs no explanation.
      const notes = [server.installation?.state === 'ready' ? '' : installation.reason, server.agent_supported ? '' : '设备尚未声明支持 sing-box'].filter(Boolean)
      return <TableRow key={server.id}><TableCell><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={`#/servers/${server.id}`}>{server.name}</a></TableCell><TableCell><Badge tone={installation.tone}>{installation.label}</Badge>{notes.map(note => <small key={note}>{note}</small>)}</TableCell><TableCell><a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={`#/servers/${server.id}/services`}>{server.enabled ? '查看部署' : '安装运行时'}</a></TableCell></TableRow>
    })}</TableBody></Table></div>}
  </Panel>
}
