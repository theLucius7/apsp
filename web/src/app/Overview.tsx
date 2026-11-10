import type { ReactNode } from 'react'
import { Empty, ErrorNotice, InfoTip, Loading, PageHeader, Refresh } from '../components'
import { Stat, StatStrip } from '../observability/display/ui'
import { bytes } from '../format'
import { useResource } from '../hooks'
import type { Node, PluginServer, ProxyUser, Server, Usage } from '../types'
import { installationView } from '../traffic/Runtimes'
import { buttonVariants } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { useMediaQuery } from '@/lib/use-media-query'
import NetworkMap from '../traffic/NetworkMap'
import { ActivityIcon, CircleCheckIcon, ServerIcon, UsersIcon, WaypointsIcon } from 'lucide-react'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

type AlertEvent = { id: number; message: string; server_id: number; server_name: string; resolved_at: number | null }
type Due = { kind: 'item' | 'server' | 'reference'; id: number; name: string; date: string }
type RenewalSummary = { upcoming: Due[] }
const toneDot = { good: 'bg-success', bad: 'bg-destructive', warm: 'bg-warning', neutral: 'bg-muted-foreground/50' } as const
type Attention = { key: string; tone: 'bad' | 'warm'; text: ReactNode; href: string; action: string }

// The overview is the administrator's landing page: what needs attention
// across the proxy network first, then the setup steps until they are done,
// then the sing-box state of every server. Each source is optional; a card
// the administrator may not read simply stays out.
export default function Overview() {
  const pluginServers = useResource<PluginServer[]>('/api/plugins/sing-box/servers', 15_000, { keep: true })
  const servers = useResource<Server[]>('/api/servers', 15_000, { keep: true })
  const nodes = useResource<Node[]>('/api/plugins/sing-box/nodes', 30_000, { keep: true })
  const users = useResource<ProxyUser[]>('/api/plugins/sing-box/users', 30_000, { keep: true })
  const usage = useResource<Usage>('/api/plugins/sing-box/usage', 30_000, { keep: true })
  const alerts = useResource<AlertEvent[]>('/api/notifications', 30_000, { keep: true })
  const renewals = useResource<RenewalSummary>('/api/plugins/expenses/summary', 60_000, { keep: true })
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
    ...failing.map(server => ({ key: `install-${server.id}`, tone: 'bad' as const, text: <><strong>{server.name}</strong> 的 sing-box 安装或运行失败</>, href: `#/servers/${server.id}/services`, action: '查看部署' })),
    ...(active.length ? [{ key: 'alerts', tone: 'warm' as const, text: `${active.length} 条告警尚未恢复${active[0] ? `，最近：${active[0].server_name} · ${active[0].message}` : ''}`, href: '#/alerts', action: '告警事件' }] : []),
    ...due.slice(0, 5).map(entry => ({ key: `due-${entry.kind}-${entry.id}`, tone: 'warm' as const, text: <><strong>{entry.name}</strong> 将在 {entry.date} {entry.kind === 'server' ? '到期' : '续费'}</>, href: entry.kind === 'reference' ? '#/nodes?view=sources' : `#/expenses/${entry.kind === 'server' ? 'servers' : 'items'}/${entry.id}`, action: entry.kind === 'reference' ? '外部来源' : '查看' })),
  ]
  const loaded = [servers, pluginServers, alerts].every(resource => resource.data || resource.error)
  // The drawing needs room for its labels; phones get the same servers as a list.
  const wide = useMediaQuery('(min-width: 900px)')
  // Problems first, then work in progress, running servers, and those without sing-box last.
  const rank = { bad: 0, warm: 1, good: 2, neutral: 3 } as const
  const ordered = (pluginServers.data ?? []).map(server => ({ server, state: installationView(server) }))
    .sort((left, right) => Number(!left.server.enabled) - Number(!right.server.enabled) || rank[left.state.tone] - rank[right.state.tone])
  const states = { ready: ordered.filter(entry => entry.state.tone === 'good').length, attention: ordered.filter(entry => entry.state.tone === 'bad').length,
    waiting: ordered.filter(entry => entry.server.enabled && (entry.state.tone === 'warm' || entry.state.tone === 'neutral')).length, off: ordered.filter(entry => !entry.server.enabled).length }

  // Setup steps stay visible until each one is done.
  const steps = [
    { title: '接入服务器', done: enrolled.length > 0, description: '安装 Agent 并确认服务器在线。监控、资产和日常运维都在「服务器」中管理。', href: '#/servers', action: '管理服务器' },
    { title: '安装 sing-box', done: enabled.length > 0, description: '在需要提供代理的服务器上安装 sing-box，等待设备确认运行状态。', href: '#/nodes?view=runtimes', action: '安装 sing-box' },
    { title: '创建节点与链路', done: Boolean(nodes.data?.length), description: '节点是某台服务器上的代理入口；链路把多台服务器或订阅中的节点串成一条线路。新增监听需要先预检并确认，才会发布到服务器。', href: '#/nodes', action: '创建代理节点' },
    { title: '创建用户并授权', done: Boolean(users.data?.length), description: '用策略组决定代理用户能用哪些节点；需要流量和有效期限制时，再分配套餐。之后把门户或订阅链接发给对方。', href: '#/users', action: '管理代理用户' },
  ]
  const setupDone = steps.every(step => step.done)
  const setupKnown = [servers, pluginServers, nodes, users].every(resource => resource.data || resource.error)

  return <>
    <PageHeader title="概览"><Refresh onClick={reload} /></PageHeader>
    {/* Each figure waits for its own read; one that cannot be read says so instead of "——". */}
    <div className="mb-4"><StatStrip label="代理网络概况" columns={4}>
      <Stat icon={ServerIcon} label="服务器在线" loading={!servers.data && !servers.error} value={servers.data ? `${enrolled.length - offline.length} / ${enrolled.length}` : '—'}>{servers.data ? `${servers.data.length - enrolled.length} 台待接入` : '无法读取'}</Stat>
      <Stat icon={WaypointsIcon} label="代理节点" loading={!nodes.data && !nodes.error} value={nodes.data ? nodes.data.length : '—'}>{pluginServers.data ? `${enabled.length} 台服务器已安装 sing-box` : pluginServers.error ? '无法读取' : ''}</Stat>
      <Stat icon={UsersIcon} label="代理用户" loading={!users.data && !users.error} value={users.data ? users.data.length : '—'}>{users.error ? '无法读取' : ''}</Stat>
      <Stat icon={ActivityIcon} label="累计代理流量" loading={!usage.data && !usage.error} value={usage.data ? bytes(usage.data.total) : '—'} />
    </StatStrip></div>
    {setupKnown && !setupDone && <Panel><PanelHeader><PanelTitle>开始使用</PanelTitle></PanelHeader><ol className="m-0 grid gap-px p-1.5">{steps.map((step, index) => <li className="flex flex-wrap items-center justify-between gap-2 rounded-md px-2.5 py-2" key={step.title}>
      <div className="min-w-0 flex-1"><strong className="flex items-center gap-2 text-[13px] font-medium">{step.done ? <CircleCheckIcon className="text-success size-4" aria-label="已完成" /> : <span className="bg-muted grid size-4 place-items-center rounded-full text-[11px]" aria-hidden="true">{index + 1}</span>}{step.title}</strong>{!step.done && <p className="text-muted-foreground mt-0.5 pl-6 text-xs">{step.description}</p>}</div>
      {!step.done && <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={step.href}>{step.action}</a>}
    </li>)}</ol></Panel>}
    <ErrorNotice message={pluginServers.error} retry={pluginServers.reload} />
      <Panel><PanelHeader className="min-h-10 py-1.5"><PanelTitle>需要处理{attention.length > 0 && <span className="bg-muted text-muted-foreground rounded px-1.5 text-xs font-medium tabular-nums">{attention.length}</span>}</PanelTitle></PanelHeader>
        {!loaded ? <Loading variant="table" rows={3} /> : attention.length ? <ul className="divide-y" data-slot="overview-attention">{attention.map(entry => <li key={entry.key} className="flex items-center gap-2.5 px-4 py-2 text-[13px]">
          <span className={entry.tone === 'bad' ? 'bg-destructive size-2 shrink-0 rounded-full' : 'bg-warning size-2 shrink-0 rounded-full'} title={entry.tone === 'bad' ? '异常' : '注意'} aria-hidden="true" />
          <span className="sr-only">{entry.tone === 'bad' ? '异常' : '注意'}：</span>
          <span className="min-w-0 flex-1 wrap-anywhere">{entry.text}</span>
          <a className={buttonVariants({ variant: 'ghost', size: 'xs' })} href={entry.href}>{entry.action}</a>
        </li>)}</ul> : <p data-slot="overview-clear" className="flex items-center gap-2.5 px-4 py-3.5 text-[13px]"><CircleCheckIcon className="text-success size-4 shrink-0" aria-hidden="true" /><strong className="font-medium">没有需要处理的事项</strong><span className="text-muted-foreground">服务器在线、sing-box 运行正常、告警均已恢复{renewals.data ? '，30 天内也没有待续费的项目' : ''}。</span></p>}
      </Panel>
      <Panel><PanelHeader><span className="flex min-w-0 items-center gap-1.5"><PanelTitle>代理网络</PanelTitle>{wide && <InfoTip label="图例"><p>圆点大小表示本期流量，虚线红圈表示 Agent 离线；点击服务器查看它的 sing-box。</p></InfoTip>}</span>{pluginServers.data && <span data-slot="singbox-summary" className="text-muted-foreground flex flex-wrap items-center gap-x-3 gap-y-1 text-xs font-normal tabular-nums">
            <span className="flex items-center gap-1.5"><i className="bg-success size-2 rounded-full" />{states.ready} 运行</span>
            {states.attention > 0 && <span className="text-destructive flex items-center gap-1.5"><i className="bg-destructive size-2 rounded-full" />{states.attention} 需处理</span>}
            {states.waiting > 0 && <span className="flex items-center gap-1.5"><i className="bg-warning size-2 rounded-full" />{states.waiting} 等待</span>}
            <span className="flex items-center gap-1.5"><i className="border-muted-foreground size-2 rounded-full border" />{states.off} 未安装</span>
            {offline.length > 0 && <span className="flex items-center gap-1.5"><i className="border-destructive size-2.5 rounded-full border border-dashed" />{offline.length} 离线</span>}
          </span>}{states.off > 0 && <a className={buttonVariants({ variant: 'ghost', size: 'xs' })} href="#/nodes?view=runtimes">安装 sing-box</a>}</PanelHeader>
        {pluginServers.loading && !pluginServers.data ? <Loading variant="table" rows={3} /> : !pluginServers.data?.length ? <Empty icon={ServerIcon} title="先接入服务器" description="服务器在线后，再为需要代理服务的设备安装 sing-box。"><a className={buttonVariants()} href="#/servers">添加服务器</a></Empty>
          : wide ? <div className="px-2 py-3"><NetworkMap plugins={pluginServers.data} servers={servers.data} /></div>
          : <ul data-slot="singbox-servers" className="grid grid-cols-[repeat(auto-fill,minmax(9.5rem,1fr))] gap-1.5 p-2.5">{ordered.map(({ server, state }) => <li key={server.id} className="min-w-0">
            <a href={`#/servers/${server.id}/services`} title={state.reason} className={cn('hover:border-foreground/25 focus-visible:ring-ring/50 flex min-w-0 items-center gap-2 rounded-md border px-2.5 py-2 outline-none focus-visible:ring-3', !server.enabled && 'bg-muted/40')}>
              <span className={cn('size-2 shrink-0 rounded-full', toneDot[state.tone])} aria-hidden="true" />
              <span className="grid min-w-0 leading-tight"><strong className="truncate text-[13px] font-medium">{server.name}</strong><small className={cn('truncate text-xs', state.tone === 'bad' ? 'text-destructive' : 'text-muted-foreground')}>{state.label}{server.online ? '' : ' · Agent 离线'}</small></span>
            </a></li>)}</ul>}
      </Panel>
  </>
}
