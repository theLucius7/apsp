import { time } from '../format'
import { authorizationState, familyLabel, latency, loss, lossLabel, networkLabel, probeSlots, probeState, probeTone, probeValue } from '../probes'
import type { ProbeField, ProbeOverview, ProbeResult } from '../probes'
import { qualityFill, qualityText } from './ui'
import { cn } from '@/lib/utils'

const panelClass = 'bg-muted/50 min-w-0 rounded-lg px-2 py-1.5'
const valueClass = 'text-xs font-semibold whitespace-nowrap tabular-nums'
const barsClass = 'mt-1.5 grid grid-cols-[repeat(20,minmax(0,1fr))] gap-0.5'
const bar = (tone: string) => cn('h-2.5 rounded-[1px]', qualityFill[tone])

function Quality({ label, field, latest, slots, live, compact }: { label: string; field: ProbeField; latest?: ProbeResult; slots: (ProbeResult | undefined)[]; live: boolean; compact?: boolean }) {
  const format = field === 'latency_ms' ? latency : loss
  const current = live ? probeValue(latest, field) : null
  return <div data-slot="quality-panel" className={cn(panelClass, compact && 'bg-transparent p-0')}><div className="text-muted-foreground flex items-center justify-between gap-1 text-xs"><span>{label}</span><strong data-slot="quality-value" className={cn(valueClass, qualityText[probeTone(current, field)])}>{format(current)}</strong></div>{!compact && <div className={barsClass} aria-label={`${label}近期采样，空缺为灰色`}>{slots.map((point, index) => {
    const value = probeValue(point, field)
    const title = point ? `${time(point.sampled_at / 1000)} · ${point.error != null && point.attempts !== 4 ? '检测不可用' : `${label} ${format(value)}${point.error ? ` · ${point.error}` : ''}`}` : '此时段无采样'
    return <span key={index} data-slot="quality-bar" className={bar(probeTone(value, field))} title={title} />
  })}</div>}</div>
}

export default function ProbeQuality({ probes, now, unavailable, loading, online, compact = false }: { probes?: ProbeOverview[]; now: number; unavailable: boolean; loading: boolean; online: boolean; compact?: boolean }) {
  const enabled = probes?.filter(item => item.probe.enabled).sort((a, b) => a.probe.name.localeCompare(b.probe.name, 'zh-CN')) ?? []
  if (!enabled.length) return <div className="bg-muted/50 text-muted-foreground flex min-w-0 justify-between gap-2 rounded-lg px-2 py-2.5 text-xs"><span>延迟 / 丢包</span><span>{unavailable ? '拨测读取失败' : loading ? '正在读取…' : probes?.length ? probes.some(item => authorizationState(item.probe, now)) ? '目标未授权或已到期' : '拨测已暂停' : '尚未配置拨测'}</span></div>
  const rows = enabled.slice(0, compact ? 1 : 3).map(({ probe, results }) => {
    const latest = results.filter(point => point.sampled_at <= now).reduce<ProbeResult | undefined>((last, point) => !last || point.sampled_at > last.sampled_at ? point : last, undefined)
    const state = probeState(probe, latest, now, unavailable || !online)
    const label = [probe.monitor?.network && probe.monitor.network !== 'other' ? `${networkLabel(probe)} · ${probe.monitor.region}` : '', probe.carrier, probe.name].filter(Boolean).join(' · ')
    const title = `${label} · ${probe.target}${probe.port ? `:${probe.port}` : ''} · ${familyLabel(probe, latest)} · ${state}`
    return { probe, latest, state, label, title, slots: probeSlots(results, probe, now) }
  })
  if (compact) return <div className="grid gap-1">{rows.map(({ probe, latest, state, label, title, slots }) => <div key={probe.id}>
    <div className="text-muted-foreground mb-1.5 flex min-w-0 justify-between gap-2 text-xs"><span className="truncate" title={title}>{label}</span><small className="shrink-0 text-xs">{state === '最近采样' ? probe.kind === 'icmp' ? 'ICMP' : 'TCP' : state}</small></div>
    <div className="grid grid-cols-2 gap-2.5"><Quality label="延迟" field="latency_ms" compact latest={latest} slots={slots} live={state === '最近采样'} /><Quality label={lossLabel(probe)} field="loss_percent" compact latest={latest} slots={slots} live={state === '最近采样'} /></div>
  </div>)}</div>
  const lossHeading = rows.every(row => row.probe.kind === 'icmp') ? '丢包' : rows.every(row => row.probe.kind === 'tcp') ? '连接失败率' : '丢包 / 连接失败率'
  return <div className="grid gap-2">
    <div className="grid grid-cols-2 gap-1.5">{(['latency_ms', 'loss_percent'] as const).map(field => <div data-slot="quality-panel" className={cn(panelClass, 'flex flex-col gap-2')} key={field}>
      <div className="text-muted-foreground text-xs leading-none">{field === 'latency_ms' ? '延迟' : lossHeading}</div>
      {rows.map(({ probe, latest, state, label, title, slots }) => {
        const format = field === 'latency_ms' ? latency : loss
        const value = state === '最近采样' ? probeValue(latest, field) : null
        return <div className="min-w-0" key={probe.id}>
          <div className="text-muted-foreground flex items-center gap-1.5 text-xs" title={`${title} · ${field === 'latency_ms' ? '延迟' : lossLabel(probe)}`}><span className="min-w-0 flex-1 truncate">{label}</span><strong data-slot="quality-value" className={cn(valueClass, 'font-medium', qualityText[probeTone(value, field)])}>{format(value)}</strong></div>
          <div className={cn(barsClass, 'mt-1 gap-px')} aria-label={`${label}近期${field === 'latency_ms' ? '延迟' : lossLabel(probe)}`}>{slots.map((point, index) => {
            const value = probeValue(point, field)
            const reading = point ? `${time(point.sampled_at / 1000)} · ${point.error != null && point.attempts !== 4 ? '检测不可用' : `${format(value)}${point.error ? ` · ${point.error}` : ''}`}` : '此时段无采样'
            return <span key={index} data-slot="quality-bar" className={bar(probeTone(value, field))} title={reading} />
          })}</div>
          {state !== '最近采样' && <small className="text-muted-foreground mt-1 block text-xs">{state}</small>}
        </div>
      })}
    </div>)}</div>
    {enabled.length > 3 && <small className="text-muted-foreground text-xs">另有 {enabled.length - 3} 个目标</small>}
  </div>
}
