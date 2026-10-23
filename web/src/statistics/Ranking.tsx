import { bytes } from '../format'
import { amount, ratio } from './data'
import type { TrafficRanking } from './data'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

export default function Ranking({ title, rows, link }: { title: string; rows: TrafficRanking[]; link?: (row: TrafficRanking) => string }) {
  const maximum = rows.reduce((max, row) => { const value = amount(row.total) ?? 0n; return value > max ? value : max }, 0n)
  return <Panel aria-label={title}>
    <PanelHeader><PanelTitle>{title}</PanelTitle><span className="text-muted-foreground text-xs">上传 + 下载 · 前 8</span></PanelHeader>
    {rows.length ? <ol className="m-0 list-none px-5 py-1.5 max-sm:px-4">{rows.map((row, index) => <li className="flex items-start gap-3 py-3 [&+li]:border-t" key={row.id}>
      <span className="bg-muted text-muted-foreground grid size-[22px] shrink-0 place-items-center rounded-md text-xs font-medium">{index + 1}</span>
      <div className="min-w-0 flex-1"><div className="flex items-start justify-between gap-3 text-sm">{link && !row.deleted ? <a className="text-foreground hover:text-primary font-medium wrap-anywhere hover:underline" href={link(row)}>{row.name}</a> : <strong className="font-medium wrap-anywhere">{row.name}</strong>}<b className="text-xs font-medium whitespace-nowrap tabular-nums">{bytes(row.total)}</b></div>
        <small className="text-muted-foreground mt-1 block text-xs wrap-anywhere">{row.deleted ? '已删除 · ' : ''}上传 {bytes(row.uploaded)} · 下载 {bytes(row.downloaded)}{row.incomplete ? ' · 采样不完整' : ''}</small>
        <span className="bg-muted mt-2 block h-[3px] overflow-hidden rounded-full" aria-hidden="true"><span className="bg-chart-1 block h-full" style={{ width: `${ratio(row.total, maximum)}%` }} /></span>
      </div>
    </li>)}</ol> : <div className="text-muted-foreground grid min-h-[245px] place-items-center p-6 text-center text-sm">此时间段暂无流量排行</div>}
  </Panel>
}
