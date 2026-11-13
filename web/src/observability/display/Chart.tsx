import { useEffect, useId, useMemo, useRef, useState } from 'react'
import type { KeyboardEvent } from 'react'
import { segments } from './data'
import type { Point } from './data'
import { surface } from '@/components/board'
import { cn } from '@/lib/utils'

export type Series = { label: string; color: string; points: Point[] }
const height = 210, right = 12, top = 12, bottom = 30
// Axis labels are 10px: about 6px per Latin character and 10px per CJK one.
const labelWidth = (text: string) => [...text].reduce((total, char) => total + (char.charCodeAt(0) > 0x2e80 ? 10 : 6), 0)
const clock = (value: number) => new Date(value).toLocaleTimeString('zh-CN', { hour12: false, hour: '2-digit', minute: '2-digit', second: '2-digit' })
const date = (value: number) => new Date(value).toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour12: false, hour: '2-digit', minute: '2-digit' })

/**
 * A monotone cubic curve through the points (Fritsch–Carlson). It passes through
 * every sample and never overshoots between two of them, so a smoother line shows
 * no peak or dip that was not measured.
 */
export function smoothPath(points: readonly (readonly [number, number])[]): string {
  const count = points.length
  if (count === 0) return ''
  const format = (value: number) => value.toFixed(2)
  if (count < 3) return points.map(([px, py], index) => `${index ? 'L' : 'M'}${format(px)},${format(py)}`).join(' ')
  const slopes: number[] = [], tangents: number[] = []
  for (let index = 0; index < count - 1; index++) {
    const dx = points[index + 1][0] - points[index][0]
    slopes.push(dx === 0 ? 0 : (points[index + 1][1] - points[index][1]) / dx)
  }
  tangents.push(slopes[0])
  for (let index = 1; index < count - 1; index++) tangents.push(slopes[index - 1] * slopes[index] <= 0 ? 0 : (slopes[index - 1] + slopes[index]) / 2)
  tangents.push(slopes[count - 2])
  for (let index = 0; index < count - 1; index++) {
    if (slopes[index] === 0) { tangents[index] = 0; tangents[index + 1] = 0; continue }
    const a = tangents[index] / slopes[index], b = tangents[index + 1] / slopes[index], length = a * a + b * b
    if (length > 9) { const scale = 3 / Math.sqrt(length); tangents[index] = scale * a * slopes[index]; tangents[index + 1] = scale * b * slopes[index] }
  }
  let path = `M${format(points[0][0])},${format(points[0][1])}`
  for (let index = 0; index < count - 1; index++) {
    const [x0, y0] = points[index], [x1, y1] = points[index + 1], third = (x1 - x0) / 3
    path += ` C${format(x0 + third)},${format(y0 + tangents[index] * third)} ${format(x1 - third)},${format(y1 - tangents[index + 1] * third)} ${format(x1)},${format(y1)}`
  }
  return path
}

