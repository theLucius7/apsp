import { useEffect, useMemo, useState } from 'react'
import { ArrowRightIcon } from 'lucide-react'
import { api, errorMessage } from '../../../api'
import { Badge, DialogPresence, ErrorNotice, Loading, Meter, Modal } from '../../../components'
import { ago, percent, time, uptime } from '../../../format'
import { qualityValue } from '../../../quality'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { SegmentedButton, SegmentedNav } from '@/components/segmented'
import { cn } from '@/lib/utils'
import { asnLabel, carriers, defaultFilter, exitRegion, filterLines, flag, kindBadges, latencyLabel, latencyTone, pointLabel, protocolLabel, rateLabel, regionName, regions, riskFlags, routeLabel, sourceLabels, stateLabels, summary, topologySources, trafficLabels } from './atlas'
import type { LineFilter } from './atlas'
import type { Family, Host, Latency, Line, LinesView, Point, Topology } from './types'

type Tab = '线路' | '探针'
const usageLabels = ['用途类型', '连接类型', 'ASN 类型', 'ASN 用途', '组织类型']
const plainFlags = new Set(['数据中心', '云服务商'])

function Hop({ point, note, first = false }: { point?: Point; note: string; first?: boolean }) {
  return <li className="flex min-w-0 items-center gap-1.5">
    {!first && <ArrowRightIcon className="text-muted-foreground size-3.5 shrink-0" aria-hidden="true" />}
    <span className="bg-background min-w-0 rounded-md border px-2 py-1"><span className="block truncate font-medium">{point ? `${pointLabel(point)}${point.provider ? ` · ${point.provider}` : ''}` : first ? '你的设备' : '入口未知'}</span><span className="text-muted-foreground block text-[11px]">{note}</span></span>
  </li>
}

function Route({ topology }: { topology: Topology }) {
  const entry = topology.points.find(point => point.role === 'entry')
  const exits = topology.edges.map(edge => ({ edge, point: topology.points.find(point => point.id === edge.to) }))
  return <ol data-slot="line-route" aria-label={`线路走向：${routeLabel(topology)}`} className="mt-3 flex flex-wrap items-center gap-1.5 text-xs">
    <Hop first note="客户端" />
    <Hop point={entry} note={topology.kind === 'direct' ? '入口即出口' : '前置入口'} />
    {exits.map(({ edge, point }) => <Hop key={edge.to} point={point} note={topology.kind === 'policy' ? `${trafficLabels[edge.traffic]}出口` : '落地出口'} />)}
  </ol>
}

function CarrierLatency({ latency, compact = false }: { latency: Latency; compact?: boolean }) {
  if (!latency.monitored) return <span className="text-muted-foreground text-xs">暂无延迟监控</span>
  if (compact) return <span className="flex flex-wrap gap-x-3 gap-y-1 text-xs">{carriers.map(({ key, label }) => { const value = latency.carriers?.[key]?.latest; return <span key={key} className="tabular-nums">{label} <span data-tone={latencyTone(value)} className="data-[tone=bad]:text-destructive data-[tone=good]:text-success data-[tone=warm]:text-warning font-medium">{latencyLabel(value)}</span></span> })}</span>
  return <table className="w-full text-xs tabular-nums"><thead><tr className="text-muted-foreground text-left"><th className="py-1 font-normal">运营商</th><th className="py-1 font-normal">最新</th><th className="py-1 font-normal">1 小时平均</th><th className="py-1 font-normal">丢包</th></tr></thead><tbody>{carriers.map(({ key, label }) => { const value = latency.carriers?.[key]; return <tr key={key} className="border-t"><td className="py-1">{label}</td><td className="py-1">{latencyLabel(value?.latest)}</td><td className="py-1">{latencyLabel(value?.average)}</td><td className="py-1">{typeof value?.loss === 'number' ? `${value.loss}%` : '—'}</td></tr> })}</tbody></table>
}

function latencyNote(latency: Latency) {
  if (!latency.monitored) return '该主机没有三网延迟监控。'
  const source = latency.source === 'apsp_probe' ? 'apsp 三网探测' : '原面板广东三网监控'
  return `${source} · ${latency.updated_at ? `更新于 ${ago(latency.updated_at)}` : '更新时间未知'}。数值是主机到国内三网的测量，不等于你的设备实际延迟${latency.referenced ? '；转发与分流线路显示其前置入口的测量值' : ''}。`
}

