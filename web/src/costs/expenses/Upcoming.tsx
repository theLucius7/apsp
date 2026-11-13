import { useState } from 'react'
import { CalendarCheckIcon, DownloadIcon } from 'lucide-react'
import { ErrorNotice } from '../../components'
import { convert, money } from '../currency/currency'
import { toneText } from '@/components/board'
import { useAction, useResource } from '../../hooks'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { addDays, monthGroups, shortDate, weekday } from './model'
import type { Due, LedgerRow } from './model'
import { amount, download, root } from './types'
import type { Expense } from './types'
import type { Ledger } from './useLedger'

const ranges = [[30, '30 天'], [90, '90 天'], [365, '12 个月']] as const
type Range = typeof ranges[number][0]
const savedRange = (): Range => { try { const value = Number(localStorage.getItem('apsp-expenses-range')); return value === 90 || value === 365 ? value : 30 } catch { return 30 } }
const kindText = { item: '服务', server: '服务器', reference: '机场订阅' } as const
const validDues = (value: unknown): value is { entries: Due[] } => typeof value === 'object' && value !== null && Array.isArray((value as { entries?: unknown }).entries)

/** One renewal on the timeline; items and servers open their detail, a source opens its page. */
function DueRow({ due, base, ledger, onOpen }: { due: Due; base?: string; ledger: Ledger; onOpen: (row: LedgerRow) => void }) {
  const row = due.kind === 'reference' ? undefined : ledger.rows.find(entry => entry.kind === due.kind && entry.id === due.id)
  const value = due.amount_units === null ? null : amount(due.amount_units, due.currency)
  const converted = due.amount_units !== null && base && due.currency !== base ? convert(Number(due.amount_units) / 10000, due.currency, base, ledger.rates) : null
  const mode = due.kind === 'reference' ? '服务商报告的到期日' : due.kind === 'server' ? (due.auto_renew ? '到期自动顺延' : '到期日') : due.auto_renew ? '自动扣款' : '需要手动续费'
  const name = <span className="grid min-w-0"><strong className="truncate text-[13px] font-medium group-hover/due:underline">{due.name}</strong><small className="text-muted-foreground text-xs">{kindText[due.kind]} · {mode}</small></span>
  return <li data-slot="due-row" data-kind={due.kind} className="hover:bg-muted/40 grid grid-cols-[4.5rem_minmax(0,1fr)_auto] items-center gap-x-4 px-4 py-2.5 max-sm:px-3">
    <span className="grid text-[13px] tabular-nums"><span>{shortDate(due.date)}</span><small className="text-muted-foreground text-xs">{weekday(due.date)}</small></span>
    {due.kind === 'reference' ? <a href={`#${due.path ?? ''}`} className="group/due min-w-0 outline-none focus-visible:underline">{name}</a>
      : <button type="button" className="group/due min-w-0 text-left outline-none focus-visible:underline" disabled={!row} onClick={() => row && onOpen(row)}>{name}</button>}
    <span className="grid text-right text-[13px] tabular-nums">{value ?? <span className="text-muted-foreground">—</span>}{converted !== null && <small className="text-muted-foreground text-xs" title="按账本汇率折算">约 {money(converted, base!)}</small>}</span>
  </li>
}

