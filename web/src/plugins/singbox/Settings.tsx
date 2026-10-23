import { api } from '../../api'
import { Badge, Empty, ErrorNotice, Loading, Refresh } from '../../components'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import type { PluginServer } from '../../types'
import { Button, buttonVariants } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ServerIcon } from 'lucide-react'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

export function sourceLabel(source: PluginServer['source'] | undefined) {
  return source === 'administrator' ? '管理员明确启用' : source === 'agent_capability' ? '设备声明 sing-box 能力' : source === 'legacy_nodes' ? '兼容已有代理节点' : source === 'legacy_deployments' ? '兼容已有代理部署' : '尚未启用'
}

export function installationView(server: PluginServer): { label: string; tone: 'neutral' | 'good' | 'bad' | 'warm'; reason: string } {
  if (!server.enabled) return { label: '未启用', tone: 'neutral', reason: '选择启用并安装后，面板会安排此服务器的 sing-box 安装。' }
  const installation = server.installation
  if (!installation) return { label: '安装状态待确认', tone: 'warm', reason: '尚未收到安装结果，请刷新确认。设备支持插件不代表已经安装。' }
  const states: Record<NonNullable<PluginServer['installation']>['state'], { label: string; tone: 'neutral' | 'good' | 'bad' | 'warm' }> = {
    not_enabled: { label: '未启用', tone: 'neutral' },
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

export default function Settings({ serverId }: { serverId?: number }) {
  const collection = useResource<PluginServer[]>(serverId ? null : '/api/plugins/sing-box/servers')
  const single = useResource<PluginServer>(serverId ? `/api/plugins/sing-box/servers/${serverId}` : null)
  const servers = serverId ? { ...single, data: single.data ? [single.data] : undefined } : collection
  const action = useAction()
  const state = serverId ? single : collection
  const writeError = () => resourceWriteError(state)
  const enable = (id: number) => {
    if (writeError()) return
    const current = serverId ? single.getCurrent() : collection.getCurrent()?.find(server => server.id === id)
    if (!current || current.id !== id || current.read_only || current.enabled) return
    void action.run(() => api(`/api/plugins/sing-box/servers/${id}/enable`, 'POST', {}), servers.reload)
  }
  return <Panel><PanelHeader><PanelTitle>sing-box 安装与运行状态</PanelTitle><Refresh onClick={servers.reload} /></PanelHeader><div className="p-4"><p className="text-muted-foreground mt-3 text-xs leading-relaxed">选择服务器安装 sing-box。安装成功后，创建节点并为代理用户分配可用节点或链路。</p><ErrorNotice message={servers.error || action.error} retry={servers.reload} /></div>
    {servers.loading && !servers.data ? <Loading /> : !servers.data?.length ? !servers.error && <Empty icon={ServerIcon} title="还没有服务器" description="先添加并接入服务器，再安装插件。"><a className={buttonVariants()} href="#/servers">添加服务器</a></Empty> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>服务器</TableHead><TableHead>安装与运行状态</TableHead><TableHead>设备支持</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{servers.data.map(server => {
      const installation = installationView(server)
      return <TableRow key={server.id}><TableCell><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={`#/servers/${server.id}`}>{server.name}</a>{server.enabled && <small>{sourceLabel(server.source)}</small>}</TableCell><TableCell><Badge tone={installation.tone}>{installation.label}</Badge><small>{installation.reason}</small></TableCell><TableCell>{server.agent_supported ? '设备支持 sing-box' : '设备尚未声明支持'}{server.read_only && <small>保留已有启用记录</small>}</TableCell><TableCell>{server.enabled ? <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={`#/plugins/sing-box/nodes?server=${server.id}`}>管理节点</a> : <Button size="sm" disabled={action.busy || Boolean(writeError()) || server.read_only} onClick={() => enable(server.id)}>启用并安装 sing-box</Button>}</TableCell></TableRow>
    })}</TableBody></Table></div>}
  </Panel>
}