function LineCard({ line, open }: { line: Line; open: () => void }) {
  const state = stateLabels[line.status.state]
  return <li data-slot="member-line" data-line={line.id} className="bg-muted/30 min-w-0 rounded-xl border p-3.5">
    <div className="flex min-w-0 items-center gap-2"><span className="text-lg leading-none" aria-hidden="true">{flag(exitRegion(line))}</span><strong className="min-w-0 flex-1 truncate" title={line.name}>{line.name}</strong><Badge tone={state.tone}>{state.label}</Badge></div>
    <div className="mt-2 flex flex-wrap gap-1.5">{kindBadges(line.topology).map(badge => <Badge key={badge.label} tone={badge.tone}>{badge.label}</Badge>)}<Badge tone={line.access ? 'good' : 'neutral'}>{line.access ? '当前套餐可用' : '不在当前套餐'}</Badge></div>
    <Route topology={line.topology} />
    <div className="text-muted-foreground mt-3 flex flex-wrap items-center gap-x-4 gap-y-1.5 text-xs"><span>{protocolLabel(line.protocol)}</span><span>倍率 {rateLabel(line.rate)}{line.timed_rate ? '（分时）' : ''}</span><CarrierLatency latency={line.latency} compact /></div>
    <div className="mt-2 flex justify-end"><Button variant="ghost" size="sm" onClick={open}>查看详情</Button></div>
  </li>
}

function LineDetail({ line, host, onClose, probe }: { line: Line; host?: Host; onClose: () => void; probe: (key: string) => void }) {
  const state = stateLabels[line.status.state], topology = line.topology
  return <Modal title={line.name} onClose={onClose} wide slot="member-line-detail">
    <div className="grid gap-5 p-4 text-sm">
      <section><h3 className="mb-1 font-semibold">线路走向</h3><Route topology={topology} />
        <p className="text-muted-foreground mt-2 text-xs leading-relaxed">{topologySources[topology.source]}。位置精确到国家或地区。{topology.kind === 'policy' ? '未命中分流规则的流量从入口直接访问。' : ''}</p>
        {topology.inferred && <Notice className="mt-2 mb-0">{topology.source === 'unresolved' ? '目录没有标注前置入口，入口位置未知。' : '前置入口由目录信息推断，仅供参考。'}</Notice>}
      </section>
      <section><h3 className="mb-1 font-semibold">状态</h3><p className="flex flex-wrap items-center gap-2"><Badge tone={state.tone}>{state.label}</Badge><span className="text-muted-foreground text-xs">{sourceLabels[line.status.source]} · {line.status.checked_at ? `检查于 ${time(line.status.checked_at)}` : '检查时间未知'}</span></p>
        {line.status.state === 'entry_online' && <p className="text-muted-foreground mt-1 text-xs">入口在线；落地出口的可用性不由面板单独确认。</p>}
      </section>
      <section><h3 className="mb-1 font-semibold">延迟</h3><CarrierLatency latency={line.latency} /><p className="text-muted-foreground mt-2 text-xs leading-relaxed">{latencyNote(line.latency)}</p></section>
      <dl className="grid gap-4 sm:grid-cols-2">
        <div><dt className="text-muted-foreground text-xs">协议</dt><dd className="mt-1 font-medium">{protocolLabel(line.protocol)}</dd></div>
        <div><dt className="text-muted-foreground text-xs">流量倍率</dt><dd className="mt-1 font-medium">{rateLabel(line.rate)}{line.timed_rate && <span className="text-muted-foreground ml-1 text-xs">分时倍率，以实际时段为准</span>}</dd></div>
        <div><dt className="text-muted-foreground text-xs">所属套餐</dt><dd className="mt-1 flex flex-wrap gap-1.5">{line.plans.length ? line.plans.map(plan => <Badge key={plan.name} tone={plan.current ? 'good' : 'neutral'}>{plan.name}{plan.current ? '（当前）' : ''}</Badge>) : <span className="text-muted-foreground text-xs">未归属任何套餐</span>}</dd></div>
        {topology.residential && <div><dt className="text-muted-foreground text-xs">宽带类型</dt><dd className="mt-1 font-medium">{topology.dynamic ? '动态家宽' : '住宅宽带'}<span className="text-muted-foreground ml-1 text-xs">{topology.classification_source === 'catalog_tag' ? '由节点目录标签声明' : '由线路名称声明'}，非测量结果</span></dd></div>}
      </dl>
      <section><h3 className="mb-1 font-semibold">网络属性</h3>
        <p className="text-muted-foreground text-xs leading-relaxed">{line.network_scope === 'entry' ? '入口所在主机的 IP 质量、ASN 和用途类型见「探针」。' : '落地出口的网络属性未经面板测量；探针中的数据代表前置入口。'}</p>
        {host && <Button variant="outline" size="sm" className="mt-2" onClick={() => probe(host.key)}>在探针中查看{line.network_scope === 'entry' ? '' : '前置入口'}</Button>}
      </section>
    </div>
  </Modal>
}