export default function Chart({ title, series, from, to, gap, maximum, format }: {
  title: string; series: Series[]; from: number; to: number; gap: number; maximum?: number; format: (value: number | null) => string
}) {
  const id = useId().replace(/:/g, '')
  const host = useRef<HTMLElement>(null)
  const [width, setWidth] = useState(600)
  const [cursor, setCursor] = useState<number | null>(null)
  const [hidden, setHidden] = useState<string[]>([])
  useEffect(() => {
    const observer = new ResizeObserver(entries => {
      const size = entries[0]?.contentRect.width
      if (size) setWidth(Math.max(200, size))
    })
    if (host.current) observer.observe(host.current)
    return () => observer.disconnect()
  }, [])
  const plots = useMemo(() => series.map(item => ({ ...item, segments: segments(item.points, from, to, gap) })), [series, from, to, gap])
  const visible = useMemo(() => plots.filter(item => !hidden.includes(item.label)), [plots, hidden])
  const max = Math.max(maximum ?? 0, 1, ...visible.flatMap(item => item.segments.flatMap(part => part.map(point => point.range?.max ?? point.value!))))
  const ticks = [0, .25, .5, .75, 1].map(tick => ({ tick, label: format(tick * max) }))
  const left = Math.min(110, Math.max(40, 12 + Math.max(...ticks.map(entry => labelWidth(entry.label)))))
  const y = (value: number) => top + (1 - value / max) * (height - top - bottom)
  const x = (at: number) => left + (at - from) / Math.max(1, to - from) * (width - left - right)
  const times = useMemo(() => [...new Set(visible.flatMap(item => item.segments.flatMap(part => part.map(point => point.at))))].sort((a, b) => a - b), [visible])
  const selected = cursor === null || !times.length ? null : times.reduce((nearest, at) => Math.abs(at - cursor) < Math.abs(nearest - cursor) ? at : nearest)
  const pointAt = (item: typeof plots[number]) => {
    const points = item.segments.flat()
    if (!points.length || selected === null) return null
    const point = points.reduce((best, point) => Math.abs(point.at - selected) < Math.abs(best.at - selected) ? point : best)
    return Math.abs(point.at - selected) <= (point.range ? gap / 2 : Math.min(gap / 2, 30_000)) ? point : null
  }
  const aggregated = visible.some(item => item.points.some(point => point.range))
  const axisTime = to - from >= 86_400_000 ? date : clock
  const onKey = (event: KeyboardEvent<SVGSVGElement>) => {
    if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key) || !times.length) return
    event.preventDefault()
    const index = selected === null ? times.length - 1 : times.indexOf(selected)
    setCursor(times[event.key === 'Home' ? 0 : event.key === 'End' ? times.length - 1 : Math.max(0, Math.min(times.length - 1, index + (event.key === 'ArrowLeft' ? -1 : 1)))])
  }
  // At rest the legend shows each curve's latest reading; under the cursor, the reading there.
  const latest = (item: typeof plots[number]) => item.segments[item.segments.length - 1]?.at(-1)?.value ?? null
  const position = selected === null ? 0 : (x(selected) / width) * 100
  // On a phone-width chart the reading spans the plot instead of following the cursor.
  const narrow = width < 520
  return <section data-slot="chart" className={cn(surface, 'min-w-0 overflow-hidden p-4 max-sm:p-3')} aria-label={`${title}图表`} ref={host}>
    <header className="flex items-center justify-between gap-2"><h3 className="text-sm font-semibold">{title}</h3><span className="text-muted-foreground text-xs tabular-nums">{selected === null ? aggregated ? '均值与范围' : '历史采样' : axisTime(selected)}</span></header>
    <div data-slot="chart-legend" className="flex min-h-8 flex-wrap items-center gap-x-3 gap-y-1" role="group" aria-label={`${title}曲线`}>
      {plots.map(item => <button key={item.label} className="text-muted-foreground hover:text-foreground focus-visible:ring-ring/50 inline-flex items-center gap-1.5 rounded-sm py-1 text-xs outline-none focus-visible:ring-2 aria-[pressed=false]:line-through aria-[pressed=false]:opacity-50" aria-pressed={!hidden.includes(item.label)} onClick={() => setHidden(value => value.includes(item.label) ? value.filter(label => label !== item.label) : [...value, item.label])}><i className="size-2 rounded-full" style={{ background: item.color }} />{item.label}<b className="text-foreground font-medium tabular-nums">{format(selected === null ? latest(item) : pointAt(item)?.value ?? null)}</b></button>)}
    </div>
    {!times.length ? <div className="text-muted-foreground flex min-h-40 items-center justify-center p-5 text-xs">{visible.length ? '此时间段暂无采样数据' : '已隐藏全部曲线'}</div> : <div className="relative">
    <svg className="block h-auto min-h-40 w-full touch-pan-y rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring/50 max-sm:min-h-36 [&_text]:fill-muted-foreground [&_text]:text-[10px]" viewBox={`0 0 ${width} ${height}`} role="img" aria-label={`${title}历史曲线，左右方向键查看采样，首尾键跳转`} tabIndex={0} onKeyDown={onKey} onBlur={() => setCursor(null)} onPointerLeave={() => setCursor(null)} onPointerMove={event => {
      const bounds = event.currentTarget.getBoundingClientRect()
      const position = (event.clientX - bounds.left) / bounds.width * width
      setCursor(from + Math.max(0, Math.min(1, (position - left) / (width - left - right))) * (to - from))
    }}>
      <defs>{visible.map((item, index) => <linearGradient key={item.label} id={`${id}-${index}`} x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stopColor={item.color} stopOpacity=".24" /><stop offset="100%" stopColor={item.color} stopOpacity=".02" /></linearGradient>)}</defs>
      {ticks.map(({ tick, label }) => <g key={tick}><line x1={left} x2={width - right} y1={y(tick * max)} y2={y(tick * max)} className="stroke-border" strokeDasharray="3 3" /><text x={left - 8} y={y(tick * max) + 4} textAnchor="end">{label}</text></g>)}
      {[0, .5, 1].map(tick => <text key={tick} x={x(from + (to - from) * tick)} y={height - 8} textAnchor={tick === 0 ? 'start' : tick === 1 ? 'end' : 'middle'}>{axisTime(from + (to - from) * tick)}</text>)}
      {visible.map((item, index) => <g key={item.label}>{item.segments.map((part, partIndex) => {
        const path = smoothPath(part.map(point => [x(point.at), y(point.value!)] as const))
        const range = part.every(point => point.range)
        const band = range ? `${part.map((point, i) => `${i ? 'L' : 'M'}${x(point.at)},${y(point.range!.max)}`).join(' ')} ${[...part].reverse().map(point => `L${x(point.at)},${y(point.range!.min)}`).join(' ')} Z` : ''
        return <g key={partIndex}>{part.length === 1 ? <>{range && <line x1={x(part[0].at)} x2={x(part[0].at)} y1={y(part[0].range!.min)} y2={y(part[0].range!.max)} stroke={item.color} strokeWidth="5" opacity=".25" />}<circle cx={x(part[0].at)} cy={y(part[0].value!)} r="2.5" fill={item.color} /></> : <><path d={range ? band : `${path} L${x(part[part.length - 1].at)},${y(0)} L${x(part[0].at)},${y(0)} Z`} fill={range ? item.color : `url(#${id}-${index})`} opacity={range ? '.18' : undefined} data-range={range || undefined} /><path d={path} fill="none" stroke={item.color} strokeWidth="1.8" vectorEffect="non-scaling-stroke" /></>}{part.filter(point => point.range?.live).map(point => <circle key={point.at} cx={x(point.at)} cy={y(point.value!)} r="3" fill={item.color} data-live="true" />)}</g>
      })}</g>)}
      {selected !== null && <line x1={x(selected)} x2={x(selected)} y1={top} y2={height - bottom} className="stroke-muted-foreground opacity-70" strokeDasharray="3 3" />}
      {selected !== null && visible.map(item => { const point = pointAt(item); return point && <circle key={item.label} cx={x(point.at)} cy={y(point.value!)} r="3.5" fill={item.color} className="stroke-card" strokeWidth="1.5" /> })}
    </svg>
    {/* The reading under the cursor floats beside it instead of reserving space below every chart. */}
    {selected !== null && aggregated && <div data-slot="chart-details" aria-live="polite" style={narrow ? undefined : { left: `${position}%` }}
      className={cn('bg-popover text-popover-foreground ring-foreground/10 pointer-events-none absolute top-1 z-10 grid gap-1.5 rounded-md px-2.5 py-2 text-xs shadow-md ring-1 wrap-anywhere [&_strong]:font-medium [&_span]:text-muted-foreground [&_small]:text-muted-foreground',
        narrow ? 'inset-x-1' : cn('w-max max-w-72', position > 55 ? '-translate-x-[calc(100%+0.75rem)]' : 'translate-x-3'))}>
      {visible.map(item => {
        const point = pointAt(item), range = point?.range
        return <div key={item.label} className="grid gap-0.5">{range ? <><strong>{item.label} · {range.live ? '实时采样' : `平均 ${format(point!.value)}`}</strong><span>{range.live ? `采样 ${axisTime(range.from)}` : `最小 ${format(range.min)} · 最大 ${format(range.max)} · ${range.count} / ${range.samples} 个指标采样`}</span>{!range.live && <small>采样范围 {axisTime(range.from)} — {axisTime(range.to)}{range.bucketFrom !== undefined && range.bucketTo !== undefined && <> · 聚合窗口 {axisTime(range.bucketFrom)} — {axisTime(range.bucketTo)}</>}{range.partial && ' · 部分历史或边界窗口'}</small>}</> : <span>{item.label} · 此处无采样</span>}</div>
      })}
    </div>}
    </div>}
  </section>
}
