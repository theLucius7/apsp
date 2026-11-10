import { useState } from 'react'
import type { CSSProperties } from 'react'
import { Badge, Empty, ErrorNotice, InfoTip, Loading, PageHeader, Refresh } from '../../components'
import { time } from '../../format'
import { useResource } from '../../hooks'
import { familyLabel, latency, loss, lossLabel, networkLabel, probeTone } from '../probes'
import type { ProbeOverview } from '../probes'
import type { Server } from '../../types'
import ProbeCharts from '../display/ProbeCharts'
import { QualityBars } from '../display/ProbeQuality'
import { Stat, StatStrip, qualityText } from '../display/ui'
import { carrierFigures, carrierHeading, carriers, lineOrders, lossThreshold, matchLines, reachabilityFigures, serverLines, sortLines } from './line-quality'
import type { Carrier, LineOrder, LineSample, ServerLine } from './line-quality'
import { sortTargets, stateLabels, stateTones } from './reachability'
import type { ReachabilityOverview } from './reachability'
import { ReachabilityEndpoints, ReachabilityEvents } from './ReachabilityEndpoints'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { Button, buttonVariants } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'
import { ChevronDownIcon, GaugeIcon, GlobeIcon, ServerIcon, WaypointsIcon } from 'lucide-react'
import { cn } from '@/lib/utils'

const reachabilityRoot = '/api/plugins/sing-box/reachability'
const settingsPath = '#/latency/settings'
const live = '最近采样'

/** The latest latency and loss of one probe, with the recent samples as two rows of bars. */
function Reading({ label, sample }: { label: string; sample: LineSample }) {
  const current = sample.state === live
  return <div className="min-w-0" title={`${sample.probe.name} · ${sample.probe.target}${sample.probe.port ? `:${sample.probe.port}` : ''} · ${sample.state}`}>
    <div className="flex min-w-0 flex-wrap items-baseline gap-x-2 tabular-nums">
      <strong data-slot="line-latency" className={cn('text-sm font-semibold whitespace-nowrap', qualityText[probeTone(sample.latency, 'latency_ms')])}>{latency(sample.latency)}</strong>
      <span data-slot="line-loss" className={cn('text-xs whitespace-nowrap', qualityText[probeTone(sample.loss, 'loss_percent')])} title={lossLabel(sample.probe)}>{loss(sample.loss)}</span>
    </div>
    {!current && <small className="text-muted-foreground block truncate text-xs">{sample.state}</small>}
    <QualityBars className="mt-1.5 gap-px" slots={sample.slots} field="latency_ms" label={`${label}近期延迟`} />
    <QualityBars className="mt-0.5 gap-px" slots={sample.slots} field="loss_percent" label={`${label}近期${lossLabel(sample.probe)}`} />
  </div>
}

function CarrierCell({ label, samples }: { label: string; samples: LineSample[] }) {
  return <div role="group" className="grid min-w-0 content-start gap-2" aria-label={label}>
    <span className="text-muted-foreground text-xs md:hidden">{label}</span>
    {samples.length ? samples.map(sample => <Reading key={sample.probe.id} label={label} sample={sample} />) : <span className="text-muted-foreground text-xs">未分配</span>}
  </div>
}

function ReachabilityCell({ line, enabled }: { line: ServerLine; enabled: boolean }) {
  const pending = line.endpoints.filter(target => target.pending_state).length
  return <div className="flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 md:grid md:content-start md:justify-items-start" role="group" aria-label="中国可达性">
    <span className="text-muted-foreground text-xs md:hidden">中国可达性</span>
    {line.reachability ? <>
      <Badge tone={stateTones[line.reachability]}>{stateLabels[line.reachability]}</Badge>
      <small className="text-muted-foreground text-xs">{line.endpoints.length} 个端点{pending ? ` · ${pending} 个待确认` : ''}</small>
    </> : <span className="text-muted-foreground text-xs">{enabled ? '没有可检测的端点' : '未启用'}</span>}
  </div>
}

// "广东电信" already names its carrier and region; other names get them added.
function where({ probe }: LineSample) {
  const parts = [networkLabel(probe), probe.monitor?.region?.trim() ?? ''].filter(part => part && !probe.name.includes(part))
  return parts.length ? ` · ${parts.join(' · ')}` : ''
}

