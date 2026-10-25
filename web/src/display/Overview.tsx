import { useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { AnimatePresence, m } from 'motion/react'
import { ArrowDownIcon, ArrowUpIcon, DatabaseIcon, LayoutGridIcon, ListFilterIcon, RefreshCwIcon, SearchIcon, ServerIcon, TableIcon, WalletIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import type { ProbeOverview } from '../probes'
import type { Server } from '../types'
import { aggregate, fresh, network, size, speed } from './data'
import { dashboardCounts, savedSort, savedView, selectServers, snapshotUnavailable } from './dashboard'
import type { DashboardFilter, DashboardSort, DashboardView } from './dashboard'
import { ServerCard } from './ServerCard'
import ServerTable from './ServerTable'
import { useDashboardPoll } from './useDashboardPoll'
import { useDashboardServers } from './useDashboardServers'
import { useCurrency } from './CurrencyContext'
import { costSummary, money, quoteState } from './currency'
import { DisplayNotice, surface, toneText } from './ui'
import { cn } from '@/lib/utils'
import { itemMotion, revealMotion } from '@/lib/motion'
import { Button, buttonVariants } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Skeleton } from '@/components/ui/skeleton'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'

function Stat({ icon: Icon = ServerIcon, label, value, unit, children, tone, title }: { icon?: LucideIcon; label: string; value: ReactNode; unit?: string; children: ReactNode; tone?: string; title?: string }) {
  return <div data-slot="overview-item" className="min-w-0 border-border px-5 py-4 max-sm:min-h-24 max-sm:px-3.5 max-sm:py-3" title={title}>
    <div data-slot="overview-label" className="text-muted-foreground mb-2 flex items-center justify-between gap-2 text-sm font-medium">
      <span>{label}</span>
      <span className={cn('bg-primary/10 text-primary grid size-7 place-items-center rounded-md', tone && toneText[tone])}><Icon className="size-4" aria-hidden="true" /></span>
    </div>
    <div data-slot="overview-value" className="flex min-w-0 items-baseline gap-1.5">
      <strong className={cn('truncate text-[1.75rem] leading-tight font-semibold tracking-tight tabular-nums max-sm:text-2xl', tone && toneText[tone])}>{value}</strong>
      {unit && <b className="text-muted-foreground shrink-0 text-sm font-medium">{unit}</b>}
    </div>
    <div className="text-muted-foreground mt-1.5 flex min-h-4 items-center gap-2 overflow-hidden text-xs whitespace-nowrap *:truncate">{children}</div>
  </div>
}

function readPreference(key: string): string | null {
  try { return localStorage.getItem(`apsp-dashboard-${key}`) } catch { return null }
}
function savePreference(key: string, value: string) {
  try { localStorage.setItem(`apsp-dashboard-${key}`, value) } catch { /* Keep the in-memory choice when storage is disabled. */ }
}
const filters: [DashboardFilter, string][] = [['all', '全部'], ['online', '在线'], ['offline', '离线'], ['pending', '待接入'], ['stale', '指标待更新']]

export default function Overview({ now }: { now: number }) {
  const resource = useDashboardServers(false)
  const { currency, quote, error: currencyError } = useCurrency()
  const { data: servers, error, loading, updatedAt } = resource
  const probes = useDashboardPoll<ProbeOverview[]>('/api/dashboard/probes/overview', 15_000, false, servers?.some(server => server.public_view) ? 'public' : 'admin')
  const [view, setView] = useState<DashboardView>(() => savedView(readPreference('view')))
  const [sort, setSort] = useState<DashboardSort>(() => savedSort(readPreference('sort')))
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<DashboardFilter>('all')
  const [group, setGroup] = useState('')
  const [region, setRegion] = useState('')
  const [optionsOpen, setOptionsOpen] = useState(false)
  const unavailable = snapshotUnavailable(updatedAt, now, false, error)
  const probeUnavailable = Boolean(probes.error) || probes.updatedAt === null || now - probes.updatedAt > 45_000
  const byServer = useMemo(() => {
    const grouped = new Map<number, ProbeOverview[]>()
    for (const entry of probes.data ?? []) {
      const items = grouped.get(entry.server_id) ?? []
      items.push(entry); grouped.set(entry.server_id, items)
    }
    return grouped
  }, [probes.data])
  const entries = useMemo(() => (servers ?? []).filter(server => !server.asset_settings?.hidden), [servers])
  const groups = [...new Set(entries.map(server => server.asset_settings?.group_name).filter((value): value is string => Boolean(value)))].sort()
  const regions = [...new Set(entries.map(server => server.asset_settings?.region).filter((value): value is string => Boolean(value)))].sort()
  const visible = useMemo(() => selectServers(entries, query, filter, group, region, sort, unavailable), [entries, query, filter, group, region, sort, unavailable])
  const counts = dashboardCounts(entries)
  const upload = aggregate(entries, 'transmit_bytes_per_sec', true), download = aggregate(entries, 'receive_bytes_per_sec', true)
  const completeCounters = entries.filter(server => network(server.latest_metrics, 'transmitted_bytes', server.asset_settings?.network_interface) !== null && network(server.latest_metrics, 'received_bytes', server.asset_settings?.network_interface) !== null)
  const sent = aggregate(completeCounters, 'transmitted_bytes'), received = aggregate(completeCounters, 'received_bytes')
  const total = sent.value === null || received.value === null ? null : sent.value + received.value
  const costs = costSummary(entries, currency, quote, now)
  const showCosts = entries.some(server => !server.public_view)
  const needsConversion = entries.some(server => !server.public_view && server.asset_settings?.price !== null && server.asset_settings?.price !== undefined && server.asset_settings.currency !== currency)
  const referenceState = quoteState(quote, currencyError)
  const staleCosts = needsConversion && referenceState !== 'fresh'
  const busiest = (field: 'transmit_bytes_per_sec' | 'receive_bytes_per_sec') => entries.filter(fresh).reduce<Server | null>((best, server) => (network(server.latest_metrics, field, server.asset_settings?.network_interface) ?? -1) > (best ? network(best.latest_metrics, field, best.asset_settings?.network_interface) ?? -1 : -1) ? server : best, null)
  const topUpload = busiest('transmit_bytes_per_sec'), topDownload = busiest('receive_bytes_per_sec')
  const peak = (server: Server | null, field: 'transmit_bytes_per_sec' | 'receive_bytes_per_sec') => {
    if (unavailable) return '等待更新'
    if (!server) return '暂无有效速率数据'
    const value = network(server.latest_metrics, field, server.asset_settings?.network_interface)
    return value === null ? '暂无有效速率数据' : value > 0 ? <span title={server.name}>峰值 {server.name}</span> : '暂无实时流量'
  }
  const [uploadValue, uploadUnit] = (unavailable ? '—' : speed(upload.value)).split(' ')
  const [downloadValue, downloadUnit] = (unavailable ? '—' : speed(download.value)).split(' ')
  const refresh = () => { resource.reload(); probes.reload() }
  const reset = () => { setQuery(''); setFilter('all'); setGroup(''); setRegion('') }
  const changed = Boolean(query || filter !== 'all' || group || region)
  const customised = filter !== 'all' || Boolean(region) || sort !== 'default' || view !== 'cards'
  const chooseView = (value: DashboardView) => { setView(value); savePreference('view', value) }
  const chooseSort = (value: string) => { const next = savedSort(value); setSort(next); savePreference('sort', next) }
  return <div data-slot="dashboard-home" className="flex flex-col gap-4">
    <section aria-label="服务器总览" className={cn(surface, 'grid overflow-hidden [&>*]:border-border max-lg:grid-cols-2 max-lg:[&>*:nth-child(even)]:border-l max-lg:[&>*:not(:nth-last-child(-n+2))]:border-b max-lg:[&>*:last-child:nth-child(odd)]:col-span-full lg:[&>*+*]:border-l', showCosts ? 'lg:grid-cols-5' : 'lg:grid-cols-4')}>
      <Stat icon={ServerIcon} label="在线服务器" value={!servers || unavailable ? '—' : counts.online} unit={servers ? `/ ${entries.length} 台` : undefined} tone="good">{unavailable ? '等待更新' : !entries.length ? '等待接入' : counts.online === counts.all ? '全部运行正常' : `${counts.offline} 台离线${counts.pending ? ` · ${counts.pending} 台待接入` : ''}`}</Stat>
      {showCosts && <Stat icon={WalletIcon} label="资产" value={money(costs.total, currency)} title={`${costs.converted} 台可折算 · ${costs.missingPrices} 台未填写费用；每 30 天约 ${money(costs.recurring, currency)}`}><span>剩余价值 {money(costs.remaining, currency)}</span>{costs.missingRates > 0 ? <span className="text-warning" title={`${costs.missingRates} 台缺少汇率，未计入合计`}>汇率缺失</span> : staleCosts ? <span className="text-warning" title={referenceState === 'read-error' ? '汇率读取失败，使用上次有效汇率' : referenceState === 'stale' ? '使用上次有效汇率' : '无法确认参考汇率状态'}>{referenceState === 'read-error' ? '汇率读取失败' : referenceState === 'stale' ? '旧汇率' : '汇率状态未知'}</span> : null}</Stat>}
      <Stat icon={DatabaseIcon} label="累计流量" value={size(total)} title={`网卡累计 · ${completeCounters.length} / ${entries.length} 台有完整计数`}><span className="text-success">上传 {size(sent.value)}</span><span className="text-info">下载 {size(received.value)}</span></Stat>
      <Stat icon={ArrowUpIcon} label="实时上行" value={uploadValue} unit={uploadUnit} tone="good" title={`${upload.count} / ${entries.length} 台有有效采样`}>{peak(topUpload, 'transmit_bytes_per_sec')}</Stat>
      <Stat icon={ArrowDownIcon} label="实时下行" value={downloadValue} unit={downloadUnit} tone="info" title={`${download.count} / ${entries.length} 台有有效采样`}>{peak(topDownload, 'receive_bytes_per_sec')}</Stat>
    </section>
    <div data-slot="dashboard-toolbar" className="min-w-0">
      <div className="flex min-h-9 items-center gap-3 max-lg:flex-wrap max-lg:gap-2">
        <label className="relative flex min-w-44 flex-[0_1_300px] items-center max-lg:flex-[1_1_180px] max-sm:min-w-24 max-sm:flex-[1_1_100px]">
          <SearchIcon className="text-muted-foreground pointer-events-none absolute left-2.5 size-4" aria-hidden="true" />
          <Input type="search" aria-label="搜索服务器" placeholder="搜索服务器" value={query} onChange={event => setQuery(event.target.value)} className="pl-8" />
        </label>
        <SegmentedGroup aria-label="服务器分组" className="no-scrollbar max-w-full flex-nowrap overflow-x-auto max-lg:order-3 max-lg:flex-[1_1_100%]">
          <SegmentedButton aria-pressed={!group} onClick={() => setGroup('')}>全部</SegmentedButton>
          {groups.map(name => <SegmentedButton key={name} aria-pressed={group === name} onClick={() => setGroup(name)}>{name}</SegmentedButton>)}
        </SegmentedGroup>
        <div className="ml-auto flex flex-none items-center gap-1">
          <span className="text-muted-foreground mr-1.5 text-xs whitespace-nowrap">{servers ? `${visible.length} 台服务器` : '等待数据'}</span>
          <Button variant="ghost" size="icon" className={cn(customised && 'bg-primary/10 text-primary')} aria-label="筛选与视图" title="筛选与视图" aria-expanded={optionsOpen} aria-controls="dashboard-options" onClick={() => setOptionsOpen(value => !value)}><ListFilterIcon /></Button>
          <Button variant="ghost" size="icon" aria-label="刷新服务器" title="刷新服务器" onClick={refresh} disabled={loading && probes.loading}><RefreshCwIcon /></Button>
        </div>
      </div>
      <AnimatePresence initial={false}>
        {optionsOpen && <m.div key="options" {...revealMotion} className="overflow-hidden">
          <div id="dashboard-options" className="mt-3 flex flex-wrap items-center gap-3 border-t pt-3">
            <SegmentedGroup aria-label="服务器状态筛选" className="max-w-full max-sm:flex-[1_1_100%] max-sm:*:flex-1">
              {filters.map(([value, label]) => <SegmentedButton key={value} aria-label={label} aria-pressed={filter === value} onClick={() => setFilter(value)}>{label}<span data-slot="filter-count" className="text-muted-foreground text-xs tabular-nums" aria-hidden="true">{!servers || (value !== 'all' && unavailable) ? '—' : counts[value]}</span></SegmentedButton>)}
            </SegmentedGroup>
            <div className="text-muted-foreground flex flex-wrap gap-3 text-xs max-sm:w-full max-sm:gap-2">
              <label className="flex items-center gap-2 max-sm:flex-auto max-sm:justify-between">地区<NativeSelect size="sm" aria-label="地区" value={region} onChange={event => setRegion(event.target.value)} className="max-w-40"><option value="">全部地区</option>{regions.map(name => <option key={name} value={name}>{name}</option>)}</NativeSelect></label>
              <label className="flex items-center gap-2 max-sm:flex-auto max-sm:justify-between">排序<NativeSelect size="sm" aria-label="排序" value={sort} onChange={event => chooseSort(event.target.value)} className="max-w-40"><option value="default">默认顺序</option><option value="attention">需关注的优先</option><option value="name">按名称</option><option value="cpu">处理器占用优先</option><option value="memory">内存占用优先</option><option value="network">网络速率优先</option></NativeSelect></label>
            </div>
            <div className="ml-auto flex items-center gap-3 max-sm:m-0 max-sm:w-full max-sm:justify-end">
              {changed && <Button variant="link" size="sm" onClick={reset}>清除筛选</Button>}
              <SegmentedGroup aria-label="看板视图">
                <SegmentedButton aria-pressed={view === 'cards'} onClick={() => chooseView('cards')}><LayoutGridIcon aria-hidden="true" />卡片</SegmentedButton>
                <SegmentedButton aria-pressed={view === 'table'} onClick={() => chooseView('table')}><TableIcon aria-hidden="true" />表格</SegmentedButton>
              </SegmentedGroup>
            </div>
          </div>
        </m.div>}
      </AnimatePresence>
    </div>
    {!error && unavailable && servers && <DisplayNotice role="status" onRetry={refresh}>连接中断，等待更新</DisplayNotice>}
    {error && <DisplayNotice tone="error" onRetry={refresh}>{error}</DisplayNotice>}
    {probes.error && <DisplayNotice tone="error" onRetry={probes.reload} retryLabel="重试拨测">拨测读取失败</DisplayNotice>}
    {!servers ? !error && (loading ? <CardsLoading /> : <div className="text-muted-foreground flex min-h-80 flex-col items-center justify-center gap-3 p-6 text-center text-sm" role="status">等待数据</div>) : visible.length ? view === 'cards' ?
      <section aria-label="服务器列表" className="grid grid-cols-[repeat(auto-fill,minmax(320px,1fr))] items-start gap-3 max-sm:grid-cols-1">
        {/* Filtering removes cards at once so results never mix with stale entries; kept cards glide into place. */}
        {visible.map(server => <m.div key={server.id} layout="position" {...itemMotion} className="min-w-0"><ServerCard server={server} unavailable={unavailable} probes={probes.data ? byServer.get(server.id) ?? [] : undefined} probeError={probeUnavailable} probeLoading={probes.loading} now={now} /></m.div>)}
      </section> :
      <ServerTable servers={visible} unavailable={unavailable} byServer={byServer} probesKnown={probes.data !== undefined} probeError={probeUnavailable} probeLoading={probes.loading} now={now} /> :
      !error && <div className="text-muted-foreground flex min-h-80 flex-col items-center justify-center gap-3 p-6 text-center"><ServerIcon className="size-8" aria-hidden="true" /><strong className="text-foreground text-sm font-medium">{entries.length ? '没有匹配的节点' : resource.hiddenOnly ? '节点已隐藏' : '尚未添加服务器'}</strong>{entries.length ? <Button onClick={reset}>清除筛选</Button> : <a className={buttonVariants()} href="#/servers">前往服务器管理</a>}</div>}
  </div>
}

/** First read of the dashboard: card outlines in the same grid as the servers to come. */
function CardsLoading() {
  return <div data-slot="loading" role="status" className="grid grid-cols-[repeat(auto-fill,minmax(320px,1fr))] items-start gap-3 max-sm:grid-cols-1">
    <span className="sr-only">正在读取服务器…</span>
    {[0, 1, 2].map(index => <div key={index} aria-hidden="true" className={cn(surface, 'grid gap-4 p-4')}>
      <div className="flex items-center gap-3"><Skeleton className="size-9 rounded-lg" /><div className="grid flex-1 gap-1.5"><Skeleton className="h-4 w-1/2" /><Skeleton className="h-3 w-1/3" /></div></div>
      <div className="grid grid-cols-2 gap-3">{[0, 1, 2, 3].map(metric => <div key={metric} className="grid gap-1.5"><Skeleton className="h-3 w-1/2" /><Skeleton className="h-1.5" /></div>)}</div>
    </div>)}
  </div>
}
