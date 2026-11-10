import { useState } from 'react'
import type { ReactNode } from 'react'
import { ExternalLinkIcon, PlusIcon } from 'lucide-react'
import { api } from '../../api'
import { Badge, ErrorNotice, Field, Modal } from '../../components'
import type { Tone as BadgeTone } from '../../components'
import { money } from '../currency/currency'
import { toneText } from '../../observability/display/ui'
import { useAction, useResource } from '../../hooks'
import { ActionMenu } from '@/components/action-menu'
import { Button, buttonVariants } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Spinner } from '@/components/ui/spinner'
import { Switch } from '@/components/ui/switch'
import { cn } from '@/lib/utils'
import { recordKinds, validCosts, validItemPayments } from './model'
import type { ItemRow, LedgerRow, ServerRow, Tone } from './model'
import { confirmed, operationText, runOperation } from './operations'
import type { ItemOperation } from './operations'
import { amount, currencies, currencyDigits, minorToUnits, parseMinor, root } from './types'
import type { Expense } from './types'
import type { Ledger } from './useLedger'

const badgeTone: Record<Tone, BadgeTone> = { good: 'good', warning: 'warm', danger: 'bad', info: 'neutral', muted: 'neutral' }
const isoDay = (at: number | null | undefined) => at ? new Date(at * 1000).toISOString().slice(0, 10) : ''

/** Facts as a two-column list; empty facts are left out instead of showing dashes. */
function Facts({ facts }: { facts: [string, ReactNode, boolean?][] }) {
  return <dl className="grid gap-x-6 gap-y-3 text-[13px] sm:grid-cols-2">
    {facts.filter(([, value]) => value !== null && value !== undefined && value !== '' && value !== false).map(([label, value, wide]) => <div key={label} className={cn('grid min-w-0 gap-0.5', wide && 'sm:col-span-2')}>
      <dt className="text-muted-foreground text-xs">{label}</dt><dd className="min-w-0 wrap-anywhere">{value}</dd>
    </div>)}
  </dl>
}

function Heading({ row }: { row: LedgerRow }) {
  return <div className="flex flex-wrap items-center gap-2">
    <Badge tone={badgeTone[row.due.status.tone]}>{row.due.status.label}</Badge>
    {!row.inactive && <span className={cn('text-xs', row.due.next.tone === 'muted' ? 'text-muted-foreground' : toneText[row.due.next.tone])}>{row.kind === 'server' ? '到期' : '下次续费'} {row.next}{row.due.days !== null ? ` · ${row.due.days < 0 ? `已过 ${-row.due.days} 天` : row.due.days === 0 ? '今天' : `${row.due.days} 天后`}` : ''}</span>}
  </div>
}