/** What is measured on this server, its history and its endpoints as seen from China. */
function Details({ line, owner, now }: { line: ServerLine; owner: boolean; now: number }) {
  const samples = [...carriers.flatMap(([carrier]) => line.carriers[carrier]), ...line.others]
  return <div data-slot="line-details" className="bg-muted/30 grid min-w-0 gap-4 border-t px-4 py-4">
    <section aria-label="检测目标" className="grid min-w-0 gap-2">
      <h3 className="text-sm font-medium">检测目标</h3>
      {samples.length ? <ul className="grid gap-1.5">{samples.map(sample => <li key={sample.probe.id} className="grid min-w-0 gap-x-4 gap-y-0.5 text-xs sm:grid-cols-[minmax(0,1fr)_auto]">
        <span className="min-w-0"><span className="text-foreground">{sample.probe.name}</span><span className="text-muted-foreground">{where(sample)} · {sample.probe.kind === 'tcp' ? 'TCP 连接' : 'ICMP 回显'} · <span className="font-mono">{sample.probe.target}{sample.probe.port ? `:${sample.probe.port}` : ''}</span> · {familyLabel(sample.probe, sample.latest)}</span></span>
        <span className="text-muted-foreground min-w-0 tabular-nums">{sample.state}{sample.latest ? ` · ${time(sample.latest.sampled_at / 1000)}` : ''}{sample.recentLoss !== null ? ` · 近期平均${lossLabel(sample.probe)} ${loss(sample.recentLoss)}` : ''}{sample.latest?.error ? ` · ${sample.latest.error}` : ''}</span>
      </li>)}</ul> : <p className="text-muted-foreground text-xs">这台服务器没有分配延迟任务。<a className="text-primary underline-offset-4 hover:underline" href={settingsPath}>去检测设置</a></p>}
    </section>
    <ProbeCharts base={`/api/servers/${line.server.id}`} now={now} unavailable={!line.server.online} settings={settingsPath} />
    {owner && line.endpoints.length > 0 && <section aria-label="中国可达性" className="grid min-w-0 gap-2">
      <h3 className="text-sm font-medium">中国可达性</h3>
      <ReachabilityEndpoints targets={sortTargets(line.endpoints)} />
    </section>}
    <div className="flex flex-wrap gap-2">
      <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={`#/servers/${line.server.id}`}><ServerIcon aria-hidden="true" />服务器详情</a>
      <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={`#/servers/${line.server.id}/tcp-quality`}><WaypointsIcon aria-hidden="true" />线路诊断</a>
    </div>
  </div>
}

function Row({ line, columns, owner, reachabilityEnabled, open, onToggle, now }: { line: ServerLine; columns: (readonly [Carrier, string])[]; owner: boolean; reachabilityEnabled: boolean; open: boolean; onToggle: () => void; now: number }) {
  const { server } = line, details = `line-details-${server.id}`
  return <li data-slot="server-line" data-problem={line.problems.length > 0 || undefined} className="min-w-0 border-b last:border-b-0">
    <div className="grid min-w-0 grid-cols-3 gap-x-4 gap-y-3 px-4 py-3 md:items-start md:[grid-template-columns:var(--line-columns)]">
      <div className="col-span-3 flex min-w-0 items-start gap-2 md:col-span-1">
        {server.region && <span className="mt-0.5 shrink-0 text-sm leading-none" aria-hidden="true">{server.region}</span>}
        <div className="min-w-0 flex-1">
          <a className="block truncate text-sm font-medium underline-offset-4 hover:underline" href={`#/servers/${server.id}`}>{server.name}</a>
          <span className="text-muted-foreground block truncate text-xs">{[server.group, server.online ? '' : '离线'].filter(Boolean).join(' · ') || '未分组'}</span>
          {line.problems.length > 0 && <span data-slot="line-problems" className="text-warning block text-xs wrap-anywhere">{line.problems.slice(0, 2).join('；')}{line.problems.length > 2 ? ` 等 ${line.problems.length} 项` : ''}</span>}
        </div>
        <Button variant="ghost" size="icon" className="-my-1 shrink-0 md:hidden" aria-expanded={open} aria-controls={details} aria-label={`${server.name} 详情`} onClick={onToggle}><ChevronDownIcon aria-hidden="true" className={cn('transition-transform', open && 'rotate-180')} /></Button>
      </div>
      {columns.map(([carrier, label]) => <CarrierCell key={carrier} label={label} samples={line.carriers[carrier]} />)}
      {owner && <div className="col-span-3 md:col-span-1"><ReachabilityCell line={line} enabled={reachabilityEnabled} /></div>}
      <Button variant="ghost" size="icon" className="-my-1 hidden justify-self-end md:inline-flex" aria-expanded={open} aria-controls={details} aria-label={`${server.name} 详情`} onClick={onToggle}><ChevronDownIcon aria-hidden="true" className={cn('transition-transform', open && 'rotate-180')} /></Button>
    </div>
    {open && <div id={details}><Details line={line} owner={owner} now={now} /></div>}
  </li>
}

