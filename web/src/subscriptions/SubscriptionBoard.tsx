import { memo, useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { AnimatePresence, m } from 'motion/react'
import { AlertTriangleIcon, CalendarClockIcon, CalendarIcon, CreditCardIcon, LayoutGridIcon, ListFilterIcon, RefreshCcwIcon, RssIcon, SearchIcon, TableIcon, WalletIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { Empty, ErrorNotice, PageHeader, Refresh } from '../components'
import { useResourceGroup } from '../hooks'
import { money } from '../display/currency'
import { Stat, StatStrip, cardLink, surface, toneFill, toneText } from '../display/ui'
import { root as expenses } from '../plugins/expenses/types'
import { boardCounts, renewalView, savedSort, selectEntries, validLabels, validRenewalSummary, validRenewals } from './board'
import type { BoardFilter, BoardSort, Entry, Tone } from './board'
import { cn } from '@/lib/utils'
import { revealMotion } from '@/lib/motion'
import { Button, buttonVariants } from '@/components/ui/button'
import { ProgressRing } from '@/components/progress-ring'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Skeleton } from '@/components/ui/skeleton'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'

type View = 'cards' | 'table'

function readPreference(key: string): string | null {
  try { return localStorage.getItem(`apsp-subscription-board-${key}`) } catch { return null }
}
function savePreference(key: string, value: string) {
  try { localStorage.setItem(`apsp-subscription-board-${key}`, value) } catch { /* Keep the in-memory choice when storage is disabled. */ }
}

const filters: [BoardFilter, string][] = [['all', '全部'], ['active', '使用中'], ['attention', '需关注'], ['stopped', '已停用']]
/** Plain values read in the foreground; only states worth noticing take a colour. */
const valueTone = (tone: Tone) => tone === 'warning' || tone === 'danger' || tone === 'info' ? toneText[tone] : 'text-foreground'
const statusText = (tone: Tone) => tone === 'good' ? 'text-muted-foreground' : toneText[tone]
const grid = 'grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] items-start gap-3 max-sm:grid-cols-1'
const detail = (item: { id: number }) => `#/plugins/expenses/items/${item.id}`
/** "约 CNY 7.50 / 月" when the price is not already a monthly amount in the ledger's currency. */
function perMonth(item: Entry['item'], view: Entry['view'], base?: string) {
  if (!base) return ''
  if (view.monthly === null) return item.currency !== base ? '缺少汇率' : ''
  return item.currency !== base || item.cycle !== 'month' || item.frequency !== 1 ? `约 ${money(view.monthly, base)} / 月` : ''
}

function Datum({ icon: Icon, label, value, tone, title }: { icon: LucideIcon; label: string; value: ReactNode; tone: Tone; title: string }) {
  return <div data-slot="datum" title={title} className="bg-muted/50 min-w-0 overflow-hidden rounded-md px-2 py-1">
    <span className="text-muted-foreground flex items-center gap-1 text-[11px] leading-4"><Icon className="size-3 flex-none" aria-hidden="true" />{label}</span>
    <span className={cn('block truncate text-xs leading-4 tabular-nums', valueTone(tone))}>{value}</span>
  </div>
}

/** One subscription: the ring is how much of the period has passed, the middle how many days are left. */
const SubscriptionCard = memo(function SubscriptionCard({ entry: { item, view }, base }: { entry: Entry; base?: string }) {
  const ring = item.inactive ? '已停用' : view.days === null ? `下次续费 ${item.next_payment}` : view.days < 0 ? `续费日已过 ${-view.days} 天` : `本期已过 ${Math.round(view.progress ?? 0)}%，${view.days} 天后续费`
  return <a data-slot="subscription-card" data-stopped={item.inactive || undefined} href={detail(item)} aria-label={`${item.name}，${view.status.label}，查看详情`}
    className={cn(surface, cardLink, item.inactive && 'bg-muted/30')}>
    <div className="flex min-w-0 items-center gap-2 px-3.5 pt-2.5 pb-1.5">
      <span className={cn('size-2 flex-none rounded-full', toneFill[view.status.tone])} aria-hidden="true" />
      <strong data-slot="card-name" className="min-w-0 flex-1 truncate text-sm font-semibold" title={item.name}>{item.name}</strong>
      <span className={cn('flex-none text-xs', statusText(view.status.tone))}>{view.status.label}</span>
    </div>
    <div className="flex min-w-0 items-center gap-3 px-3.5 pb-2.5">
      <ProgressRing value={view.progress} tone={item.inactive ? 'muted' : view.next.tone === 'muted' ? 'info' : view.next.tone} label={ring}>
        {item.inactive ? <span className="text-muted-foreground text-[13px] font-semibold">—</span> : view.days !== null && view.days < 0 ? <span className="text-destructive text-xs font-semibold">逾期</span>
          : <><span className="text-[13px] font-semibold">{view.days === null ? '—' : Math.min(999, view.days)}</span><span className="text-muted-foreground mt-0.5 text-[10px]">天</span></>}
      </ProgressRing>
      <div className="grid min-w-0 flex-1 gap-0.5">
        <div className="flex min-w-0 items-baseline gap-1.5 whitespace-nowrap">
          <strong className="truncate text-base leading-tight font-semibold tabular-nums">{view.price}</strong>
          <span className="text-muted-foreground flex-none text-xs">/ {view.cycle.replace(/^每\s*/, '')}</span>
        </div>
        <small className="text-muted-foreground truncate text-xs" title={view.monthly === null && base && item.currency !== base ? `缺少 ${item.currency} 的汇率` : undefined}>
          {[perMonth(item, view, base), view.category || '未分类'].filter(Boolean).join(' · ')}
        </small>
        {view.reasons.length > 0 && <small data-slot="attention" className="text-warning flex min-w-0 items-center gap-1 text-xs"><AlertTriangleIcon className="size-3 flex-none" aria-hidden="true" /><span className="truncate">{view.reasons.join(' · ')}</span></small>}
      </div>
    </div>
    <div className="grid grid-cols-3 gap-1.5 px-3.5 pb-3">
      <Datum icon={CalendarIcon} label="下次续费" value={view.next.label} tone={view.next.tone} title={view.next.title} />
      <Datum icon={RefreshCcwIcon} label="续费方式" value={item.inactive ? '—' : view.renewal} tone="muted" title={item.auto_renew ? '服务商自动扣款；到日后自动进入下一期' : '需要手动续费，并在「费用与续费」确认'} />
      <Datum icon={CreditCardIcon} label="付款" value={view.payment || '—'} tone="muted" title={view.payment ? `付款方式：${view.payment}` : '未设置付款方式'} />
    </div>
    {item.notes && <p className="text-muted-foreground -mt-1 truncate px-3.5 pb-3 text-xs" title={item.notes}>{item.notes}</p>}
  </a>
})

function SubscriptionTable({ entries, base }: { entries: Entry[]; base?: string }) {
  return <div className={cn(surface, 'max-w-full min-w-0 overflow-hidden')}><Table>
    <TableHeader><TableRow><TableHead>订阅</TableHead><TableHead>费用 / 周期</TableHead><TableHead>折合每月</TableHead><TableHead>下次续费</TableHead><TableHead>续费方式</TableHead><TableHead>分类 / 付款</TableHead></TableRow></TableHeader>
    <TableBody>{entries.map(({ key, item, view }) => <TableRow key={key} data-stopped={item.inactive || undefined}>
      <TableCell><a href={detail(item)} className="flex min-w-0 items-center gap-2 font-medium hover:underline"><span className={cn('size-2 flex-none rounded-full', toneFill[view.status.tone])} title={view.status.label} />{item.name}</a>{view.reasons.length > 0 && <small className="text-warning block text-xs">{view.reasons.join(' · ')}</small>}</TableCell>
      <TableCell className="text-xs whitespace-nowrap tabular-nums">{view.price}<small className="text-muted-foreground block">{view.cycle}</small></TableCell>
      <TableCell className="text-xs whitespace-nowrap tabular-nums">{view.monthly !== null && base ? money(view.monthly, base) : '—'}</TableCell>
      <TableCell title={view.next.title} className={cn('text-xs whitespace-nowrap', valueTone(view.next.tone))}>{item.inactive ? '已停用' : item.next_payment}{!item.inactive && <small className="block">{view.next.label}</small>}</TableCell>
      <TableCell className="text-xs whitespace-nowrap">{item.inactive ? '—' : `${view.renewal}续费`}</TableCell>
      <TableCell className="text-muted-foreground max-w-56 truncate text-xs">{[view.category, view.payment].filter(Boolean).join(' · ') || '—'}</TableCell>
    </TableRow>)}</TableBody>
  </Table></div>
}

/** The administrator's own subscriptions at a glance: what they cost per month and when each renews. */
export default function SubscriptionBoard() {
  const data = useResourceGroup<{ items: unknown; labels: unknown; summary: unknown }>({ items: `${expenses}/subscriptions`, labels: `${expenses}/labels`, summary: `${expenses}/summary` }, 60_000, { keep: true })
  const [view, setView] = useState<View>(() => readPreference('view') === 'table' ? 'table' : 'cards')
  const [sort, setSort] = useState<BoardSort>(() => savedSort(readPreference('sort')))
  const [filter, setFilter] = useState<BoardFilter>('all')
  const [category, setCategory] = useState('')
  const [query, setQuery] = useState('')
  const [optionsOpen, setOptionsOpen] = useState(false)

  const items = validRenewals(data.items.data) ? data.items.data : undefined
  const labels = validLabels(data.labels.data) ? data.labels.data.entries : undefined
  const ledger = validRenewalSummary(data.summary.data) ? data.summary.data : undefined
  const malformed = (['items', 'labels', 'summary'] as const).some(key => data[key].data !== undefined && !{ items, labels, summary: ledger }[key])
  const base = ledger?.base_currency
  const entries = useMemo<Entry[]>(() => (items ?? []).map(item => ({ key: `item-${item.id}`, item, view: renewalView(item, ledger?.from, base, ledger?.exchange, labels ?? []) })), [items, ledger, base, labels])
  const visible = useMemo(() => selectEntries(entries, { query, filter, category, sort }), [entries, query, filter, category, sort])
  const counts = boardCounts(entries)
  const categories = (labels ?? []).filter(label => label.kind === 'category' && entries.some(entry => entry.item.category_id === label.id))
  const budget = ledger?.budget_units ? Number(ledger.budget_units) / 10000 : null
  const over = budget !== null && counts.monthly > budget
  const filterCounts: Record<BoardFilter, number> = { all: counts.all, active: counts.active, attention: counts.attention, stopped: counts.stopped }
  const parts = [counts.overdue && `${counts.overdue} 个已过续费日`, counts.soon && `${counts.soon} 个 7 天内续费`].filter(Boolean)

  const refresh = () => { data.items.reload(); data.labels.reload(); data.summary.reload() }
  const reset = () => { setQuery(''); setFilter('all'); setCategory('') }
  const changed = Boolean(query) || filter !== 'all' || Boolean(category)
  const customised = Boolean(category) || sort !== 'next' || view !== 'cards'
  const chooseView = (value: View) => { setView(value); savePreference('view', value) }
  const chooseSort = (value: string) => { const next = savedSort(value); setSort(next); savePreference('sort', next) }

  return <div data-slot="subscription-board" className="flex flex-col gap-3">
    <PageHeader title="订阅看板" description="你自己付费的订阅：每月花多少、什么时候续费。">
      <Refresh onClick={refresh} />
      <a className={buttonVariants({ variant: 'outline' })} href="#/plugins/expenses">在费用与续费中管理</a>
    </PageHeader>
    <ErrorNotice message={data.items.error || data.summary.error || (malformed ? '订阅数据不完整，请刷新重试。' : '')} retry={refresh} />
    <StatStrip label="订阅总览" columns={4}>
      <Stat icon={RssIcon} label="使用中" value={items ? counts.active : '—'} unit={items ? `/ ${counts.all} 个` : undefined} tone="good">{!items ? '等待数据' : counts.stopped ? `${counts.stopped} 个已停用` : '全部使用中'}</Stat>
      <Stat icon={WalletIcon} label="每月费用" value={items && base ? money(counts.monthly, base) : '—'} tone={over ? 'warning' : undefined} title={`使用中的订阅按周期折合到每月${base ? `，以 ${base} 计` : ''}${budget !== null && base ? `；月预算 ${money(budget, base)}` : ''}`}>
        {!items ? '等待数据' : counts.unconverted ? <span className="text-warning">{counts.unconverted} 个缺少汇率，未计入</span> : over && base ? <span className="text-warning">超出月预算 {money(counts.monthly - budget!, base)}</span> : budget !== null && base ? `月预算 ${money(budget, base)}` : '按周期平摊'}
      </Stat>
      <Stat icon={CalendarClockIcon} label="30 天内续费" value={items && base ? money(counts.monthTotal, base) : '—'} title="未来 30 天内要续费的订阅，按账本汇率折算">{items ? `${counts.month} 个` : '等待数据'}</Stat>
      <Stat icon={AlertTriangleIcon} label="需关注" value={items ? counts.attention : '—'} tone={counts.attention ? 'warning' : undefined} title="已过续费日，或 7 天内续费">{!items ? '等待数据' : parts.length ? parts.join(' · ') : '没有需要处理的订阅'}</Stat>
    </StatStrip>

    <div data-slot="subscription-toolbar" className="min-w-0">
      <div className="flex min-h-9 items-center gap-3 max-lg:flex-wrap max-lg:gap-2">
        <label className="relative flex min-w-44 flex-[0_1_300px] items-center max-lg:flex-[1_1_180px] max-sm:min-w-24 max-sm:flex-[1_1_100px]">
          <SearchIcon className="text-muted-foreground pointer-events-none absolute left-2.5 size-4" aria-hidden="true" />
          <Input type="search" aria-label="搜索订阅" placeholder="搜索名称、分类、备注" value={query} onChange={event => setQuery(event.target.value)} className="pl-8" />
        </label>
        <SegmentedGroup aria-label="订阅状态筛选" className="no-scrollbar max-w-full flex-nowrap overflow-x-auto *:shrink-0 max-lg:order-3 max-lg:flex-[1_1_100%]">
          {filters.map(([value, label]) => <SegmentedButton key={value} aria-pressed={filter === value} onClick={() => setFilter(value)}>{label}<span data-slot="filter-count" className="text-muted-foreground text-xs tabular-nums" aria-hidden="true">{items ? filterCounts[value] : '—'}</span></SegmentedButton>)}
        </SegmentedGroup>
        <div className="ml-auto flex flex-none items-center gap-1">
          <span className="text-muted-foreground mr-1.5 text-xs whitespace-nowrap">{items ? `${visible.length} 个订阅` : '等待数据'}</span>
          <Button variant="ghost" size="icon" className={cn(customised && 'bg-primary/10 text-primary')} aria-label="筛选与视图" title="筛选与视图" aria-expanded={optionsOpen} aria-controls="subscription-board-options" onClick={() => setOptionsOpen(value => !value)}><ListFilterIcon /></Button>
        </div>
      </div>
      <AnimatePresence initial={false}>
        {optionsOpen && <m.div key="options" {...revealMotion} className="overflow-hidden">
          <div id="subscription-board-options" className="text-muted-foreground mt-3 flex flex-wrap items-center gap-3 border-t pt-3 text-xs">
            {categories.length > 0 && <label className="flex items-center gap-2 max-sm:flex-auto max-sm:justify-between">分类<NativeSelect size="sm" aria-label="分类" value={category} onChange={event => setCategory(event.target.value)} className="max-w-40"><option value="">全部分类</option>{categories.map(label => <option key={label.id} value={label.id}>{label.name}</option>)}</NativeSelect></label>}
            <label className="flex items-center gap-2 max-sm:flex-auto max-sm:justify-between">排序<NativeSelect size="sm" aria-label="排序" value={sort} onChange={event => chooseSort(event.target.value)} className="max-w-44"><option value="next">先续费的在前</option><option value="cost">每月费用高的在前</option><option value="name">按名称</option></NativeSelect></label>
            <div className="ml-auto flex items-center gap-3 max-sm:m-0 max-sm:w-full max-sm:justify-end">
              {changed && <Button variant="link" size="sm" onClick={reset}>清除筛选</Button>}
              <SegmentedGroup aria-label="看板视图">
                <SegmentedButton aria-pressed={view === 'cards'} onClick={() => chooseView('cards')}><LayoutGridIcon aria-hidden="true" />卡片</SegmentedButton>
                <SegmentedButton aria-pressed={view === 'table'} onClick={() => chooseView('table')}><TableIcon aria-hidden="true" />表格</SegmentedButton>
              </SegmentedGroup>
            </div>
          </div>
        </m.div>}
      </AnimatePresence>
    </div>

    {!items ? !data.items.error && !malformed && <CardsLoading />
      : !entries.length ? <Empty icon={RssIcon} title="还没有订阅" description="在「费用与续费」里添加你付费的服务，或从 Wallos 导入。"><a className={buttonVariants()} href="#/plugins/expenses">前往费用与续费</a></Empty>
        : !visible.length ? <div className="text-muted-foreground flex min-h-60 flex-col items-center justify-center gap-3 p-6 text-center"><RssIcon className="size-8" aria-hidden="true" /><strong className="text-foreground text-sm font-medium">没有匹配的订阅</strong><Button onClick={reset}>清除筛选</Button></div>
          : view === 'cards' ? <section aria-label="订阅" className={grid}>{visible.map(entry => <div key={entry.key} className="min-w-0"><SubscriptionCard entry={entry} base={base} /></div>)}</section>
            : <SubscriptionTable entries={visible} base={base} />}
    {view === 'cards' && entries.length > 0 && <p className="text-muted-foreground text-xs">圆环是本期已经过去的时间，中间是距下次续费的天数。</p>}
  </div>
}

/** First read: card outlines in the same grid as the subscriptions to come. */
function CardsLoading() {
  return <div data-slot="loading" role="status" className={grid}>
    <span className="sr-only">正在读取订阅…</span>
    {[0, 1, 2].map(index => <div key={index} aria-hidden="true" className={cn(surface, 'grid gap-3 p-3.5')}>
      <Skeleton className="h-4 w-1/2" />
      <div className="flex items-center gap-3"><Skeleton className="size-14 rounded-full" /><div className="grid flex-1 gap-1.5"><Skeleton className="h-4 w-2/3" /><Skeleton className="h-3 w-1/2" /></div></div>
      <div className="grid grid-cols-3 gap-1.5">{[0, 1, 2].map(cell => <Skeleton key={cell} className="h-9" />)}</div>
    </div>)}
  </div>
}
