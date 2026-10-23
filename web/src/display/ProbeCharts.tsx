import { useState } from 'react'
import { time } from '../format'
import { useDashboardPoll } from './useDashboardPoll'
import { familyLabel, latency, loss, lossLabel, networkLabel, probeState, probeValue } from '../probes'
import type { Probe, ProbeResult } from '../probes'
import Chart from './Chart'
import { NetworkIcon } from 'lucide-react'
import { DisplayNotice, SectionHeading, surface } from './ui'
import { cn } from '@/lib/utils'
import { buttonVariants } from '@/components/ui/button'
import { NativeSelect } from '@/components/ui/native-select'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'

function History({ id, probe, now, minutes, unavailable }: { id: number; probe: Probe; now: number; minutes: number; unavailable: boolean }) {
  const results = useDashboardPoll<ProbeResult[]>(`/api/dashboard/servers/${id}/probe-results?probe_id=${encodeURIComponent(probe.id)}&hours=${minutes / 60}`, 15_000, false)
  const points = (results.data ?? []).filter(result => result.probe_id === probe.id && result.sampled_at >= now - minutes * 60_000 && result.sampled_at <= now).sort((a, b) => a.sampled_at - b.sampled_at)
  const latest = points[points.length - 1]
  const state = probeState(probe, latest, now, unavailable || Boolean(results.error))
  const props = { from: now - minutes * 60_000, to: now, gap: Math.max(30_000, probe.interval_secs * 3000) }
  return <>
    {results.error && <DisplayNotice tone="error" onRetry={results.reload}>{results.denied ? '拨测结果读取权限不可用，已清除历史数据。' : '拨测结果读取失败，已显示的曲线为历史数据。'}</DisplayNotice>}
    {results.loading && !results.data && <p className="text-muted-foreground text-xs" role="status">正在读取拨测结果…</p>}
    <div data-slot="probe-summary" className="text-muted-foreground flex flex-wrap items-center justify-between gap-x-3 gap-y-1.5 px-1 py-2.5 text-xs wrap-anywhere [&_strong]:text-foreground [&_strong]:font-medium"><span>{networkLabel(probe)} · {probe.monitor?.region || '地区未配置'} · {familyLabel(probe, latest)} · {probe.carrier && `${probe.carrier} · `}{probe.kind === 'tcp' ? 'TCP 连接' : 'ICMP 回显'} · {probe.target}{probe.port ? `:${probe.port}` : ''}</span><span>{state}{latest && ` · ${time(latest.sampled_at / 1000)}`}</span><strong>{latency(state === '最近采样' ? probeValue(latest, 'latency_ms') : null)} · {lossLabel(probe)} {loss(state === '最近采样' ? probeValue(latest, 'loss_percent') : null)}</strong></div>
    {latest?.error != null && <DisplayNotice>{latest.attempts === 4 ? `已完成 4 次尝试，存在失败：${latest.error}。失败比例按实际尝试记录。` : `最近一次检测不可用：${latest.error}。该次结果以空缺显示。`}</DisplayNotice>}
    <div className="grid gap-3 md:grid-cols-2">
      <Chart {...props} title={`${probe.name} · 延迟`} format={latency} series={[{ label: '往返延迟', color: 'var(--info)', points: points.map(point => ({ at: point.sampled_at, value: probeValue(point, 'latency_ms') })) }]} />
      <Chart {...props} title={`${probe.name} · ${lossLabel(probe)}`} maximum={100} format={loss} series={[{ label: lossLabel(probe), color: 'var(--warning)', points: points.map(point => ({ at: point.sampled_at, value: probeValue(point, 'loss_percent') })) }]} />
    </div>
  </>
}

export default function ProbeCharts({ id, now, unavailable, publicView }: { id: number; now: number; unavailable: boolean; publicView: boolean }) {
  const definitions = useDashboardPoll<Probe[]>(`/api/dashboard/servers/${id}/probes`, 30_000, false)
  const [minutes, setMinutes] = useState(60)
  const [selected, setSelected] = useState('')
  const probes = definitions.data ?? []
  const probe = probes.find(item => item.id === selected) ?? probes.find(item => item.enabled) ?? probes[0]
  const empty = 'text-muted-foreground flex min-h-44 flex-col items-center justify-center gap-3 p-5 text-center text-xs'
  return <section data-slot="probes" className="flex flex-col gap-3">
    <SectionHeading icon={NetworkIcon} title="延迟与丢包"><SegmentedGroup aria-label="拨测时间范围" className="max-sm:*:flex-1">{[[60, '1 小时'], [360, '6 小时'], [1440, '24 小时']].map(([value, label]) => <SegmentedButton key={value} aria-pressed={minutes === value} onClick={() => setMinutes(Number(value))}>{label}</SegmentedButton>)}</SegmentedGroup></SectionHeading>
    {definitions.error && <DisplayNotice tone="error" onRetry={definitions.reload}>拨测配置读取失败。</DisplayNotice>}
    {definitions.loading && !definitions.data ? <div className={empty} role="status">正在读取拨测配置…</div> : definitions.error && !definitions.data ? <div className={cn(surface, empty)}>暂时无法读取拨测配置</div> : !probe ? <div className={cn(surface, empty)}>尚无已配置的拨测项目{!publicView && <a className={buttonVariants({ size: 'sm' })} href={`#/servers/${id}`}>前往后台配置拨测</a>}</div> : <>
      <label className="text-muted-foreground flex min-w-0 items-center gap-3 text-sm">拨测目标<NativeSelect aria-label="拨测目标" className="w-[min(400px,75%)]" value={probe.id} onChange={event => setSelected(event.target.value)}>{probes.map(item => <option key={item.id} value={item.id}>{networkLabel(item)} · {item.monitor?.region || '地区未配置'} · {item.carrier ? `${item.carrier} · ` : ''}{item.name}{item.enabled ? '' : '（已暂停）'}</option>)}</NativeSelect></label>
      <History key={`${probe.id}/${minutes}`} id={id} probe={probe} now={now} minutes={minutes} unavailable={unavailable || Boolean(definitions.error)} />
      <p className="text-muted-foreground my-1 text-xs leading-relaxed">由服务器向配置的目标发起检测，每轮 4 次。ICMP 显示回显丢包率，TCP 显示连接失败率；延迟为成功响应的平均值。无响应为 100%，检测失败或缺少采样保留空缺。</p>
    </>}
  </section>
}
