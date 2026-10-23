import { useState } from 'react'
import { bytes } from '../format'
import { amount, ratio, utcDay } from './data'
import type { TrafficPoint } from './data'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

export default function TrafficChart({ title, points }: { title: string; points: TrafficPoint[] }) {
  const [selected, setSelected] = useState<number | null>(null)
  const maximum = points.reduce((max, point) => { const value = amount(point.total); return value !== null && value > max ? value : max }, 0n)
  const current = points.find(point => point.day === selected) ?? points[points.length - 1]
  const observed = points.some(point => point.total !== null)
  return <Panel className="flex flex-col" aria-label={title}>
    <PanelHeader><PanelTitle>{title}</PanelTitle><span className="text-muted-foreground text-xs">按日汇总 · UTC</span></PanelHeader>
    <div className="flex flex-1 flex-col p-5 max-sm:p-4">
      <div className="text-muted-foreground mb-4 flex flex-wrap items-center gap-4 text-xs max-sm:gap-3"><span className="inline-flex items-center gap-1.5"><i className="bg-chart-1 size-2 rounded-sm" />上传</span><span className="inline-flex items-center gap-1.5"><i className="bg-chart-2 size-2 rounded-sm" />下载</span><small className="ml-auto text-xs">日最高 {observed ? bytes(maximum) : '暂无数据'}</small></div>
      {observed ? <>
        <div data-slot="statistics-chart-bars" className="grid h-[150px] min-h-[150px] flex-1 gap-1 border-b bg-[repeating-linear-gradient(to_top,transparent_0,transparent_49px,var(--border)_49px,var(--border)_50px)] max-sm:gap-0.5" role="group" aria-label={`${title}每日数据`} style={{ gridTemplateColumns: `repeat(${points.length}, minmax(0, 1fr))` }}>
          {points.map(point => <button data-slot="statistics-chart-bar" key={point.day} type="button" className="hover:bg-accent/60 aria-pressed:bg-accent/60 focus-visible:ring-ring/50 relative flex h-full min-w-0 items-end justify-center rounded-t border-0 bg-transparent px-[12%] outline-none focus-visible:ring-2 max-sm:px-[5%]" aria-pressed={current?.day === point.day} data-missing={point.total === null} aria-label={`${utcDay(point.day)}，${point.total === null ? '暂无记录' : `上传 ${bytes(point.uploaded)}，下载 ${bytes(point.downloaded)}`}`} onMouseEnter={() => setSelected(point.day)} onFocus={() => setSelected(point.day)} onClick={() => setSelected(point.day)} onKeyDown={event => {
            if (!['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(event.key)) return
            event.preventDefault()
            const index = points.indexOf(point)
            const next = event.key === 'Home' ? 0 : event.key === 'End' ? points.length - 1 : Math.max(0, Math.min(points.length - 1, index + (event.key === 'ArrowLeft' ? -1 : 1)))
            event.currentTarget.parentElement?.querySelectorAll('button')[next]?.focus()
          }}>
            {point.total === null ? <span className="text-muted-foreground text-base leading-none" aria-hidden="true">·</span> : <span className="bg-chart-1/50 flex min-h-0.5 w-full flex-col overflow-hidden rounded-t-[3px]" style={{ height: `${ratio(point.total, maximum)}%` }} aria-hidden="true"><span className="bg-chart-1 block basis-0" style={{ flexGrow: ratio(point.uploaded, amount(point.total) ?? 0n) }} /><span className="bg-chart-2 block basis-0" style={{ flexGrow: ratio(point.downloaded, amount(point.total) ?? 0n) }} /></span>}
          </button>)}
        </div>
        <div className="text-muted-foreground mt-2 flex justify-between text-xs"><span>{utcDay(points[0].day)}</span><span>{utcDay(points[Math.floor(points.length / 2)].day)}</span><span>{utcDay(points[points.length - 1].day)}</span></div>
        <div data-slot="statistics-chart-selection" className="bg-muted/60 mt-4 flex min-h-12 flex-wrap items-center gap-x-3 gap-y-1 rounded-lg px-3 py-2.5 text-xs" aria-live="polite"><strong className="text-foreground font-medium">{current ? `${utcDay(current.day)}（UTC）` : ''}</strong><span>上传 {bytes(current?.uploaded)} · 下载 {bytes(current?.downloaded)}</span>{current?.incomplete && <small className="text-warning w-full text-xs">这一天包含不完整采样</small>}</div>
      </> : <div className="text-muted-foreground grid min-h-[245px] place-items-center p-6 text-center text-sm">此时间段暂无流量记录，等待数据上报。</div>}
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">悬停、点击或使用左右方向键查看每日数值。空白日期表示暂无记录，当天数据仍在累积。</p>
    </div>
  </Panel>
}
