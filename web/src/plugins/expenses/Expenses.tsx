import { useRef, useState } from 'react'
import { AlertTriangleIcon, CalendarClockIcon, PlusIcon, ReceiptIcon, Settings2Icon, WalletIcon } from 'lucide-react'
import { api } from '../../api'
import { Confirm, DialogPresence, ErrorNotice, PageHeader, Refresh } from '../../components'
import { convert, money } from '../../display/currency'
import { Stat, StatStrip } from '../../display/ui'
import { navigate } from '../../format'
import { useAction } from '../../hooks'
import { SegmentedButton, SegmentedNav } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import EntryDetail from './EntryDetail'
import ExpenseForm from './ExpenseForm'
import LedgerList from './LedgerList'
import LedgerSettings from './LedgerSettings'
import type { SettingsTab } from './LedgerSettings'
import Spending from './Spending'
import Upcoming from './Upcoming'
import { paymentYears } from './model'
import type { LedgerRow } from './model'
import { confirmed, operationText, runOperation } from './operations'
import type { ItemOperation } from './operations'
import { optionalId, optionalText, parseAmount, root } from './types'
import type { Expense } from './types'
import { useLedger } from './useLedger'

type View = 'items' | 'upcoming' | 'spending'
const views: [View, string][] = [['items', '项目'], ['upcoming', '即将续费'], ['spending', '支出']]
const savedView = (): View => { try { const value = localStorage.getItem('apsp-expenses-view'); return value === 'upcoming' || value === 'spending' ? value : 'items' } catch { return 'items' } }
const base = '/expenses'
export const entryPath = (row: Pick<LedgerRow, 'kind' | 'id'>) => `${base}/${row.kind === 'item' ? 'items' : 'servers'}/${row.id}`

/**
 * The costs and renewals ledger (ADR 0118). It answers three questions, one view
 * each: what is paid for, what is coming, and how much it all costs. An entry's
 * detail has its own address, so a link opens it and Back closes it; settings
 * that are made once live in one dialog instead of tabs.
 */
