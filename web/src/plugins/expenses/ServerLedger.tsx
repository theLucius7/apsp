import { useState } from 'react'
import { api } from '../../api'
import { ErrorNotice, Field, Modal } from '../../components'
import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { bytes } from '../../format'
import { useAction, useResource } from '../../hooks'
import CancellationReminders from '../../operations/CancellationReminders'
import { amount, currencyDigits, minorToUnits, parseMinor, root } from './types'

// Server cost records and cancellation reminders keep their host tables and APIs
// (fleet assets, operations); the ledger is their single editing place (ADR 0097).

const heading = 'flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3'
const sub = 'text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere'
const link = 'text-primary underline-offset-4 hover:underline'
export const recordKinds: Record<string, string> = { renewal: '续费', purchase: '购买', price_change: '价格变化', refund: '退款', cancellation: '退订' }
const minorAmount = (minor: string, currency: string) => amount(minorToUnits(minor, currency), currency)
const date = (at: number | null | undefined) => at ? new Date(at * 1000).toLocaleDateString() : '—'
const startOfDay = (value: FormDataEntryValue | null) => value ? Math.floor(new Date(`${String(value)}T00:00:00`).getTime() / 1000) : null

type CostRecord = { id: string; kind: string; amount_minor: number | string; currency: string; occurred_at: number; valid_until: number | null; reference: string }
type Costs = { records: CostRecord[]; annual: { year: number; currency: string; amount_minor: string }[] }

/** Cost history of one server, with the form that adds a record (renewals with a validity date move the server's expiry). */
export function ServerCostDialog({ server, onClose, onChanged }: { server: { id: number; name: string }; onClose: () => void; onChanged: () => void }) {
  const costs = useResource<Costs>(`/api/servers/${server.id}/fleet/costs`), action = useAction()
  const [currency, setCurrency] = useState('CNY')
  const digits = currencyDigits(currency)
  function submit(form: FormData) {
    void action.run(() => {
      const code = String(form.get('currency')).trim().toUpperCase()
      return api(`/api/servers/${server.id}/fleet/costs`, 'POST', { kind: String(form.get('kind')), amount_minor: parseMinor(form.get('amount'), code), currency: code, occurred_at: startOfDay(form.get('occurred')) ?? Math.floor(Date.now() / 1000), valid_until: startOfDay(form.get('until')), reference: String(form.get('reference') ?? '') })
    }, () => { costs.reload(); onChanged() })
  }
  return <Modal title={`费用记录 · ${server.name}`} onClose={onClose} busy={action.busy} wide slot="server-costs">
    <div data-slot="modal-body" className="grid gap-4 p-4">
      <ErrorNotice message={costs.error || action.error} retry={costs.reload} />
      <form className="grid gap-x-4 sm:grid-cols-2" onSubmit={event => { event.preventDefault(); submit(new FormData(event.currentTarget)) }}>
        <Field label="记录类型"><NativeSelect className="w-full" name="kind" defaultValue="renewal">{Object.entries(recordKinds).map(([key, name]) => <option key={key} value={key}>{name}</option>)}</NativeSelect></Field>
        <Field label="币种"><Input name="currency" required pattern="[A-Za-z]{3}" maxLength={3} value={currency} onChange={event => setCurrency(event.target.value.toUpperCase())} /></Field>
        <Field label="金额" hint={digits ? `最多 ${digits} 位小数` : '此币种只使用整数'}><Input name="amount" required inputMode="decimal" placeholder="0.00" /></Field>
        <Field label="发生日期"><Input name="occurred" type="date" defaultValue={new Date().toISOString().slice(0, 10)} /></Field>
        <Field label="有效期至 / 退订生效日" hint="续费或购买填写后，会把服务器到期日期更新为此日期。"><Input name="until" type="date" /></Field>
        <Field label="订单 / 备注"><Input name="reference" maxLength={512} /></Field>
        <div className="sm:col-span-2"><Button type="submit" disabled={action.busy}>记录费用</Button></div>
      </form>
      {costs.data && <>
        {costs.data.annual.length > 0 && <p className="text-sm">各年实际支出：{costs.data.annual.map(row => `${row.year} · ${minorAmount(row.amount_minor, row.currency)}`).join('；')}</p>}
        {costs.data.records.length === 0 ? <p className="text-muted-foreground text-sm">还没有费用记录。</p> : <div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>日期</TableHead><TableHead>类型</TableHead><TableHead>金额</TableHead><TableHead>有效期至</TableHead><TableHead>订单 / 备注</TableHead></TableRow></TableHeader><TableBody>{costs.data.records.map(r => <TableRow key={r.id}>
          <TableCell className="whitespace-nowrap">{date(r.occurred_at)}</TableCell><TableCell>{recordKinds[r.kind] ?? r.kind}</TableCell><TableCell className="whitespace-nowrap">{minorAmount(String(r.amount_minor), r.currency)}</TableCell><TableCell className="whitespace-nowrap">{date(r.valid_until)}</TableCell><TableCell className="max-w-[220px] wrap-anywhere">{r.reference || '—'}</TableCell>
        </TableRow>)}</TableBody></Table></div>}
      </>}
    </div>
    <footer><Button variant="outline" disabled={action.busy} onClick={onClose}>关闭</Button></footer>
  </Modal>
}

type Reference = { kind: string; id: number; name: string; expires_at: number | null; expires_on: string | null; used_bytes: number | null; total_bytes: number | null; reported_at: number | null; path: string }