export default function LineQuality() {
  const actor = useResource<{ role: string }>('/api/control-center/me', 0)
  const owner = actor.data?.role === 'owner'
  const servers = useResource<Server[]>('/api/servers', 30_000)
  const overview = useResource<ProbeOverview[]>('/api/probes/overview', 15_000)
  const reachability = useResource<ReachabilityOverview>(owner ? reachabilityRoot : null, 60_000)
  const [filter, setFilter] = useState<'all' | 'problems'>('all')
  const [order, setOrder] = useState<LineOrder>('problems')
  const [query, setQuery] = useState('')
  const [open, setOpen] = useState<ReadonlySet<number>>(() => new Set())
  const refresh = () => { servers.reload(); overview.reload(); if (owner) reachability.reload() }
  const now = Date.now()
  const lines = serverLines(servers.data ?? [], overview.data, owner ? reachability.data?.targets : undefined, now, Boolean(overview.error))
  const problems = lines.filter(line => line.problems.length).length
  const columns = carriers.filter(([carrier]) => lines.some(line => line.carriers[carrier].length))
  const visible = sortLines(matchLines(filter === 'problems' ? lines.filter(line => line.problems.length) : lines, query), order)
  const settings = reachability.data?.settings, enabled = Boolean(settings?.enabled)
  const reach = reachabilityFigures(reachability.data?.targets ?? [])
  // Reachability alone is enough to show the rows.
  const measuring = columns.length > 0 || lines.some(line => line.others.length > 0 || line.endpoints.length > 0)
  const template = ['minmax(11rem,1.3fr)', ...columns.map(() => 'minmax(8rem,1fr)'), ...(owner ? ['minmax(8.5rem,0.9fr)'] : []), '2.25rem'].join(' ')
  const statColumns = Math.min(5, Math.max(2, 1 + columns.length + (owner ? 1 : 0))) as 2 | 3 | 4 | 5
  const loading = !servers.data && servers.loading
  // The count waits for every source it depends on, so it does not jump when reachability arrives.
  const counted = Boolean(overview.data || overview.error) && Boolean(actor.data || actor.error) && (!owner || Boolean(reachability.data || reachability.error))
  const toggle = (id: number) => setOpen(current => { const next = new Set(current); if (!next.delete(id)) next.add(id); return next })

  return <>
    <PageHeader title="质量概况"><Refresh onClick={refresh} /></PageHeader>
    <ErrorNotice message={servers.error || overview.error || reachability.error} retry={refresh} />
    {loading ? <Loading variant="table" rows={6} /> : !lines.length ? <Panel><Empty icon={ServerIcon} title="尚无服务器" description="接入服务器后，这里显示它们到三网的延迟与丢包。" /></Panel> : <>
      <StatStrip label="线路质量" columns={statColumns}>
        <Stat icon={ServerIcon} label="需要关注" value={problems} unit="台" tone={problems ? 'warning' : 'good'} loading={!counted}>共 {lines.length} 台服务器{problems ? '' : ' · 全部正常'}</Stat>
        {columns.map(([carrier, label]) => {
          const figures = carrierFigures(lines, carrier)
          return <Stat key={carrier} icon={GaugeIcon} label={carrierHeading(lines, carrier, label)} value={figures.median === null ? '—' : figures.median.toFixed(1)} unit={figures.median === null ? undefined : 'ms'} loading={!overview.data && overview.loading} title="各服务器最近一次采样的中位数">
            {figures.lossy ? <span className="text-warning">{figures.lossy} 台丢包偏高</span> : figures.measured ? `最快 ${latency(figures.fastest)} · 最慢 ${latency(figures.slowest)}` : '暂无采样'}
          </Stat>
        })}
        {owner && <Stat icon={GlobeIcon} label="中国可达性" value={settings && enabled && reach.total ? `${reach.reachable}/${reach.total}` : '—'} tone={reach.blocked ? 'danger' : reach.degraded ? 'warning' : undefined} loading={!reachability.data && reachability.loading}>
          {!settings ? '' : !enabled ? <a className="text-primary underline-offset-4 hover:underline" href={settingsPath}>未启用</a> : reach.blocked ? `${reach.blocked} 个端点被封锁` : reach.degraded ? `${reach.degraded} 个端点部分可达` : reach.total ? '端点全部可达' : '没有可检测的端点'}
        </Stat>}
      </StatStrip>
      <Panel aria-label="服务器线路" className="mt-4" style={{ '--line-columns': template } as CSSProperties}>
        <PanelHeader>
          <span className="flex min-w-0 items-center gap-1.5"><PanelTitle>服务器线路</PanelTitle><InfoTip label="统计说明"><p>延迟与丢包为最近一次采样，下方两行条形为最近 20 个采样时段，灰色表示没有采样。</p><p className="mt-2">需要关注：服务器离线、检测不可用或过期、最近平均丢包不低于 {lossThreshold}%{owner ? '，或中国可达性异常' : ''}。延迟高低与服务器所在地有关，不单独算作异常。</p>{owner && <p className="mt-2">中国可达性由第三方探测点从国内与海外发起 TCP 连接，连续两轮失败才判定为封锁。</p>}</InfoTip></span>
          <div className="flex min-w-0 flex-wrap items-center gap-2 max-sm:w-full">
            <SegmentedGroup aria-label="筛选服务器">
              <SegmentedButton aria-pressed={filter === 'all'} onClick={() => setFilter('all')}>全部 {lines.length}</SegmentedButton>
              <SegmentedButton aria-pressed={filter === 'problems'} onClick={() => setFilter('problems')}>需要关注 {problems}</SegmentedButton>
            </SegmentedGroup>
            <NativeSelect aria-label="排序" className="w-auto" value={order} onChange={event => setOrder(event.target.value as LineOrder)}>{lineOrders.map(([value, label]) => <option key={value} value={value}>{label}</option>)}</NativeSelect>
            <Input type="search" aria-label="搜索服务器" placeholder="搜索服务器" className="w-44 max-sm:w-full max-sm:basis-full" value={query} onChange={event => setQuery(event.target.value)} />
          </div>
        </PanelHeader>
        {!measuring && overview.data ? <Empty icon={GaugeIcon} title="尚未配置延迟任务" description="添加检测目标并分配给服务器后，这里显示每台服务器到三网的延迟与丢包。"><a className={buttonVariants({ size: 'sm' })} href={settingsPath}>添加延迟任务</a></Empty> : <>
          <div aria-hidden="true" className="text-muted-foreground hidden gap-x-4 border-b px-4 py-2 text-xs md:grid md:[grid-template-columns:var(--line-columns)]">
            <span>服务器</span>{columns.map(([carrier, label]) => <span key={carrier}>{carrierHeading(lines, carrier, label)}</span>)}{owner && <span>中国可达性</span>}<span />
          </div>
          {visible.length ? <ul aria-label="服务器线路列表">{visible.map(line => <Row key={line.server.id} line={line} columns={columns} owner={owner} reachabilityEnabled={enabled} open={open.has(line.server.id)} onToggle={() => toggle(line.server.id)} now={now} />)}</ul>
            : <p className="text-muted-foreground px-4 py-8 text-center text-sm">{filter === 'problems' && !query ? '没有需要关注的服务器。' : '没有匹配的服务器。'}</p>}
        </>}
      </Panel>
      {owner && Boolean(reachability.data?.events.length) && <Panel aria-label="可达性变化">
        <PanelHeader><span className="flex min-w-0 items-center gap-1.5"><PanelTitle>可达性变化</PanelTitle><InfoTip label="变化说明"><p>保留 90 天，通过已配置的通知渠道发送。</p></InfoTip></span></PanelHeader>
        <ReachabilityEvents events={reachability.data?.events ?? []} />
      </Panel>}
    </>}
  </>
}