function ItemDetail({ row, ledger, onClose, onEdit, onOpenItem }: { row: ItemRow; ledger: Ledger; onClose: () => void; onEdit: (item: Expense) => void; onOpenItem: (id: number) => void }) {
  const item = row.item
  const paymentQuery = useResource<unknown>(`${root}/subscriptions/${item.id}/payments`, 60_000)
  const payments = { ...paymentQuery, data: validItemPayments(paymentQuery.data) ? paymentQuery.data : undefined, error: paymentQuery.error || (paymentQuery.data !== undefined && !validItemPayments(paymentQuery.data) ? '付款记录的数据不完整。' : '') }
  const action = useAction()
  const [asking, setAsking] = useState<'renew' | 'delete' | null>(null)
  const [requestId, setRequestId] = useState('')
  const [notice, setNotice] = useState('')
  const replacement = item.replacement_id ? ledger.items?.find(entry => entry.id === item.replacement_id)?.name : ''
  const act = (kind: ItemOperation) => {
    if (ledger.stale) return
    action.clearError(); setNotice('')
    if (confirmed(kind) && asking !== kind) { setAsking(kind as 'renew' | 'delete'); setRequestId(crypto.randomUUID()); return }
    // A confirmed operation keeps the id it was asked with, so a retry is recorded once; the others take a fresh one.
    const id = confirmed(kind) ? requestId : crypto.randomUUID()
    void action.run(() => runOperation(item, kind, id), result => {
      setAsking(null); ledger.reload(); payments.reload()
      if (kind === 'delete') onClose()
      else if (kind === 'clone' && result) onOpenItem(result.id)
      else if (kind === 'renew' && result) setNotice(`已记录付款，下次续费改为 ${result.next_payment}。`)
      else if (kind === 'toggle') setNotice(item.inactive ? '已恢复使用。' : '已停用：不再计入费用，也不会提醒。')
    })
  }
  const converted = row.monthly !== null && ledger.base && row.currency !== ledger.base ? `约 ${money(row.monthly, ledger.base)} / 月` : ''
  return <Modal title={item.name} onClose={onClose} busy={action.busy} wide slot="ledger-detail">
    <div data-slot="modal-body" className="grid gap-5 p-4">
      <Heading row={row} />
      <ErrorNotice message={action.error} />
      {notice && <p role="status" className="text-success text-sm">{notice}</p>}
      {asking ? <div data-slot="confirm" className="bg-muted/50 grid gap-2 rounded-lg border p-3.5">
        <strong className="text-sm font-semibold">{operationText[asking].title(item)}</strong>
        <p className="text-muted-foreground text-sm leading-relaxed">{operationText[asking].body(item)}</p>
      </div> : <>
        <Facts facts={[
          ['费用', <>{row.price} · {row.cycle}{converted && <span className="text-muted-foreground"> · {converted}</span>}</>],
          ['续费方式', item.auto_renew ? '服务商自动扣款' : '手动续费'],
          ['提醒', item.notify ? `提前 ${item.notify_days_before} 天` : '关闭'],
          ['分类', row.category], ['付款方式', row.payment], ['付款人', row.payer],
          ['开始日期', item.start_date ?? ''], ['取消日期', item.cancellation_date ?? ''], ['替代项目', replacement ?? ''],
          ['管理网址', item.url && <a className="text-primary inline-flex items-center gap-1 break-all underline-offset-4 hover:underline" href={item.url} target="_blank" rel="noreferrer">{item.url}<ExternalLinkIcon className="size-3 flex-none" aria-hidden="true" /></a>, true],
          ['备注', item.notes && <span className="whitespace-pre-wrap">{item.notes}</span>, true],
        ]} />
        <section aria-label="付款记录" className="grid gap-2">
          <h3 className="text-xs font-medium">付款记录</h3>
          <ErrorNotice message={payments.error} retry={payments.reload} />
          {!payments.data ? <p className="text-muted-foreground text-xs">正在读取…</p> : payments.data.entries.length === 0 ? <p className="text-muted-foreground text-xs">还没有付款记录；确认续期时会记一笔。</p>
            : <ul className="divide-y rounded-lg border text-[13px]">{payments.data.entries.map(payment => <li key={payment.id} className="flex items-center justify-between gap-3 px-3 py-2">
              <span className="tabular-nums">{payment.paid_on}<span className="text-muted-foreground ml-2 text-xs">覆盖 {payment.period_start} 起的一期</span></span><span className="tabular-nums">{amount(payment.amount_units, payment.currency)}</span>
            </li>)}</ul>}
        </section>
      </>}
    </div>
    <footer>
      {asking ? <>
        <Button variant="outline" disabled={action.busy} onClick={() => setAsking(null)}>取消</Button>
        <Button variant={asking === 'delete' ? 'destructive' : 'default'} disabled={action.busy || ledger.stale} onClick={() => act(asking)}>{action.busy && <Spinner role="presentation" aria-hidden="true" aria-label={undefined} />}{action.busy ? operationText[asking].busy : operationText[asking].confirm}</Button>
      </> : <>
        <span className="mr-auto"><ActionMenu label={`${item.name} 的更多操作`} groups={[[{ label: '复制为新项目', disabled: ledger.stale || action.busy, onSelect: () => act('clone') }, { label: item.inactive ? '恢复使用' : '停用', disabled: ledger.stale || action.busy, onSelect: () => act('toggle') }], [{ label: '删除', destructive: true, disabled: ledger.stale || action.busy, onSelect: () => act('delete') }]]} /></span>
        <Button variant="outline" disabled={ledger.stale || action.busy} onClick={() => onEdit(item)}>编辑</Button>
        {!item.inactive && <Button disabled={ledger.stale || action.busy} onClick={() => act('renew')}>确认续期</Button>}
      </>}
    </footer>
  </Modal>
}

type Reminder = { record_id: string; server_id: number; valid_until: number | null; enabled: boolean; lead_secs: number; notified_at: number | null; last_error: string | null }
const DEFAULT_LEAD = 604800
const leads = [[86400, '提前 1 天'], [259200, '提前 3 天'], [DEFAULT_LEAD, '提前 7 天'], [1209600, '提前 14 天'], [2592000, '提前 30 天']] as const
const startOfDay = (value: FormDataEntryValue | null) => value ? Math.floor(new Date(`${String(value)}T00:00:00`).getTime() / 1000) : null
const minorAmount = (minor: string | number, currency: string) => amount(minorToUnits(String(minor), currency), currency)

