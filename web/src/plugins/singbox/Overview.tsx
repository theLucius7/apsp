import type { ReactNode } from 'react'
import { Badge, Empty, ErrorNotice, Loading, PageHeader, Refresh, Stat } from '../../components'
import { bytes } from '../../format'
import { useResource } from '../../hooks'
import type { Node, PluginServer, ProxyUser, Server, Usage } from '../../types'
import { installationView } from './Settings'
import { buttonVariants } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ActivityIcon, BellRingIcon, CalendarClockIcon, CircleCheckIcon, ServerIcon, UsersIcon, WaypointsIcon } from 'lucide-react'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

type AlertEvent = { id: number; message: string; server_id: number; server_name: string; resolved_at: number | null }
type Due = { kind: 'item' | 'server' | 'reference'; id: number; name: string; date: string }
type RenewalSummary = { upcoming: Due[] }
type Attention = { key: string; tone: 'bad' | 'warm'; text: ReactNode; href: string; action: string }

// The overview is the administrator's landing page: what needs attention
// across the proxy network first, then the setup steps until they are done,
// then the sing-box state of every server. Each source is optional; a card
// the administrator may not read simply stays out.
export default function Overview() {
  const pluginServers = useResource<PluginServer[]>('/api/plugins/sing-box/servers', 15_000)
  const servers = useResource<Server[]>('/api/servers', 15_000)
  const nodes = useResource<Node[]>('/api/plugins/sing-box/nodes', 30_000)
  const users = useResource<ProxyUser[]>('/api/plugins/sing-box/users', 30_000)
  const usage = useResource<Usage>('/api/plugins/sing-box/usage', 30_000)
  const alerts = useResource<AlertEvent[]>('/api/notifications', 30_000)
  const renewals = useResource<RenewalSummary>('/api/plugins/expenses/summary', 60_000)
  const reload = () => { for (const resource of [pluginServers, servers, nodes, users, usage, alerts, renewals]) resource.reload() }

  const enrolled = servers.data?.filter(server => server.device_public_key) ?? []
  const offline = enrolled.filter(server => !server.online)
  const enabled = pluginServers.data?.filter(server => server.enabled) ?? []
  const failing = enabled.filter(server => server.installation?.state === 'failed')
  const active = alerts.data?.filter(event => event.resolved_at === null) ?? []
  const due = renewals.data?.upcoming ?? []

  const attention: Attention[] = [
    ...offline.slice(0, 5).map(server => ({ key: `offline-${server.id}`, tone: 'bad' as const, text: <>服务器 <strong>{server.name}</strong> 离线</>, href: `#/servers/${server.id}`, action: '查看服务器' })),
    ...(offline.length > 5 ? [{ key: 'offline-more', tone: 'bad' as const, text: `另有 ${offline.length - 5} 台服务器离线`, href: '#/servers', action: '服务器列表' }] : []),
    ...failing.map(server => ({ key: `install-${server.id}`, tone: 'bad' as const, text: <><strong>{server.name}</strong> 的 sing-box 安装或运行失败</>, href: `#/servers/${server.id}/plugins`, action: '查看部署' })),
    ...(active.length ? [{ key: 'alerts', tone: 'warm' as const, text: `${active.length} 条告警尚未恢复${active[0] ? `，最近：${active[0].server_name} · ${active[0].message}` : ''}`, href: '#/system/notifications', action: '告警事件' }] : []),
    ...due.slice(0, 5).map(entry => ({ key: `due-${entry.kind}-${entry.id}`, tone: 'warm' as const, text: <><strong>{entry.name}</strong> 将在 {entry.date} 续费</>, href: '#/plugins/expenses', action: '费用与续费' })),
  ]
  const loaded = [servers, pluginServers, alerts].every(resource => resource.data || resource.error)

  // Setup steps stay visible until each one is done.
  const steps = [
    { title: '接入服务器', done: enrolled.length > 0, description: '安装 Agent 并确认服务器在线。监控、资产和日常运维都在「服务器」中管理。', href: '#/servers', action: '管理服务器' },
    { title: '启用 sing-box', done: enabled.length > 0, description: '为需要提供代理的服务器启用插件，等待设备确认安装和运行状态。', href: '#/system/plugins', action: '启用插件' },
    { title: '创建节点与链路', done: Boolean(nodes.data?.length), description: '节点是某台服务器上的代理入口；链路把多台服务器或订阅中的节点串成一条线路。新增监听需要先预检并确认，才会发布到服务器。', href: '#/plugins/sing-box/nodes', action: '创建代理节点' },
    { title: '创建用户并授权', done: Boolean(users.data?.length), description: '用策略组决定代理用户能用哪些节点；需要流量和有效期限制时，再分配套餐。之后把门户或订阅链接发给对方。', href: '#/plugins/sing-box/users', action: '管理代理用户' },
  ]
  const setupDone = steps.every(step => step.done)
  const setupKnown = [servers, pluginServers, nodes, users].every(resource => resource.data || resource.error)

  return <>
    <PageHeader eyebrow="总览" title="概览" description="代理网络的整体状态：先看需要处理的事项，再看各服务器的代理服务；首次使用时按下面的步骤完成配置。"><Refresh onClick={reload} /></PageHeader>
    <div className="mb-4 grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
      <Stat label="服务器在线" icon={ServerIcon} value={servers.data ? `${enrolled.length - offline.length} / ${enrolled.length}` : '—'} note={servers.data ? `${servers.data.length - enrolled.length} 台待接入` : '无权查看或正在读取'} />
      <Stat label="代理节点" icon={WaypointsIcon} value={nodes.data ? nodes.data.length : '—'} note={pluginServers.data ? `${enabled.length} 台服务器已启用 sing-box` : '无权查看或正在读取'} />
      <Stat label="代理用户" icon={UsersIcon} value={users.data ? users.data.length : '—'} note="每个代理用户拥有独立订阅" />
      <Stat label="累计代理流量" icon={ActivityIcon} value={usage.data ? bytes(usage.data.total) : '—'} note="按用户精确记账，含已删除用户" />
    </div>
    <Panel><PanelHeader><PanelTitle className="flex items-center gap-1.5"><BellRingIcon className="size-4" aria-hidden="true" />需要处理</PanelTitle></PanelHeader>
      {!loaded ? <Loading variant="table" rows={3} /> : attention.length ? <ul className="divide-y" data-slot="overview-attention">{attention.map(entry => <li key={entry.key} className="flex flex-wrap items-center justify-between gap-2 px-4 py-2.5 text-sm">
        <span className="flex min-w-0 items-center gap-2"><Badge tone={entry.tone}>{entry.tone === 'bad' ? '异常' : '注意'}</Badge><span className="min-w-0 wrap-anywhere">{entry.text}</span></span>
        <a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={entry.href}>{entry.action}</a>
      </li>)}</ul> : <Empty icon={CircleCheckIcon} title="没有需要处理的事项" description={`服务器在线、sing-box 运行正常、告警均已恢复${renewals.data ? '，30 天内也没有待续费的项目' : ''}。`} />}
    </Panel>
    {setupKnown && !setupDone && <Panel><PanelHeader><PanelTitle>开始使用</PanelTitle></PanelHeader><div className="p-4"><ol className="m-0 grid gap-1 pl-0">{steps.map((step, index) => <li className="flex flex-wrap items-start justify-between gap-2 py-2" key={step.title}>
      <div className="min-w-0"><strong className="flex items-center gap-2">{step.done ? <CircleCheckIcon className="text-success size-4" aria-label="已完成" /> : <span className="bg-muted grid size-4 place-items-center rounded-full text-[10px]" aria-hidden="true">{index + 1}</span>}{step.title}</strong><p className="text-muted-foreground my-1 text-sm">{step.description}</p></div>
      {!step.done && <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={step.href}>{step.action}</a>}
    </li>)}</ol><p className="text-muted-foreground mt-2 text-xs leading-relaxed">面板保存配置和授权，Agent 将配置应用到对应服务器并回报结果；订阅只提供设备已应用的可用节点。面板暂时离线时，已应用的节点照常工作。</p></div></Panel>}
    <ErrorNotice message={pluginServers.error} retry={pluginServers.reload} />
    <Panel><PanelHeader><PanelTitle>各服务器的 sing-box 状态</PanelTitle></PanelHeader>{pluginServers.loading && !pluginServers.data ? <Loading /> : !pluginServers.data?.length ? <Empty icon={ServerIcon} title="先接入服务器" description="服务器在线后，再为需要代理服务的设备安装 sing-box。"><a className={buttonVariants()} href="#/servers">添加服务器</a></Empty> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>服务器</TableHead><TableHead>安装与运行</TableHead><TableHead>下一步</TableHead></TableRow></TableHeader><TableBody>{pluginServers.data.map(server => {
      const state = installationView(server)
      return <TableRow key={server.id}><TableCell><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={`#/servers/${server.id}`}>{server.name}</a><small>{server.online ? 'Agent 在线' : 'Agent 离线'} · {server.agent_supported ? '支持 sing-box' : '尚未声明支持'}</small></TableCell><TableCell><Badge tone={state.tone}>{state.label}</Badge><small>{state.reason}</small></TableCell><TableCell>{server.enabled ? <div className="flex flex-wrap items-center justify-end gap-2"><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={`#/plugins/sing-box/nodes?server=${server.id}`}>管理此服务器节点</a><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={`#/servers/${server.id}/plugins`}>查看部署</a></div> : <a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href="#/system/plugins">启用并安装</a>}</TableCell></TableRow>
    })}</TableBody></Table></div>}</Panel>
    <Panel><PanelHeader><PanelTitle className="flex items-center gap-1.5"><CalendarClockIcon className="size-4" aria-hidden="true" />常用入口</PanelTitle></PanelHeader><div className="p-4 flex flex-wrap gap-3"><a className={buttonVariants({ variant: 'outline' })} href="#/plugins/sing-box/nodes">代理节点</a><a className={buttonVariants({ variant: 'outline' })} href="#/plugins/sing-box/nodes?kind=chains">链路</a><a className={buttonVariants({ variant: 'outline' })} href="#/plugins/sing-box/groups">策略与套餐</a><a className={buttonVariants({ variant: 'outline' })} href="#/plugins/sing-box/users">代理用户与订阅</a><a className={buttonVariants({ variant: 'outline' })} href="#/plugins/expenses">费用与续费</a></div></Panel>
  </>
}