export default function Expenses({ item, server }: { item?: number; server?: number }) {
  const ledger = useLedger()
  const [view, setView] = useState<View>(savedView)
  const [editing, setEditing] = useState<Expense | 'new' | null>(null)
  const [returnTo, setReturnTo] = useState<number | null>(null)
  const [settings, setSettings] = useState<SettingsTab | null>(null)
  const [asking, setAsking] = useState<{ item: Expense; kind: 'renew' | 'delete' } | null>(null)
  const [notice, setNotice] = useState('')
  const form = useAction(), operation = useAction()
  const request = useRef('')

  const selected = item !== undefined ? ledger.rows.find(row => row.kind === 'item' && row.id === item) : server !== undefined ? ledger.rows.find(row => row.kind === 'server' && row.id === server) : undefined
  const open = (row: LedgerRow) => navigate(entryPath(row))
  const close = () => navigate(base)
  const choose = (value: View) => { setView(value); try { localStorage.setItem('apsp-expenses-view', value) } catch { /* The choice stays for this visit. */ } }
  const edit = (value: Expense | 'new') => {
    if (ledger.stale) return
    form.clearError()
    // Editing from a detail returns there afterwards.
    setReturnTo(value !== 'new' && item === value.id ? value.id : null)
    if (item !== undefined || server !== undefined) close()
    setEditing(value)
  }
  const operate = (target: Expense, kind: ItemOperation) => {
    if (ledger.stale) return
    operation.clearError(); setNotice('')
    if (confirmed(kind)) { request.current = crypto.randomUUID(); setAsking({ item: target, kind: kind as 'renew' | 'delete' }); return }
    void operation.run(() => runOperation(target, kind, crypto.randomUUID()), result => {
      ledger.reload()
      if (kind === 'clone' && result) navigate(entryPath({ kind: 'item', id: result.id }))
      else setNotice(kind === 'toggle' ? `「${target.name}」${target.inactive ? '已恢复使用' : '已停用'}。` : '')
    })
  }
  const confirm = () => {
    if (!asking || ledger.stale) return
    const { item: target, kind } = asking
    void operation.run(() => runOperation(target, kind, request.current), result => {
      setAsking(null); ledger.reload()
      setNotice(kind === 'delete' ? `已删除「${target.name}」。` : result ? `「${target.name}」已记录付款，下次续费改为 ${result.next_payment}。` : '')
    })
  }
  const save = (data: FormData) => {
    if (!editing || ledger.stale) return
    const target = editing
    void form.run(() => api<Expense>(target === 'new' ? `${root}/subscriptions` : `${root}/subscriptions/${target.id}`, target === 'new' ? 'POST' : 'PUT', {
      name: String(data.get('name')).trim(), amount_units: parseAmount(data.get('amount')), currency: String(data.get('currency')).trim().toUpperCase(), cycle: data.get('cycle'), frequency: Number(data.get('frequency')),
      start_date: optionalText(data.get('start_date')), next_payment: data.get('next_payment'), category_id: optionalId(data.get('category_id')), payment_method_id: optionalId(data.get('payment_method_id')),
      payer_id: optionalId(data.get('payer_id')), notes: String(data.get('notes') ?? ''), url: String(data.get('url') ?? ''), inactive: data.get('inactive') === 'on', auto_renew: data.get('auto_renew') === 'on',
      notify: data.get('notify') === 'on', notify_days_before: Number(data.get('notify_days_before') ?? 0), cancellation_date: optionalText(data.get('cancellation_date')),
      replacement_id: optionalId(data.get('replacement_id')), version: target === 'new' ? null : target.version,
    }), saved => {
      setEditing(null); ledger.reload()
      if (returnTo !== null) navigate(entryPath({ kind: 'item', id: returnTo }))
      else setNotice(target === 'new' ? `已添加「${saved.name}」。` : `已保存「${saved.name}」。`)
    })
  }

  // The headline figures: the monthly run rate, the next thirty days, what needs doing and what was paid this year.
  const { totals, budget } = ledger
  const thisYear = paymentYears(ledger.payments ?? [], ledger.base, ledger.rates).find(entry => entry.year === ledger.today.slice(0, 4))
  const upcoming = (ledger.summary?.upcoming ?? []).filter(due => due.date >= ledger.today)
  let next30 = 0, unconverted30 = 0
  for (const due of upcoming) {
    if (due.amount_units === null) continue
    const value = ledger.base ? convert(Number(due.amount_units) / 10000, due.currency, ledger.base, ledger.rates) : null
    if (value === null) unconverted30++; else next30 += value
  }
  const over = budget !== null && totals.monthly > budget
  const parts = [totals.overdue && `${totals.overdue} 项已过续费日`, totals.soon && `${totals.soon} 项 7 天内到期`].filter(Boolean)

  return <>
    <PageHeader title="费用与续费">
      <Refresh onClick={ledger.reload} />
      <Button variant="outline" onClick={() => setSettings('general')}><Settings2Icon aria-hidden="true" />设置</Button>
      <Button disabled={ledger.stale} onClick={() => edit('new')}><PlusIcon aria-hidden="true" />添加项目</Button>
    </PageHeader>
    <div className="grid min-w-0 gap-4">
      <ErrorNotice message={ledger.error || (ledger.malformed ? '部分数据不完整，请刷新重试。' : '')} retry={ledger.reload} />
      <ErrorNotice message={!asking ? operation.error : ''} retry={operation.clearError} />
      {notice && <p role="status" className="text-success -mb-1 text-sm">{notice}</p>}
      <StatStrip label="费用概况" columns={4}>
        <Stat icon={WalletIcon} label="每月费用" value={ledger.base && ledger.loaded ? money(totals.monthly, ledger.base) : '—'} tone={over ? 'warning' : undefined}
          title={`使用中的服务与服务器按周期折合到每月${ledger.base ? `，以 ${ledger.base} 计` : ''}`}>
          {!ledger.loaded ? '等待数据' : totals.unconverted ? <span className="text-warning">{totals.unconverted} 项缺少汇率，未计入</span>
            : budget !== null && ledger.base ? <span className={over ? 'text-warning' : undefined}>{over ? `超出预算 ${money(totals.monthly - budget, ledger.base)}` : `预算 ${money(budget, ledger.base)} · 已用 ${Math.round(totals.monthly / budget * 100)}%`}</span>
              : `${totals.items} 项服务 · ${totals.servers} 台服务器`}
        </Stat>
        <Stat icon={CalendarClockIcon} label="未来 30 天" value={ledger.base && ledger.summary ? money(next30, ledger.base) : '—'} title="未来 30 天内到期的续费合计，按账本汇率折算">
          {ledger.summary ? `${upcoming.length} 笔续费${unconverted30 ? ` · ${unconverted30} 笔缺少汇率` : ''}` : '等待数据'}
        </Stat>
        <Stat icon={AlertTriangleIcon} label="待处理" value={ledger.loaded ? totals.attention : '—'} tone={totals.attention ? 'warning' : undefined} title="已过续费日的项目，以及 7 天内续费或到期的项目">
          {!ledger.loaded ? '等待数据' : parts.length ? parts.join(' · ') : '没有需要处理的项目'}
        </Stat>
        <Stat icon={ReceiptIcon} label={`${ledger.today.slice(0, 4)} 年已付`} value={!ledger.payments ? '—' : thisYear ? thisYear.converted !== null && ledger.base ? money(thisYear.converted, ledger.base) : `${thisYear.totals.length} 种币种` : ledger.base ? money(0, ledger.base) : '0'}
          title="今年确认续期与服务器费用记录的合计；退款记为负数">
          {!ledger.payments ? '等待数据' : `${thisYear?.payments.length ?? 0} 笔付款`}
        </Stat>
      </StatStrip>
      <SegmentedNav aria-label="费用与续费视图">{views.map(([value, label]) => <SegmentedButton key={value} aria-pressed={view === value} onClick={() => choose(value)}>{label}</SegmentedButton>)}</SegmentedNav>
      {view === 'items' && <LedgerList ledger={ledger} onOpen={open} onEdit={edit} onOperate={operate} onAdd={() => edit('new')} onImport={() => setSettings('migration')} />}
      {view === 'upcoming' && <Upcoming ledger={ledger} onOpen={open} onOperate={operate} />}
      {view === 'spending' && <Spending ledger={ledger} onOpen={open} />}
    </div>
    <DialogPresence>{(item !== undefined || server !== undefined) && <EntryDetail row={selected} missing={ledger.loaded && !selected} ledger={ledger} onClose={close} onEdit={edit} onOpenItem={id => navigate(entryPath({ kind: 'item', id }))} />}</DialogPresence>
    <DialogPresence>{editing && <ExpenseForm value={editing} entries={ledger.items ?? []} labels={ledger.labels} busy={form.busy} stale={ledger.stale} error={form.error} onSubmit={save} onClose={() => { setEditing(null); if (returnTo !== null) navigate(entryPath({ kind: 'item', id: returnTo })) }} />}</DialogPresence>
    <DialogPresence>{asking && <Confirm title={operationText[asking.kind].title(asking.item)} busy={operation.busy} error={operation.error} confirmDisabled={ledger.stale}
      tone={asking.kind === 'delete' ? 'destructive' : 'default'} confirmLabel={operationText[asking.kind].confirm} busyLabel={operationText[asking.kind].busy}
      onClose={() => setAsking(null)} onConfirm={confirm}>{operationText[asking.kind].body(asking.item)}</Confirm>}</DialogPresence>
    <DialogPresence>{settings && <LedgerSettings initial={settings} onClose={() => setSettings(null)} onChanged={ledger.reload} />}</DialogPresence>
  </>
}
