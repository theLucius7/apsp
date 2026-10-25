import { Panel, PanelTitle } from '@/components/panel'
import { SegmentedNav, SegmentedButton } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { PackageIcon, WalletIcon } from 'lucide-react'
import { useRef, useState } from 'react'
import { api } from '../../api'
import { ErrorNotice, FormDialog, PageHeader, Stat, DialogPresence } from '../../components'
import { useAction, useResource, useResourceGroup } from '../../hooks'
import { amount, cycles, download, optionalId, optionalText, parseAmount, root } from './types'
import type { Expense, Label } from './types'
import ExpenseForm from './ExpenseForm'
import Configuration from './Configuration'
import Migration from './Migration'
import { Cancellations, Payments, References, ServerCostDialog } from './ServerLedger'

const tabs = ['续费项目', '统计', '日历', '付款记录', '退订提醒', '提醒记录', '配置', '导入导出'] as const
export default function Expenses() {
  const [tab, setTab] = useState<typeof tabs[number]>('续费项目')
  return <><PageHeader eyebrow="总览" title="费用与续费" description="一处查看服务器、域名和其他服务的周期费用、下次续费日与预计支出。服务器按资产设置自动列出，其他项目在这里添加。只记账和提醒，不付款，也不自动续费或退订。" /><SegmentedNav aria-label="费用与续费">{tabs.map(t => <SegmentedButton key={t} aria-pressed={tab === t} onClick={() => setTab(t)}>{t}</SegmentedButton>)}</SegmentedNav><div className="mt-5 grid min-w-0 gap-4">{tab === '续费项目' && <><Subscriptions /><ServerRenewals /><References /></>}{tab === '付款记录' && <Payments />}{tab === '退订提醒' && <Cancellations />}{tab === '统计' && <Summary />}{tab === '日历' && <Calendar />}{tab === '提醒记录' && <Notifications />}{tab === '配置' && <Configuration />}{tab === '导入导出' && <Migration />}</div></>
}
function Subscriptions() {
  const data = useResourceGroup<{ entries: Expense[]; labels: { entries: Label[] } }>({ entries: `${root}/subscriptions`, labels: `${root}/labels` }, 30000), action = useAction()
  const [editing, setEditing] = useState<Expense | 'new' | null>(null), [operation, setOperation] = useState<{ entry: Expense; kind: 'clone' | 'renew' | 'delete' } | null>(null)
  const [query, setQuery] = useState(''), [state, setState] = useState('active'), [category, setCategory] = useState('')
  const request = useRef('')
  const stale = !data.entries.isCurrent() || !data.labels.isCurrent() || Boolean(data.entries.error || data.labels.error)
  const entries = data.entries.data ?? [], labels = data.labels.data?.entries ?? []
  const label = (id: number | null) => labels.find(l => l.id === id)?.name ?? '—'
  const rows = entries.filter(e => e.name.toLocaleLowerCase().includes(query.toLocaleLowerCase()) && (state === 'all' || e.inactive === (state === 'inactive')) && (!category || String(e.category_id) === category))
  const reload = () => { data.entries.reload(); data.labels.reload() }
  function save(form: FormData) {
    if (!editing || !data.entries.isCurrent() || !data.labels.isCurrent()) return
    void action.run(() => api(editing === 'new' ? `${root}/subscriptions` : `${root}/subscriptions/${editing.id}`, editing === 'new' ? 'POST' : 'PUT', {
      name: form.get('name'), amount_units: parseAmount(form.get('amount')), currency: form.get('currency'), cycle: form.get('cycle'), frequency: Number(form.get('frequency')), start_date: optionalText(form.get('start_date')), next_payment: form.get('next_payment'),
      category_id: optionalId(form.get('category_id')), payment_method_id: optionalId(form.get('payment_method_id')), payer_id: optionalId(form.get('payer_id')), notes: form.get('notes'), url: form.get('url'), inactive: form.get('inactive') === 'on', auto_renew: form.get('auto_renew') === 'on', notify: form.get('notify') === 'on', notify_days_before: Number(form.get('notify_days_before')), cancellation_date: optionalText(form.get('cancellation_date')), replacement_id: optionalId(form.get('replacement_id')), version: editing === 'new' ? null : editing.version,
    }), () => { setEditing(null); reload() })
  }
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>续费项目</PanelTitle><Button size="sm" disabled={stale} onClick={() => { action.clearError(); setEditing('new') }}>添加续费项目</Button></div><ErrorNotice message={data.entries.error || data.labels.error} retry={reload} /><div className="flex flex-wrap items-center gap-3 p-4 [&>*]:w-auto [&>*]:min-w-36"><Input aria-label="搜索项目" placeholder="搜索项目名称" value={query} onChange={e => setQuery(e.target.value)} /><NativeSelect aria-label="项目状态" value={state} onChange={e => setState(e.target.value)}><option value="active">使用中</option><option value="inactive">已停用</option><option value="all">全部</option></NativeSelect><NativeSelect aria-label="筛选分类" value={category} onChange={e => setCategory(e.target.value)}><option value="">全部分类</option>{labels.filter(l => l.kind === 'category').map(l => <option key={l.id} value={l.id}>{l.name}</option>)}</NativeSelect></div>
    <div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>项目</TableHead><TableHead>费用 / 周期</TableHead><TableHead>下次续费</TableHead><TableHead>分类 / 付款</TableHead><TableHead>续费设置</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{rows.map(e => <TableRow key={e.id}><TableCell>{e.url ? <a href={e.url} target="_blank" rel="noreferrer">{e.name}</a> : e.name}{e.notes && <p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">{e.notes}</p>}</TableCell><TableCell>{amount(e.amount_units, e.currency)}<p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">每 {e.frequency} {cycles[e.cycle]}</p></TableCell><TableCell>{e.next_payment}{e.cancellation_date && <p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">取消：{e.cancellation_date}</p>}</TableCell><TableCell>{label(e.category_id)}<p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">{label(e.payment_method_id)} · {label(e.payer_id)}</p></TableCell><TableCell>{e.inactive ? '已停用' : e.auto_renew ? '自动续费' : '手动续费'}<p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">{e.notify ? `提前 ${e.notify_days_before} 天提醒` : '不提醒'}</p>{e.replacement_id && <p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">替代：{entries.find(s => s.id === e.replacement_id)?.name}</p>}</TableCell><TableCell><div className="flex flex-wrap items-center gap-2"><Button variant="ghost" size="sm" disabled={stale} onClick={() => { action.clearError(); setEditing(e) }}>编辑</Button>{(['renew', 'clone', 'delete'] as const).map(kind => <Button key={kind} variant="ghost" size="sm" disabled={stale} onClick={() => { action.clearError(); request.current = crypto.randomUUID(); setOperation({ entry: e, kind }) }}>{({ renew: '续期', clone: '复制', delete: '删除' })[kind]}</Button>)}</div></TableCell></TableRow>)}{!rows.length && <TableRow><TableCell colSpan={6}>暂无匹配的项目。添加续费项目，或在「导入导出」中载入 Wallos 配置。</TableCell></TableRow>}</TableBody></Table></div>
    <DialogPresence>{editing && <ExpenseForm value={editing} entries={entries} labels={labels} busy={action.busy} stale={stale} error={action.error} onSubmit={save} onClose={() => setEditing(null)} />}</DialogPresence>
    <DialogPresence>{operation && <FormDialog title={`${({ renew: '确认续期', clone: '复制项目', delete: '删除项目' })[operation.kind]} · ${operation.entry.name}`} onClose={() => setOperation(null)} busy={action.busy} submitDisabled={stale} error={action.error} onSubmit={() => {
      if (!data.entries.isCurrent()) return
      const { entry, kind } = operation
      void action.run(() => api(`${root}/subscriptions/${entry.id}${kind === 'delete' ? '' : `/${kind}`}`, kind === 'delete' ? 'DELETE' : 'POST', { version: entry.version, ...(kind === 'delete' ? {} : { request_id: request.current }) }), () => { setOperation(null); reload() })
    }}><p>{({ renew: '确认已经续费后，将下次续费推进到今天之后的下一个周期，标记为使用中，并记录一笔付款。', clone: '复制配置后默认停用且关闭提醒，方便核对后再启用。', delete: '将删除此项目及其付款记录，并取消尚未发送的提醒。' })[operation.kind]}</p></FormDialog>}</DialogPresence>
  </Panel>
}
type Due = { kind: 'item' | 'server' | 'reference'; id: number; name: string; date: string; amount_units: string | null; currency: string; auto_renew: boolean; path?: string }
type SummaryView = { from: string; to: string; active: number; inactive: number; totals_units: Record<string, string>; servers: { count: number; unpriced: number; totals_units: Record<string, string> }; upcoming: Due[]; categories: { category_id: number | null; totals: Record<string, string> }[]; base_currency: string; budget_units: string | null; converted_yearly: number | null; conversion_stale?: boolean; missing_currencies: string[]; exchange: { source: string; rate_date?: string; stale: boolean } }
function Summary() {
  const data = useResource<SummaryView>(`${root}/summary`, 30000), labels = useResource<{ entries: Label[] }>(`${root}/labels`, 30000), view = data.data
  return <><ErrorNotice message={data.error || labels.error} retry={() => { data.reload(); labels.reload() }} />{view && <><div className="mb-4 grid grid-cols-2 gap-3 lg:grid-cols-4"><Stat icon={PackageIcon} label="使用中的项目" value={view.active} note={`${view.inactive} 项已停用 · ${view.servers.count} 台服务器`} /><Stat icon={WalletIcon} label="未来 365 天预计费用" value={view.converted_yearly === null ? '汇率不完整' : `${view.base_currency} ${view.converted_yearly.toFixed(2)}`} note={`${view.from} 至 ${view.to}${view.conversion_stale ? ' · 汇率可能过期' : ''}`} /><Stat icon={WalletIcon} label="月均预计费用" value={view.converted_yearly === null ? '—' : `${view.base_currency} ${(view.converted_yearly / 12).toFixed(2)}`} note={view.budget_units ? `月预算 ${amount(view.budget_units, view.base_currency)}` : '未设置月预算'} /></div><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>30 天内续费</PanelTitle></div>{view.upcoming.length === 0 ? <p className="text-muted-foreground p-4 text-sm">未来 30 天没有需要续费的项目。</p> : <DueTable entries={view.upcoming} />}</Panel><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>各币种预计支出</PanelTitle></div><div className="p-4 space-y-3">{Object.entries(view.totals_units).map(([code, total]) => <p key={code}>{amount(total, code)}</p>)}<p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">按续费日估算，含 {view.servers.count} 台服务器{view.servers.unpriced > 0 ? `（${view.servers.unpriced} 台未填写价格，未计入）` : ''}。手动续费仅计入下一期，停用与取消后的费用不计入；服务器按费用周期从到期日起推算。</p>{view.missing_currencies.length > 0 && <p>缺少汇率：{view.missing_currencies.join('、')}。原币金额仍完整显示。</p>}<p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">折算来源：{view.exchange.source || '未取得汇率'} · {view.exchange.rate_date ?? '日期未知'}{view.exchange.stale ? ' · 缓存可能过期' : ''}</p></div></Panel><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>分类支出</PanelTitle></div><div className="p-4 space-y-3">{Object.keys(view.servers.totals_units).length > 0 && <p>服务器：{Object.entries(view.servers.totals_units).map(([code, total]) => amount(total, code)).join('；')}</p>}{view.categories.map(c => <p key={c.category_id ?? 'none'}>{labels.data?.entries.find(l => l.id === c.category_id)?.name ?? '未分类'}：{Object.entries(c.totals).map(([code, total]) => amount(total, code)).join('；')}</p>)}</div></Panel></>}</>
}
const sub = 'text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere'
const link = 'text-primary underline-offset-4 hover:underline'
function DueTable({ entries }: { entries: Due[] }) {
  return <div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>日期</TableHead><TableHead>项目</TableHead><TableHead>预计金额</TableHead><TableHead>续费方式</TableHead></TableRow></TableHeader><TableBody>{entries.map(e => <TableRow key={`${e.kind}/${e.id}/${e.date}`}>
    <TableCell className="whitespace-nowrap">{e.date}</TableCell>
    <TableCell className="max-w-[280px] wrap-anywhere">{e.kind === 'server' ? <><a className={link} href={`#/servers/${e.id}/assets`}>{e.name}</a><p className={sub}>服务器</p></> : e.kind === 'reference' ? <><a className={link} href={`#${e.path ?? ''}`}>{e.name}</a><p className={sub}>机场订阅</p></> : e.name}</TableCell>
    <TableCell>{e.kind === 'reference' ? '—' : e.amount_units === null ? '未填写' : amount(e.amount_units, e.currency)}</TableCell>
    <TableCell>{e.kind === 'reference' ? '服务商报告的到期日' : e.kind === 'server' ? (e.auto_renew ? '自动顺延到期记录' : '按到期日') : e.auto_renew ? '自动续费' : '手动续费'}</TableCell>
  </TableRow>)}</TableBody></Table></div>
}
type ServerRenewal = { id: number; name: string; amount_units: string | null; currency: string; cycle_days: number; next_renewal: string; auto_renewal: boolean }
// Servers come from their asset settings and are edited there; expiry reminders stay in alert settings.
function ServerRenewals() {
  const data = useResource<{ entries: ServerRenewal[] }>(`${root}/servers`, 30000), [costs, setCosts] = useState<ServerRenewal | null>(null)
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>服务器</PanelTitle><span className="text-muted-foreground text-xs">来自资产设置，只读</span></div>
    <p className="text-muted-foreground border-b px-4 py-3 text-xs leading-relaxed">已设置到期日期的服务器自动列在这里。价格、周期与到期在服务器详情的「资产与流量」中修改，到期提醒在「告警设置」中配置；购买、续费、退款与退订在「费用记录」中登记，带有效期的续费会更新服务器到期日期。</p>
    <ErrorNotice message={data.error} retry={data.reload} />
    {data.data && (data.data.entries.length === 0 ? <p className="text-muted-foreground p-4 text-sm">还没有设置到期日期的服务器。</p> : <div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>服务器</TableHead><TableHead>费用 / 周期</TableHead><TableHead>到期</TableHead><TableHead>续费方式</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{data.data.entries.map(s => <TableRow key={s.id}>
      <TableCell className="max-w-[280px] wrap-anywhere"><a className={link} href={`#/servers/${s.id}/assets`}>{s.name}</a></TableCell>
      <TableCell>{s.amount_units === null ? '未填写价格' : amount(s.amount_units, s.currency)}<p className={sub}>{s.cycle_days > 0 ? `每 ${s.cycle_days} 天` : '一次性'}</p></TableCell>
      <TableCell className="whitespace-nowrap">{s.next_renewal}</TableCell>
      <TableCell>{s.auto_renewal ? '自动顺延到期记录' : '手动'}</TableCell>
      <TableCell><Button variant="ghost" size="sm" onClick={() => setCosts(s)}>费用记录</Button></TableCell>
    </TableRow>)}</TableBody></Table></div>)}
    <DialogPresence>{costs && <ServerCostDialog server={costs} onClose={() => setCosts(null)} onChanged={data.reload} />}</DialogPresence>
  </Panel>
}
function Calendar() {
  const today = new Date(), start = `${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, '0')}-01`
  const [range, setRange] = useState({ from: start, to: `${today.getFullYear()}-12-31` }), [draft, setDraft] = useState(range)
  const path = `?from=${encodeURIComponent(range.from)}&to=${encodeURIComponent(range.to)}`, data = useResource<{ entries: Due[] }>(`${root}/calendar${path}`, 30000), action = useAction()
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>续费日历</PanelTitle><Button variant="ghost" size="sm" disabled={action.busy || !data.isCurrent()} onClick={() => void action.run(() => download(`${root}/calendar.ics${path}`, 'apsp-expenses.ics'))}>导出日历</Button></div><form className="flex flex-wrap items-center gap-3 p-4 [&>*]:w-auto [&>*]:min-w-36" onSubmit={e => { e.preventDefault(); setRange(draft) }}><Input type="date" aria-label="开始日期" required value={draft.from} onChange={e => setDraft({ ...draft, from: e.target.value })} /><Input type="date" aria-label="结束日期" required value={draft.to} onChange={e => setDraft({ ...draft, to: e.target.value })} /><Button variant="outline" size="sm">查看</Button></form><ErrorNotice message={data.error || action.error} retry={data.reload} />{data.data && (data.data.entries.length === 0 ? <p className="text-muted-foreground p-4 text-sm">这段时间没有续费。</p> : <DueTable entries={data.data.entries} />)}</Panel>
}
function Notifications() {
  const data = useResource<{ entries: { id: string; kind: string; created_at: number; detail: { name?: string; due?: string }; deliveries: { channel: string; state: string; attempts: number; last_error: string | null }[] }[] }>(`${root}/notifications`, 30000)
  const kinds: Record<string, string> = { renewal_reminder: '续费提醒', schedule_advanced: '计划推进', cancelled_on_date: '按计划停用', manual_renewal: '手动续期', created: '新增项目', updated: '修改项目', deleted: '删除项目', cloned: '复制项目', imported: '导入配置', settings_updated: '修改设置' }
  const states: Record<string, string> = { pending: '待发送', sending: '发送中', sent: '已发送', failed: '失败', cancelled: '已取消' }
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>最近活动与提醒</PanelTitle></div><ErrorNotice message={data.error} retry={data.reload} /><div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>时间</TableHead><TableHead>事件</TableHead><TableHead>项目</TableHead><TableHead>投递结果</TableHead></TableRow></TableHeader><TableBody>{data.data?.entries.map(e => <TableRow key={e.id}><TableCell>{new Date(e.created_at * 1000).toLocaleString()}</TableCell><TableCell>{kinds[e.kind] ?? e.kind}</TableCell><TableCell>{e.detail.name ?? '—'} {e.detail.due ?? ''}</TableCell><TableCell>{e.deliveries.map(d => <p key={d.channel}>{d.channel === 'telegram' ? 'Telegram' : 'Webhook'} · {states[d.state] ?? d.state} · {d.attempts} 次{d.last_error && <span> · {d.last_error}</span>}</p>)}</TableCell></TableRow>)}</TableBody></Table></div></Panel>
}
