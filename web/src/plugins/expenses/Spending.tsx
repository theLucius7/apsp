import { useState } from 'react'
import { ErrorNotice } from '../../components'
import { money } from '../../display/currency'
import { Track, toneText } from '../../display/ui'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { monthlyByCategory, paymentYears, recordKinds, signedUnits } from './model'
import type { LedgerRow, Payment } from './model'
import { amount, minorToUnits } from './types'
import type { Ledger } from './useLedger'

const PAGE = 20
const paymentAmount = (payment: Payment) => payment.kind === 'item' ? amount(payment.amount_units, payment.currency) : amount(minorToUnits(payment.amount_minor, payment.currency), payment.currency)
const paymentNote = (payment: Payment) => payment.kind === 'item' ? `覆盖 ${payment.period_start} 起的一期`
  : [payment.reference, payment.valid_until ? `有效期至 ${new Date(payment.valid_until * 1000).toISOString().slice(0, 10)}` : ''].filter(Boolean).join(' · ')

/** How much: the monthly run rate by category against the budget, then what was actually paid each year. */
export default function Spending({ ledger, onOpen }: { ledger: Ledger; onOpen: (row: LedgerRow) => void }) {
  const base = ledger.base
  const categories = monthlyByCategory(ledger.rows)
  const years = paymentYears(ledger.payments ?? [], base, ledger.rates)
  const [chosen, setChosen] = useState<string>()
  const [all, setAll] = useState(false)
  const year = years.find(entry => entry.year === chosen) ?? years[0]
  const budget = ledger.budget, monthly = ledger.totals.monthly
  const used = budget ? monthly / budget * 100 : null
  // A payment of an item deleted since stays in the history but has nothing to open.
  const rowOf = (payment: Payment) => ledger.rows.find(entry => payment.kind === 'item' ? entry.kind === 'item' && entry.id === payment.subscription_id : entry.kind === 'server' && entry.id === payment.server_id)
  const shown = year ? all ? year.payments : year.payments.slice(0, PAGE) : []
  return <section aria-label="支出" className="grid min-w-0 gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.25fr)] lg:items-start">
    <Panel className="mb-0">
      <PanelHeader className="min-h-11 py-2"><PanelTitle>每月费用构成</PanelTitle><span className="text-muted-foreground text-xs tabular-nums">{base ? money(monthly, base) : '—'} / 月</span></PanelHeader>
      <div className="grid gap-4 p-4">
        {budget !== null && base && <div data-slot="budget" className="grid gap-1.5">
          <div className="flex items-baseline justify-between gap-3 text-xs"><span className="text-muted-foreground">月预算 {money(budget, base)}</span>
            <span className={cn('tabular-nums', used !== null && used > 100 ? toneText.danger : used !== null && used >= 90 ? toneText.warning : 'text-muted-foreground')}>{used === null ? '' : used > 100 ? `超出 ${money(monthly - budget, base)}` : `已用 ${Math.round(used)}%`}</span></div>
          <Track value={used} tone={used !== null && used > 100 ? 'danger' : used !== null && used >= 90 ? 'warning' : 'good'} label={`月预算已用 ${Math.round(used ?? 0)}%`} />
        </div>}
        {categories.length ? <ul className="grid gap-3">{categories.map(group => <li key={group.name} className="grid gap-1">
          <div className="flex items-baseline justify-between gap-3 text-[13px]"><span className="min-w-0 truncate">{group.name}<span className="text-muted-foreground ml-1.5 text-xs">{group.count} 项</span></span><span className="flex-none tabular-nums">{money(group.monthly, base!)}<span className="text-muted-foreground ml-1.5 text-xs">{Math.round(group.share)}%</span></span></div>
          <Track value={group.share} tone="info" />
        </li>)}</ul> : <p className="text-muted-foreground text-sm">还没有可折算的周期费用。</p>}
        {ledger.totals.unconverted > 0 && <p className="text-warning text-xs">{ledger.totals.unconverted} 项缺少到 {base} 的汇率，未计入。</p>}
        <p className="text-muted-foreground text-xs leading-relaxed">按周期平摊到每月，服务器按资产设置的费用周期计算；一次性购买与已停用的项目不计入。</p>
      </div>
    </Panel>
    <Panel className="mb-0">
      <PanelHeader className="min-h-11 py-2"><PanelTitle>实际付款</PanelTitle>
        {years.length > 1 && <SegmentedGroup aria-label="年份">{years.map(entry => <SegmentedButton key={entry.year} aria-pressed={entry.year === year?.year} onClick={() => { setChosen(entry.year); setAll(false) }}>{entry.year}</SegmentedButton>)}</SegmentedGroup>}
      </PanelHeader>
      <ErrorNotice message={ledger.paymentsError} />
      {!ledger.payments ? <p role="status" className="text-muted-foreground p-4 text-sm">正在读取…</p> : !year ? <p className="text-muted-foreground p-4 text-sm">还没有付款记录。确认续期或在服务器的费用记录里登记后会出现在这里。</p> : <>
        <div className="flex flex-wrap items-baseline gap-x-4 gap-y-1 border-b px-4 py-3">
          <strong className="text-lg font-semibold tabular-nums">{year.converted !== null && base ? money(year.converted, base) : year.totals.map(total => amount(total.units.toString(), total.currency)).join(' + ')}</strong>
          <span className="text-muted-foreground text-xs">{year.year} 年 · {year.payments.length} 笔{year.converted !== null && year.totals.some(total => total.currency !== base) ? ` · ${year.totals.map(total => amount(total.units.toString(), total.currency)).join(' + ')}，按当前汇率折算` : ''}</span>
        </div>
        <ul className="divide-y">{shown.map(payment => {
          const units = signedUnits(payment), row = rowOf(payment)
          const label = <><span className="block truncate">{payment.name}</span>
            <small className="text-muted-foreground block truncate text-xs">{payment.kind === 'item' ? '确认续期' : recordKinds[payment.record_kind] ?? payment.record_kind}{paymentNote(payment) ? ` · ${paymentNote(payment)}` : ''}</small></>
          return <li key={`${payment.kind}-${payment.id}`} data-slot="payment" className="grid grid-cols-[5.5rem_minmax(0,1fr)_auto] items-center gap-x-3 px-4 py-2.5 text-[13px] max-sm:px-3">
            <span className="text-muted-foreground tabular-nums">{payment.paid_on}</span>
            {row ? <button type="button" className="min-w-0 text-left outline-none hover:underline focus-visible:underline" onClick={() => onOpen(row)}>{label}</button> : <span className="min-w-0">{label}</span>}
            <span className={cn('text-right tabular-nums', units < 0n && toneText.good, units === 0n && 'text-muted-foreground')}>{units < 0n ? '−' : ''}{paymentAmount(payment)}</span>
          </li>
        })}</ul>
        {year.payments.length > PAGE && <div className="border-t px-4 py-2"><Button variant="ghost" size="sm" onClick={() => setAll(value => !value)}>{all ? '收起' : `显示全部 ${year.payments.length} 笔`}</Button></div>}
        <p className="text-muted-foreground border-t px-4 py-2.5 text-xs">退款记为负数，价格变化与退订不计入合计。</p>
      </>}
    </Panel>
  </section>
}