/** Expiries reported by proxy subscription sources (`subscription-userinfo`); for reference only. */
export function References() {
  const data = useResource<{ entries: Reference[] }>(`${root}/references`, 30000)
  if (data.data && data.data.entries.length === 0 && !data.error) return null
  return <Panel><div className={heading}><PanelTitle>机场订阅</PanelTitle><span className="text-muted-foreground text-xs">服务商报告的到期与流量，仅供参考</span></div>
    <p className="text-muted-foreground border-b px-4 py-3 text-xs leading-relaxed">代理订阅来源中报告了到期日期的机场会列在这里，数据随来源刷新更新。它们没有价格，也不会被续期；需要记账时，另外添加一个续费项目。</p>
    <ErrorNotice message={data.error} retry={data.reload} />
    {data.data && <div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>订阅来源</TableHead><TableHead>到期</TableHead><TableHead>已用 / 总量</TableHead><TableHead>报告时间</TableHead></TableRow></TableHeader><TableBody>{data.data.entries.map(r => <TableRow key={`${r.kind}/${r.id}`}>
      <TableCell className="max-w-[280px] wrap-anywhere"><a className={link} href={`#${r.path}`}>{r.name}</a></TableCell>
      <TableCell className="whitespace-nowrap">{r.expires_on ?? '—'}</TableCell>
      <TableCell className="whitespace-nowrap">{r.used_bytes === null ? '—' : bytes(r.used_bytes)} / {r.total_bytes ? bytes(r.total_bytes) : '不限'}</TableCell>
      <TableCell className="whitespace-nowrap">{date(r.reported_at)}</TableCell>
    </TableRow>)}</TableBody></Table></div>}
  </Panel>
}

type ItemPayment = { kind: 'item'; id: string; subscription_id: number; name: string; paid_on: string; period_start: string; amount_units: string; currency: string }
type ServerPayment = { kind: 'server'; id: string; server_id: number; name: string; record_kind: string; paid_on: string; valid_until: number | null; amount_minor: string; currency: string; reference: string }
type Payment = ItemPayment | ServerPayment

// Same rule as the server asset ledger: purchases and renewals count, refunds subtract.
function signedUnits(payment: Payment): bigint {
  if (payment.kind === 'item') return BigInt(payment.amount_units)
  const units = BigInt(minorToUnits(payment.amount_minor, payment.currency))
  return payment.record_kind === 'refund' ? -units : ['purchase', 'renewal'].includes(payment.record_kind) ? units : 0n
}

/** Actual spending: confirmed renewals of manual items and the server cost records. */
export function Payments() {
  const data = useResource<{ items: ItemPayment[]; servers: ServerPayment[] }>(`${root}/payments`, 30000)
  const rows: Payment[] = data.data ? [...data.data.items, ...data.data.servers].sort((a, b) => b.paid_on.localeCompare(a.paid_on)) : []
  const totals = new Map<string, bigint>()
  for (const row of rows) { const key = `${row.paid_on.slice(0, 4)} ${row.currency}`; totals.set(key, (totals.get(key) ?? 0n) + signedUnits(row)) }
  return <Panel><div className={heading}><PanelTitle>付款记录</PanelTitle></div>
    <p className="text-muted-foreground border-b px-4 py-3 text-xs leading-relaxed">续费项目在确认续期时记录付款；服务器的购买、续费、退款等记录在服务器的「费用记录」中添加。金额按原币显示，年度合计中退款为负、价格变化与退订不计入。</p>
    <ErrorNotice message={data.error} retry={data.reload} />
    {data.data && (rows.length === 0 ? <p className="text-muted-foreground p-4 text-sm">还没有付款记录。</p> : <>
      <div className="grid gap-1 border-b px-4 py-3 text-sm">{[...totals].sort(([a], [b]) => b.localeCompare(a)).map(([key, total]) => { const [year, code] = key.split(' '); return <p key={key}>{year} 年 · {amount(total.toString(), code)}</p> })}</div>
      <div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>日期</TableHead><TableHead>项目</TableHead><TableHead>类型</TableHead><TableHead>金额</TableHead><TableHead>说明</TableHead></TableRow></TableHeader><TableBody>{rows.map(r => <TableRow key={`${r.kind}/${r.id}`}>
        <TableCell className="whitespace-nowrap">{r.paid_on}</TableCell>
        <TableCell className="max-w-[260px] wrap-anywhere">{r.kind === 'server' ? <><a className={link} href={`#/servers/${r.server_id}/assets`}>{r.name}</a><p className={sub}>服务器</p></> : r.name}</TableCell>
        <TableCell>{r.kind === 'server' ? recordKinds[r.record_kind] ?? r.record_kind : '确认续期'}</TableCell>
        <TableCell className="whitespace-nowrap">{r.kind === 'server' ? minorAmount(r.amount_minor, r.currency) : amount(r.amount_units, r.currency)}</TableCell>
        <TableCell className="max-w-[240px] wrap-anywhere">{r.kind === 'server' ? [r.reference, r.valid_until ? `有效期至 ${date(r.valid_until)}` : ''].filter(Boolean).join(' · ') || '—' : `覆盖 ${r.period_start} 起的一期`}</TableCell>
      </TableRow>)}</TableBody></Table></div>
    </>)}
  </Panel>
}

/** Cancellation reminders on server cost records, previously under automation and recovery. */
export function Cancellations() {
  // The reminders component reports its own load and save errors.
  const run = (task: () => Promise<void>) => { void task().catch(() => undefined) }
  return <>
    <p className="text-muted-foreground mb-3 text-xs leading-relaxed">在服务器的「费用记录」中添加类型为“退订”并填写生效日期的记录，即可在这里为它开启提醒。提醒经「告警设置」的渠道发送，不会执行退订。</p>
    <CancellationReminders run={run} version={0} />
  </>
}