function usage(family: Family) {
  const found: { label: string; value: string; source: string }[] = []
  for (const dataset of family.datasets) for (const field of dataset.fields) {
    if (!usageLabels.includes(field.label)) continue
    const value = qualityValue(field, dataset.database)
    if (value && !found.some(item => item.label === field.label && item.value === value)) found.push({ label: field.label, value, source: dataset.source })
  }
  return found
}

function FamilyQuality({ family }: { family: Family }) {
  const asn = asnLabel(family.summary.asn), organization = typeof family.summary.organization === 'string' ? family.summary.organization : undefined
  const country = typeof family.summary.country === 'string' && /^[a-z]{2}$/i.test(family.summary.country) ? family.summary.country : undefined
  return <div data-slot="probe-family" className="mt-3 border-t pt-3">
    <p className="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm"><strong>{family.family}</strong>{asn && <span className="font-medium tabular-nums">{asn}</span>}{organization && <span className="min-w-0 wrap-anywhere">{organization}</span>}{country && <span>{flag(country)} {regionName(country)}</span>}</p>
    {usage(family).length > 0 && <ul className="mt-1.5 flex flex-wrap gap-1.5 text-xs">{usage(family).map(item => <li key={`${item.label}:${item.value}`} className="bg-muted rounded-md px-2 py-0.5" title={item.source}>{item.label}：{item.value}</li>)}</ul>}
    <details className="mt-2"><summary className="text-muted-foreground cursor-pointer text-xs">数据源明细（{family.datasets.length} 个）</summary>
      {family.datasets.map(dataset => <div key={dataset.database} className="mt-2 rounded-lg border p-2.5">
        <p className="flex flex-wrap items-center gap-2 text-xs"><strong>{dataset.source}</strong><span className="text-muted-foreground">{dataset.checked_at ? `查询于 ${time(dataset.checked_at)}` : '查询时间未知'}{dataset.historical ? ' · 历史结果' : ''}</span></p>
        <dl className="mt-1.5 grid gap-y-1 text-xs">{dataset.fields.map(field => <div key={field.label} className="flex justify-between gap-3"><dt className="text-muted-foreground">{field.label}</dt><dd className="m-0 text-right wrap-anywhere">{qualityValue(field, dataset.database) ?? '未知'}</dd></div>)}</dl>
      </div>)}
    </details>
  </div>
}