/** A cancellation's reminder, set where the cancellation is recorded. */
function CancellationReminder({ reminder, onSaved }: { reminder: Reminder; onSaved: () => void }) {
  const action = useAction()
  const [draft, setDraft] = useState({ enabled: reminder.enabled, lead_secs: reminder.lead_secs || DEFAULT_LEAD })
  const changed = draft.enabled !== reminder.enabled || draft.lead_secs !== (reminder.lead_secs || DEFAULT_LEAD)
  const save = () => void action.run(() => api('/api/operations/cancellation-reminders', 'POST', { record_id: reminder.record_id, ...draft }), onSaved)
  if (reminder.valid_until === null) return <small className="text-muted-foreground text-xs">没有生效日期，不能提醒</small>
  return <div data-slot="cancellation-reminder" className="flex flex-wrap items-center gap-2 text-xs">
    <label className="flex cursor-pointer items-center gap-2"><Switch size="sm" checked={draft.enabled} disabled={action.busy} onCheckedChange={enabled => setDraft(current => ({ ...current, enabled }))} aria-label="退订提醒" />提醒</label>
    <NativeSelect size="sm" aria-label="提前多久提醒" value={draft.lead_secs} disabled={action.busy || !draft.enabled} onChange={event => setDraft(current => ({ ...current, lead_secs: Number(event.target.value) }))}>
      {!leads.some(([seconds]) => seconds === draft.lead_secs) && <option value={draft.lead_secs}>提前 {Math.round(draft.lead_secs / 3600)} 小时</option>}
      {leads.map(([seconds, label]) => <option key={seconds} value={seconds}>{label}</option>)}
    </NativeSelect>
    {changed && <Button size="xs" disabled={action.busy} onClick={save}>保存</Button>}
    {reminder.notified_at !== null && <span className="text-muted-foreground">已提醒</span>}
    {(action.error || reminder.last_error) && <span className="text-destructive basis-full">{action.error || reminder.last_error}</span>}
  </div>
}