/** What is coming: past-due renewals first, then every renewal in the chosen window by month. */
export default function Upcoming({ ledger, onOpen, onOperate }: { ledger: Ledger; onOpen: (row: LedgerRow) => void; onOperate: (item: Expense, kind: 'renew') => void }) {
  const [range, setRange] = useState<Range>(savedRange)
  const to = addDays(ledger.today, range)
  const query = `?from=${ledger.today}&to=${to}`
  const calendar = useResource<unknown>(`${root}/calendar${query}`, 60_000)
  const exporting = useAction()
  // Past dates are the overdue list above; the timeline starts today.
  const dues = validDues(calendar.data) ? calendar.data.entries.filter(due => due.date >= ledger.today) : undefined
  const groups = monthGroups(dues ?? [], ledger.base, ledger.rates)
  const overdue = ledger.rows.filter(row => !row.inactive && row.due.days !== null && row.due.days < 0)
  const total = groups.reduce((sum, group) => sum + group.total, 0), unconverted = groups.reduce((sum, group) => sum + group.unconverted, 0)
  const choose = (value: Range) => { setRange(value); try { localStorage.setItem('apsp-expenses-range', String(value)) } catch { /* The choice stays for this visit. */ } }
  return <section aria-label="即将续费" className="grid min-w-0 gap-3">
    <div className="flex min-h-9 flex-wrap items-center gap-3">
      <SegmentedGroup aria-label="时间范围">{ranges.map(([value, label]) => <SegmentedButton key={value} aria-pressed={range === value} onClick={() => choose(value)}>{label}</SegmentedButton>)}</SegmentedGroup>
      <span className="text-muted-foreground text-xs tabular-nums">{dues ? `${dues.length} 笔${ledger.base ? ` · 约 ${money(total, ledger.base)}` : ''}${unconverted ? `（${unconverted} 笔缺少汇率未计入）` : ''}` : '正在读取…'}</span>
      <Button variant="outline" size="sm" className="ml-auto" disabled={exporting.busy || !dues} onClick={() => void exporting.run(() => download(`${root}/calendar.ics${query}`, 'apsp-renewals.ics'))}><DownloadIcon aria-hidden="true" />导出日历</Button>
    </div>
    <ErrorNotice message={calendar.error || exporting.error} retry={calendar.reload} />
    {overdue.length > 0 && <div data-slot="overdue" className="bg-card shadow-soft min-w-0 overflow-hidden rounded-xl border border-destructive/30">
      <h3 className="text-destructive border-b px-4 py-2.5 text-sm font-semibold">已过续费日 · {overdue.length}</h3>
      <ul className="divide-y">{overdue.map(row => <li key={row.key} className="grid grid-cols-[4.5rem_minmax(0,1fr)_auto] items-center gap-x-4 px-4 py-2.5 max-sm:px-3">
        <span className={cn('grid text-[13px] tabular-nums', toneText.danger)}><span>{shortDate(row.next)}</span><small className="text-xs">{row.due.next.label}</small></span>
        <button type="button" className="min-w-0 text-left outline-none hover:underline focus-visible:underline" onClick={() => onOpen(row)}><strong className="block truncate text-[13px] font-medium">{row.name}</strong><small className="text-muted-foreground block text-xs">{row.due.reasons[0]}</small></button>
        {row.kind === 'item' && !row.auto ? <Button size="sm" disabled={ledger.stale} onClick={() => onOperate(row.item, 'renew')}>确认续期</Button>
          : <Button variant="outline" size="sm" onClick={() => onOpen(row)}>{row.kind === 'server' ? '登记续费' : '查看'}</Button>}
      </li>)}</ul>
    </div>}
    {dues && !groups.length && !overdue.length && <div className="text-muted-foreground grid justify-items-center gap-2 py-10 text-center text-sm"><CalendarCheckIcon className="size-8" aria-hidden="true" />未来 {ranges.find(([value]) => value === range)![1]}没有续费</div>}
    {groups.map(group => <div key={group.month} data-slot="month" className="bg-card text-card-foreground shadow-soft min-w-0 overflow-hidden rounded-xl border">
      <h3 className="flex items-baseline justify-between gap-3 border-b px-4 py-2.5 text-sm font-semibold max-sm:px-3"><span>{group.label}</span><span className="text-muted-foreground text-xs font-normal tabular-nums">{group.count} 笔{ledger.base ? ` · 约 ${money(group.total, ledger.base)}` : ''}</span></h3>
      <ul className="divide-y">{group.entries.map((due, index) => <DueRow key={`${due.kind}-${due.id}-${due.date}-${index}`} due={due} base={ledger.base} ledger={ledger} onOpen={onOpen} />)}</ul>
    </div>)}
  </section>
}