function HostCard({ host, names, focused }: { host: Host; names: Map<number, string>; focused: boolean }) {
  const known = host.source !== 'unknown', flags = riskFlags(host)
  return <article id={`probe-${host.key}`} data-slot="probe-host" data-focused={focused || undefined} className={cn('min-w-0 scroll-mt-4 rounded-xl border p-3.5', focused && 'ring-primary ring-2')}>
    <header className="flex min-w-0 items-center gap-2"><span className="text-lg leading-none" aria-hidden="true">{flag(host.region)}</span><strong className="min-w-0 flex-1 truncate">{regionName(host.region)}{host.provider ? ` · ${host.provider}` : ''}</strong><Badge tone={!known ? 'neutral' : host.online ? 'good' : 'bad'}>{!known ? '状态未知' : host.online ? '在线' : '离线'}</Badge></header>
    <p className="text-muted-foreground mt-1 text-xs">{sourceLabels[host.source]}{known && ` · ${ago(host.checked_at)}`}{host.uptime_secs != null && ` · 已运行 ${uptime(host.uptime_secs)}`}</p>
    {(host.residential_declared || flags.length > 0) && <div className="mt-2 flex flex-wrap gap-1.5">{host.residential_declared && <Badge tone="warm">家宽（运营方声明）</Badge>}{flags.map(label => <Badge key={label} tone={plainFlags.has(label) ? 'neutral' : 'warm'}>{label}</Badge>)}</div>}
    {known && <div className="mt-3 grid grid-cols-3 gap-3 text-xs">{(['cpu', 'memory', 'disk'] as const).map(key => { const value = host.load[key] ?? undefined; return <div key={key} className="min-w-0"><span className="text-muted-foreground">{{ cpu: 'CPU', memory: '内存', disk: '磁盘' }[key]}</span><span className="ml-1.5 font-medium tabular-nums">{percent(value)}</span><Meter value={value} label={`${{ cpu: 'CPU', memory: '内存', disk: '磁盘' }[key]}占用`} /></div> })}</div>}
    <div className="mt-3"><CarrierLatency latency={host.latency} /><p className="text-muted-foreground mt-1 text-[11px] leading-relaxed">{latencyNote(host.latency)}</p></div>
    <div className="mt-1">{host.network.available ? host.network.families.map(family => <FamilyQuality key={family.family} family={family} />) : <p className="text-muted-foreground mt-3 border-t pt-3 text-xs">{host.network.reason === 'no_agent' ? '该主机尚未接入 apsp 设备，IP 质量暂未测量。' : '暂无 IP 质量数据。'}</p>}</div>
    <p className="text-muted-foreground mt-3 border-t pt-3 text-xs wrap-anywhere">承载线路：{host.lines.map(id => names.get(id)).filter(Boolean).join('、') || '无'}</p>
  </article>
}

function Filters({ lines, filter, setFilter }: { lines: Line[]; filter: LineFilter; setFilter: (filter: LineFilter) => void }) {
  const set = <K extends keyof LineFilter>(key: K, value: LineFilter[K]) => setFilter({ ...filter, [key]: value })
  const select = 'text-muted-foreground grid gap-1.5 text-xs max-sm:min-w-[45%] max-sm:flex-1'
  return <div data-slot="line-filters" className="mt-4 flex flex-wrap items-end gap-3">
    <label className="text-muted-foreground grid min-w-48 flex-1 gap-1.5 text-xs">搜索<Input type="search" aria-label="搜索" value={filter.query} placeholder="线路名称、地区或协议" onChange={event => set('query', event.target.value)} /></label>
    <label className={select}>地区<NativeSelect className="w-full" aria-label="地区" value={filter.region} onChange={event => set('region', event.target.value)}><option value="">全部地区</option>{regions(lines).map(code => <option key={code} value={code}>{flag(code)} {regionName(code)}</option>)}</NativeSelect></label>
    <label className={select}>类型<NativeSelect className="w-full" aria-label="类型" value={filter.kind} onChange={event => set('kind', event.target.value as LineFilter['kind'])}><option value="">全部类型</option><option value="direct">直连入口</option><option value="relay">转发线路</option><option value="policy">智能分流</option><option value="residential">家宽</option></NativeSelect></label>
    <label className={select}>范围<NativeSelect className="w-full" aria-label="范围" value={filter.scope} onChange={event => set('scope', event.target.value as LineFilter['scope'])}><option value="all">全部线路</option><option value="mine">当前套餐可用</option><option value="online">仅在线</option></NativeSelect></label>
    <label className={select}>排序<NativeSelect className="w-full" aria-label="排序" value={filter.sort} onChange={event => set('sort', event.target.value as LineFilter['sort'])}><option value="catalog">目录顺序</option><option value="latency">延迟最低</option><option value="rate">倍率最低</option></NativeSelect></label>
  </div>
}