function ServerDetail({ row, ledger, onClose }: { row: ServerRow; ledger: Ledger; onClose: () => void }) {
  const server = row.server
  const costQuery = useResource<unknown>(`/api/servers/${server.id}/fleet/costs`, 60_000)
  const costs = { ...costQuery, data: validCosts(costQuery.data) ? costQuery.data : undefined, error: costQuery.error || (costQuery.data !== undefined && !validCosts(costQuery.data) ? '费用记录的数据不完整。' : '') }
  // Cancellation reminders belong to the operations feature; without it the controls stay away.
  const reminders = useResource<Reminder[]>('/api/operations/cancellation-reminders', 60_000)
  const action = useAction()
  const [adding, setAdding] = useState(false)
  const [kind, setKind] = useState('renewal')
  const [currency, setCurrency] = useState(server.currency)
  const digits = currencyDigits(currency)
  const submit = (form: FormData) => void action.run(() => {
    const code = String(form.get('currency')).trim().toUpperCase()
    return api(`/api/servers/${server.id}/fleet/costs`, 'POST', { kind, amount_minor: parseMinor(form.get('amount'), code), currency: code, occurred_at: startOfDay(form.get('occurred')) ?? Math.floor(Date.now() / 1000), valid_until: startOfDay(form.get('until')), reference: String(form.get('reference') ?? '') })
  }, () => { setAdding(false); costs.reload(); reminders.reload(); ledger.reload() })
  const records = [...(costs.data?.records ?? [])].sort((left, right) => right.occurred_at - left.occurred_at)
  const converted = row.monthly !== null && ledger.base && row.currency !== ledger.base ? ` · 约 ${money(row.monthly, ledger.base)} / 月` : ''
  return <Modal title={server.name} onClose={onClose} busy={action.busy} wide slot="ledger-detail">
    <div data-slot="modal-body" className="grid gap-5 p-4">
      <Heading row={row} />
      <Facts facts={[
        ['费用', row.price ? `${row.price} · ${row.cycle}${converted}` : '未填写价格'],
        ['续费方式', server.cycle_days <= 0 ? '一次性购买，到期不续' : server.auto_renewal ? '到期日按周期自动顺延' : '到期后需要登记续费'],
      ]} />
      <section aria-label="费用记录" className="grid gap-2">
        <div className="flex items-center justify-between gap-3">
          <h3 className="text-xs font-medium">费用记录{costs.data && costs.data.annual.length > 0 && <span className="text-muted-foreground ml-2 font-normal">{costs.data.annual.map(entry => `${entry.year} 年 ${minorAmount(entry.amount_minor, entry.currency)}`).join(' · ')}</span>}</h3>
          {!adding && <Button variant="outline" size="sm" onClick={() => { action.clearError(); setAdding(true) }}><PlusIcon aria-hidden="true" />登记费用</Button>}
        </div>
        <ErrorNotice message={costs.error || action.error} retry={costs.reload} />
        {adding && <form data-slot="cost-form" className="bg-muted/40 grid gap-x-4 rounded-lg border p-3.5 sm:grid-cols-2" onSubmit={event => { event.preventDefault(); submit(new FormData(event.currentTarget)) }}>
          <Field label="类型"><NativeSelect className="w-full" name="kind" value={kind} onChange={event => setKind(event.target.value)}>{Object.entries(recordKinds).map(([key, name]) => <option key={key} value={key}>{name}</option>)}</NativeSelect></Field>
          <div className="grid grid-cols-[minmax(0,1fr)_6rem] gap-2">
            <Field label="金额" hint={digits ? `最多 ${digits} 位小数` : '此币种只用整数'}><Input name="amount" required inputMode="decimal" placeholder="0.00" /></Field>
            <Field label="币种"><Input name="currency" required pattern="[A-Za-z]{3}" maxLength={3} list="ledger-currencies" value={currency} onChange={event => setCurrency(event.target.value.toUpperCase())} /></Field>
          </div>
          <Field label="日期"><Input name="occurred" type="date" defaultValue={new Date().toISOString().slice(0, 10)} /></Field>
          {(kind === 'renewal' || kind === 'purchase' || kind === 'cancellation') && <Field label={kind === 'cancellation' ? '退订生效日' : '有效期至'} hint={kind === 'cancellation' ? '填写后可以设置退订提醒。' : '填写后服务器的到期日期改为这一天。'}><Input name="until" type="date" /></Field>}
          <Field label="订单号或备注" className="sm:col-span-2"><Input name="reference" maxLength={512} /></Field>
          <div className="flex justify-end gap-2 sm:col-span-2"><Button type="button" variant="ghost" disabled={action.busy} onClick={() => setAdding(false)}>取消</Button><Button disabled={action.busy}>{action.busy ? '正在登记…' : '登记'}</Button></div>
        </form>}
        {!costs.data ? !costs.error && <p className="text-muted-foreground text-xs">正在读取…</p> : records.length === 0 ? !adding && <p className="text-muted-foreground text-xs">还没有费用记录。购买、续费、退款与退订都在这里登记。</p>
          : <ul className="divide-y rounded-lg border text-[13px]">{records.map(record => {
            const reminder = record.kind === 'cancellation' ? reminders.data?.find(entry => entry.record_id === record.id) : undefined
            return <li key={record.id} data-slot="cost-record" className="grid gap-1.5 px-3 py-2">
              <div className="flex flex-wrap items-baseline justify-between gap-x-3">
                <span className="tabular-nums">{isoDay(record.occurred_at)}<span className="ml-2">{recordKinds[record.kind] ?? record.kind}</span>{record.valid_until && <span className="text-muted-foreground ml-2 text-xs">{record.kind === 'cancellation' ? '生效' : '有效期至'} {isoDay(record.valid_until)}</span>}</span>
                <span className={cn('tabular-nums', record.kind === 'refund' && toneText.good)}>{record.kind === 'refund' ? '−' : ''}{minorAmount(record.amount_minor, record.currency)}</span>
              </div>
              {record.reference && <small className="text-muted-foreground text-xs wrap-anywhere">{record.reference}</small>}
              {reminder && <CancellationReminder key={`${reminder.record_id}-${reminder.enabled}-${reminder.lead_secs}`} reminder={reminder} onSaved={reminders.reload} />}
            </li>
          })}</ul>}
      </section>
    </div>
    <footer>
      <a className={cn(buttonVariants({ variant: 'ghost' }), 'mr-auto')} href={`#/servers/${server.id}/assets`}>在资产设置中修改价格与到期</a>
      <Button variant="outline" onClick={onClose}>关闭</Button>
    </footer>
    <datalist id="ledger-currencies">{currencies.map(code => <option key={code} value={code} />)}</datalist>
  </Modal>
}

/** The detail of one entry, opened from the list, the timeline or a link to it. */
export default function EntryDetail({ row, missing, ledger, onClose, onEdit, onOpenItem }: { row?: LedgerRow; missing: boolean; ledger: Ledger; onClose: () => void; onEdit: (item: Expense) => void; onOpenItem: (id: number) => void }) {
  if (row?.kind === 'item') return <ItemDetail key={row.key} row={row} ledger={ledger} onClose={onClose} onEdit={onEdit} onOpenItem={onOpenItem} />
  if (row?.kind === 'server') return <ServerDetail key={row.key} row={row} ledger={ledger} onClose={onClose} />
  return <Modal title={missing ? '找不到这一项' : '正在读取…'} onClose={onClose} slot="ledger-detail">
    <div data-slot="modal-body" className="text-muted-foreground p-4 text-sm">{missing ? '它可能已被删除，或者你没有查看这台服务器的权限。' : <Spinner />}</div>
    <footer><Button variant="outline" onClick={onClose}>关闭</Button></footer>
  </Modal>
}
