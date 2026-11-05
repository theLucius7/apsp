import { api } from '../../api'
import { Badge, Empty, ErrorNotice, Loading, Refresh } from '../../components'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import type { PluginServer } from '../../types'
import { Button, buttonVariants } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ServerIcon } from 'lucide-react'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

/** Why a server counts as installed, when it was not an administrator's install; empty otherwise. */
export function sourceNote(server: PluginServer) {
  if (!server.enabled) return ''
  return server.source === 'agent_capability' ? '按设备声明的 sing-box 能力识别' : server.source === 'legacy_nodes' ? '沿用已有代理节点' : server.source === 'legacy_deployments' ? '沿用已有代理部署' : ''
}

export function installationView(server: PluginServer): { label: string; tone: 'neutral' | 'good' | 'bad' | 'warm'; reason: string } {
  if (!server.enabled) return { label: '未安装', tone: 'neutral', reason: '' }
  const installation = server.installation
  if (!installation) return { label: '安装状态待确认', tone: 'warm', reason: '尚未收到安装结果，请刷新确认。设备支持插件不代表已经安装。' }
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

/** Every server's sing-box installation; one server's lives in its plugins tab. */
export default function Settings() {
  const servers = useResource<PluginServer[]>('/api/plugins/sing-box/servers')
  const action = useAction()
  const writeError = () => resourceWriteError(servers)
  const enable = (id: number) => {
    if (writeError()) return
    const current = servers.getCurrent()?.find(server => server.id === id)
    if (!current || current.id !== id || current.read_only || current.enabled) return
    void action.run(() => api(`/api/plugins/sing-box/servers/${id}/enable`, 'POST', {}), servers.reload)
  }
  return <Panel><PanelHeader><PanelTitle>sing-box 安装与运行状态</PanelTitle><Refresh onClick={servers.reload} /></PanelHeader><div className="p-4"><p className="text-muted-foreground mt-3 text-xs leading-relaxed">选择服务器安装 sing-box。安装成功后，创建节点并为代理用户分配可用节点或链路。</p><ErrorNotice message={servers.error || action.error} retry={servers.reload} /></div>
    {servers.loading && !servers.data ? <Loading /> : !servers.data?.length ? !servers.error && <Empty icon={ServerIcon} title="还没有服务器" description="先添加并接入服务器，再安装插件。"><a className={buttonVariants()} href="#/servers">添加服务器</a></Empty> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>服务器</TableHead><TableHead>安装与运行状态</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{servers.data.map(server => {
      const installation = installationView(server)
      // Only what differs between rows: a healthy install needs no explanation.
      const notes = [server.installation?.state === 'ready' ? '' : installation.reason, server.agent_supported ? '' : '设备尚未声明支持 sing-box', sourceNote(server), server.read_only ? '保留已有安装记录' : ''].filter(Boolean)
      return <TableRow key={server.id}><TableCell><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={`#/servers/${server.id}`}>{server.name}</a></TableCell><TableCell><Badge tone={installation.tone}>{installation.label}</Badge>{notes.map(note => <small key={note}>{note}</small>)}</TableCell><TableCell>{server.enabled ? <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={`#/plugins/sing-box/nodes?server=${server.id}`}>管理节点</a> : <Button size="sm" disabled={action.busy || Boolean(writeError()) || server.read_only} onClick={() => enable(server.id)}>安装 sing-box</Button>}</TableCell></TableRow>
    })}</TableBody></Table></div>}
  </Panel>
}