export default function MemberLines({ account }: { account: string }) {
  const [tab, setTab] = useState<Tab>('线路')
  const [data, setData] = useState<LinesView>()
  const [error, setError] = useState('')
  const [revision, setRevision] = useState(0)
  const [filter, setFilter] = useState(defaultFilter)
  const [detail, setDetail] = useState<number | null>(null)
  const [focus, setFocus] = useState<string | null>(null)
  const reload = () => setRevision(value => value + 1)
  useEffect(() => {
    const controller = new AbortController()
    let active = true, pending = false
    const load = async () => {
      if (pending) return
      pending = true
      try {
        const value = await api<LinesView>(`/api/plugins/sing-box/portal/${account}/lines`, 'GET', undefined, controller.signal, false)
        if (active) { setData(value); setError('') }
      } catch (error) { if (active && !controller.signal.aborted) setError(errorMessage(error)) }
      finally { pending = false }
    }
    void load()
    const timer = window.setInterval(() => { if (document.visibilityState === 'visible') void load() }, 60000)
    return () => { active = false; controller.abort(); window.clearInterval(timer) }
  }, [account, revision])
  useEffect(() => { if (tab === '探针' && focus) document.getElementById(`probe-${focus}`)?.scrollIntoView({ block: 'start' }) }, [tab, focus])
  const shown = useMemo(() => data ? filterLines(data.lines, filter) : [], [data, filter])
  const names = useMemo(() => new Map(data?.lines.map(line => [line.id, line.name])), [data])
  const counts = data ? summary(data.lines) : undefined
  const open = detail === null ? undefined : data?.lines.find(line => line.id === detail)
  // Deployments without an imported catalogue show nothing here.
  if (data && !data.lines.length) return null
  return <Panel data-slot="member-lines" className="mb-5">
    <PanelHeader><PanelTitle>线路与探针</PanelTitle><div className="flex items-center gap-2"><SegmentedNav aria-label="线路视图">{(['线路', '探针'] as const).map(name => <SegmentedButton key={name} aria-pressed={tab === name} onClick={() => { setTab(name); if (name === '线路') setFocus(null) }}>{name}</SegmentedButton>)}</SegmentedNav><Button variant="ghost" size="sm" onClick={reload}>刷新</Button></div></PanelHeader>
    <div className="p-4">
      <ErrorNotice message={error} retry={reload} />
      {!data ? !error && <Loading variant="cards" rows={3} /> : tab === '线路' ? <>
        <p data-slot="line-summary" className="text-muted-foreground text-sm">共 {counts?.total} 条线路，{counts?.online} 条在线；{data.package ? `当前套餐「${data.package}」可用 ${counts?.mine} 条` : '当前没有有效套餐'}。</p>
        <Filters lines={data.lines} filter={filter} setFilter={setFilter} />
        {shown.length ? <ul data-slot="member-line-list" className="mt-4 grid gap-3 lg:grid-cols-2">{shown.map(line => <LineCard key={line.id} line={line} open={() => setDetail(line.id)} />)}</ul>
          : <p className="text-muted-foreground mt-6 text-center text-sm">没有符合条件的线路。<Button variant="link" size="sm" onClick={() => setFilter(defaultFilter)}>清除筛选</Button></p>}
        <p className="text-muted-foreground mt-4 text-xs leading-relaxed">状态每分钟刷新。线路位置只到国家或地区，不显示连接地址；家宽为运营方声明，网络属性以「探针」中的测量为准。</p>
      </> : <>
        <p className="text-muted-foreground text-sm leading-relaxed">每台主机的在线状态、负载、到国内三网的延迟，以及 IP 质量数据源测得的 ASN、网络组织、用途类型和风险标记。不显示 IP 地址、城市和坐标。</p>
        <div data-slot="probe-list" className="mt-4 grid gap-3 lg:grid-cols-2">{data.hosts.map(host => <HostCard key={host.key} host={host} names={names} focused={focus === host.key} />)}</div>
      </>}
    </div>
    <DialogPresence>{open && <LineDetail key={open.id} line={open} host={data?.hosts.find(host => host.key === open.host)} onClose={() => setDetail(null)} probe={key => { setDetail(null); setFocus(key); setTab('探针') }} />}</DialogPresence>
  </Panel>
}
