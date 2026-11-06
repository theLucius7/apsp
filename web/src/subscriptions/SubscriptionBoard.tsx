import { memo, useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { AnimatePresence, m } from 'motion/react'
import { AlertTriangleIcon, CalendarClockIcon, CalendarIcon, CreditCardIcon, DatabaseIcon, DownloadIcon, LayoutGridIcon, ListFilterIcon, RefreshCcwIcon, RotateCcwIcon, RssIcon, SearchIcon, TableIcon, UsersIcon, WalletIcon, WaypointsIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { ErrorNotice, PageHeader, Refresh } from '../components'
import { ago, bytes } from '../format'
import { useResource } from '../hooks'
import { useNow } from '../display/useNow'
import { money } from '../display/currency'
import { Stat, StatStrip, Track, cardLink, surface, toneFill, toneText } from '../display/ui'
import { sourceMigrationPath, sourceRoot, validSourceMigration, validSubscriptionSources as validOrderedSources } from '../plugins/singbox/orderedSourceTypes'
import { validSubscriptionSources as validNumberedSources } from '../plugins/singbox/sourceTypes'
import { root as expenses } from '../plugins/expenses/types'
import { readShellActor } from '@/lib/shell-actor'
import type { ShellActor } from '@/lib/shell-actor'
import { boardCounts, fullTime, numberedSourceCard, orderedSourceCard, renewalView, savedSort, selectEntries, sourceView, userView, validLabels, validRenewalSummary, validRenewals, validUserBoard } from './board'
import type { BoardFilter, BoardKind, BoardSort, Entry, SourceCard, Tone } from './board'
import { cn } from '@/lib/utils'
import { revealMotion } from '@/lib/motion'
import { Button, buttonVariants } from '@/components/ui/button'
import { ProgressRing } from '@/components/progress-ring'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Skeleton } from '@/components/ui/skeleton'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'

const singbox = '/api/plugins/sing-box'
type View = 'cards' | 'table'
type UserEntry = Extract<Entry, { kind: 'users' }>
type RenewalEntry = Extract<Entry, { kind: 'renewals' }>
type SourceEntry = Extract<Entry, { kind: 'sources' }>

function readPreference(key: string): string | null {
  try { return localStorage.getItem(`apsp-subscription-board-${key}`) } catch { return null }
}
function savePreference(key: string, value: string) {
  try { localStorage.setItem(`apsp-subscription-board-${key}`, value) } catch { /* Keep the in-memory choice when storage is disabled. */ }
}

const kinds: { kind: BoardKind; label: string; icon: LucideIcon; manage: { href: string; label: string } }[] = [
  { kind: 'users', label: '代理用户', icon: UsersIcon, manage: { href: '#/plugins/sing-box/users', label: '管理代理用户' } },
  { kind: 'renewals', label: '续费项目', icon: CalendarClockIcon, manage: { href: '#/plugins/expenses', label: '费用与续费' } },
  { kind: 'sources', label: '订阅来源', icon: WaypointsIcon, manage: { href: '#/plugins/sing-box/nodes?view=sources', label: '管理订阅来源' } },
]
const filters: [BoardFilter, string][] = [['all', '全部'], ['usable', '可用'], ['attention', '需关注'], ['stopped', '已停用']]
/** Plain values read in the foreground; only states worth noticing take a colour. */
const valueTone = (tone: Tone) => tone === 'warning' || tone === 'danger' || tone === 'info' ? toneText[tone] : 'text-foreground'
const statusText = (tone: Tone) => tone === 'good' ? 'text-muted-foreground' : toneText[tone]
const grid = 'grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] items-start gap-3 max-sm:grid-cols-1'

function Datum({ icon: Icon, label, value, tone, title }: { icon: LucideIcon; label: string; value: ReactNode; tone: Tone; title: string }) {
  return <div data-slot="datum" title={title} className="bg-muted/50 min-w-0 overflow-hidden rounded-md px-2 py-1">
    <span className="text-muted-foreground flex items-center gap-1 text-[11px] leading-4"><Icon className="size-3 flex-none" aria-hidden="true" />{label}</span>
    <span className={cn('block truncate text-xs leading-4 tabular-nums', valueTone(tone))}>{value}</span>
  </div>
}

function CardHeader({ tone, name, status }: { tone: Tone; name: string; status: { label: string; tone: Tone } }) {
  return <div className="flex min-w-0 items-center gap-2 px-3.5 pt-2.5 pb-1.5">
    <span className={cn('size-2 flex-none rounded-full', toneFill[tone])} aria-hidden="true" />
    <strong data-slot="card-name" className="min-w-0 flex-1 truncate text-sm font-semibold" title={name}>{name}</strong>
    <span className={cn('flex-none text-xs', statusText(status.tone))}>{status.label}</span>
  </div>
}

function Reasons({ reasons }: { reasons: string[] }) {
  return reasons.length > 0 ? <small data-slot="attention" className="text-warning flex min-w-0 items-center gap-1 text-xs"><AlertTriangleIcon className="size-3 flex-none" aria-hidden="true" /><span className="truncate">{reasons.join(' · ')}</span></small> : null
}

function Chips({ items }: { items: { text: string; title?: string }[] }) {
  return items.length > 0 ? <div data-slot="chips" className="-mt-1 box-content flex h-5 min-w-0 gap-1 overflow-hidden px-3.5 pb-3 text-xs [&>span]:bg-muted [&>span]:text-muted-foreground [&>span]:max-w-40 [&>span]:shrink-0 [&>span]:truncate [&>span]:rounded-full [&>span]:px-2 [&>span]:leading-5">
    {items.map(item => <span key={item.text} title={item.title}>{item.text}</span>)}
  </div> : null
}

const UserCard = memo(function UserCard({ entry: { item, view } }: { entry: UserEntry }) {
  const stopped = !item.allowed
  const ring = view.usage === null ? '不限流量' : `本期已用 ${Math.round(view.usage)}%${view.elapsed === null || stopped ? '' : `，周期已过 ${Math.round(view.elapsed)}%`}`
  return <a data-slot="subscription-card" data-kind="users" data-stopped={stopped || undefined} href={`#/plugins/sing-box/users/${item.id}`} aria-label={`${item.name}，${view.status.label}，查看代理用户`}
    className={cn(surface, cardLink, stopped && 'bg-muted/30')}>
    <CardHeader tone={view.status.tone} name={item.name} status={view.status} />
    <div className="flex min-w-0 items-center gap-3 px-3.5 pb-2.5">
      <ProgressRing value={view.usage} marker={stopped ? null : view.elapsed} tone={stopped ? 'muted' : view.usageTone} label={ring}><span className="text-[13px] font-semibold">{view.usage === null ? '∞' : `${Math.min(999, Math.round(view.usage))}%`}</span></ProgressRing>
      <div className="grid min-w-0 flex-1 gap-0.5">
        <div className="flex min-w-0 items-baseline gap-1.5 whitespace-nowrap">
          <strong className="text-base leading-tight font-semibold tabular-nums">{bytes(item.used_bytes)}</strong>
          <span className="text-muted-foreground truncate text-xs tabular-nums">/ {item.monthly_bytes ? bytes(item.monthly_bytes) : '不限量'}</span>
        </div>
        <small className="text-muted-foreground truncate text-xs" title={item.package_name ?? undefined}>{item.package_name ?? '未分配套餐'}{!stopped && view.elapsed !== null ? ` · 周期已过 ${Math.round(view.elapsed)}%` : ''}</small>
        <Reasons reasons={view.reasons} />
      </div>
    </div>
    <div className="grid grid-cols-3 gap-1.5 px-3.5 pb-3">
      <Datum icon={RotateCcwIcon} label="重置" value={view.reset.label} tone="muted" title={view.reset.title} />
      <Datum icon={CalendarIcon} label="到期" value={view.expiry.label} tone={view.expiry.tone} title={view.expiry.title} />
      <Datum icon={DownloadIcon} label="最近拉取" value={view.fetch.label} tone={view.fetch.tone} title={view.fetch.title} />
    </div>
    <Chips items={[...(item.bridged ? [{ text: '原系统', title: '订阅仍由原系统生成，用量由兼容桥接上报' }] : []), ...item.groups.map(text => ({ text }))]} />
  </a>
})

const RenewalCard = memo(function RenewalCard({ entry: { item, view }, base }: { entry: RenewalEntry; base?: string }) {
  const ring = item.inactive ? '已停用' : view.days === null ? `下次续费 ${item.next_payment}` : view.days < 0 ? `续费日已过 ${-view.days} 天` : `本期已过 ${Math.round(view.progress ?? 0)}%，${view.days} 天后续费`
  return <a data-slot="subscription-card" data-kind="renewals" data-stopped={item.inactive || undefined} href={`#/plugins/expenses/items/${item.id}`} aria-label={`${item.name}，${view.status.label}，查看费用与续费`}
    className={cn(surface, cardLink, item.inactive && 'bg-muted/30')}>
    <CardHeader tone={view.status.tone} name={item.name} status={view.status} />
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
          {view.monthly !== null && base ? `约 ${money(view.monthly, base)} / 月` : base && item.currency !== base ? '缺少汇率' : view.cycle}{view.category ? ` · ${view.category}` : ''}
        </small>
        <Reasons reasons={view.reasons} />
      </div>
    </div>
    <div className="grid grid-cols-3 gap-1.5 px-3.5 pb-3">
      <Datum icon={CalendarIcon} label="下次续费" value={view.next.label} tone={view.next.tone} title={view.next.title} />
      <Datum icon={RefreshCcwIcon} label="续费方式" value={item.inactive ? '—' : view.renewal} tone="muted" title={item.auto_renew ? '服务商自动扣款；到日后自动顺延到下一期' : '需要手动续费，并在「费用与续费」确认'} />
      <Datum icon={CreditCardIcon} label="付款" value={view.payment || '—'} tone="muted" title={view.payment ? `付款方式：${view.payment}` : '未设置付款方式'} />
    </div>
    {item.notes && <p className="text-muted-foreground -mt-1 truncate px-3.5 pb-3 text-xs" title={item.notes}>{item.notes}</p>}
  </a>
})

function SourceTile({ entry: { item: card, view } }: { entry: SourceEntry }) {
  return <a data-slot="source-card" href="#/plugins/sing-box/nodes?view=sources" aria-label={`订阅来源 ${card.name}，${card.label}`} className={cn(surface, cardLink, 'gap-2 px-3.5 py-2.5')}>
    <div className="flex min-w-0 items-center gap-2">
      <span className={cn('size-2 flex-none rounded-full', toneFill[card.tone])} aria-hidden="true" />
      <strong data-slot="card-name" className="min-w-0 flex-1 truncate text-sm font-semibold" title={card.name}>{card.name}</strong>
      <span className={cn('flex-none text-xs', statusText(card.tone))}>{card.label}</span>
    </div>
    {card.host && <small className="text-muted-foreground -mt-1.5 truncate text-xs">{card.host}</small>}
    <div className="grid gap-1">
      <div className="flex items-baseline justify-between gap-2 text-xs tabular-nums"><span className="text-muted-foreground">提供方用量</span><span>{card.used === null ? '未上报' : bytes(card.used)} / {card.total ? bytes(card.total) : '未知额度'}</span></div>
      <Track value={view.usage} tone={view.usage !== null && view.usage >= 90 ? 'danger' : view.usage !== null && view.usage >= 75 ? 'warning' : 'info'} />
    </div>
    <div className="text-muted-foreground flex flex-wrap gap-x-3 gap-y-0.5 text-xs">
      <span>{card.supported} 个可用节点</span>
      {view.expiry && <span className={valueTone(view.expiry.tone)}>{view.expiry.label}</span>}
      <span title={card.lastSuccess ? fullTime(card.lastSuccess) : undefined}>{card.lastSuccess ? `${ago(card.lastSuccess)}更新` : '尚未成功更新'}</span>
    </div>
    {card.error && <small className="text-warning line-clamp-2 text-xs wrap-anywhere">{card.error}</small>}
  </a>
}

const nameCell = (href: string, tone: Tone, label: string, name: string) => <a href={href} className="flex min-w-0 items-center gap-2 font-medium hover:underline"><span className={cn('size-2 flex-none rounded-full', toneFill[tone])} title={label} />{name}</a>

function UserTable({ entries }: { entries: UserEntry[] }) {
  return <div className={cn(surface, 'max-w-full min-w-0 overflow-hidden')}><Table>
    <TableHeader><TableRow><TableHead>代理用户</TableHead><TableHead>套餐</TableHead><TableHead className="min-w-44">本期用量</TableHead><TableHead>重置</TableHead><TableHead>到期</TableHead><TableHead>最近拉取</TableHead><TableHead>策略组</TableHead></TableRow></TableHeader>
    <TableBody>{entries.map(({ key, item, view }) => <TableRow key={key} data-stopped={!item.allowed || undefined}>
      <TableCell>{nameCell(`#/plugins/sing-box/users/${item.id}`, view.status.tone, view.status.label, item.name)}</TableCell>
      <TableCell><span className="block">{item.package_name ?? '未分配套餐'}</span><small className={cn('text-xs', statusText(view.status.tone))}>{view.status.label}</small></TableCell>
      <TableCell><span className="block text-xs tabular-nums">{bytes(item.used_bytes)} / {item.monthly_bytes ? bytes(item.monthly_bytes) : '不限量'}{view.usage !== null ? ` · ${Math.round(view.usage)}%` : ''}</span>{view.usage !== null && <Track value={view.usage} tone={item.allowed ? view.usageTone : 'muted'} />}{view.reasons.length > 0 && <small className="text-warning text-xs">{view.reasons.join(' · ')}</small>}</TableCell>
      <TableCell title={view.reset.title} className="text-xs whitespace-nowrap">{view.reset.label}</TableCell>
      <TableCell title={view.expiry.title} className={cn('text-xs whitespace-nowrap', valueTone(view.expiry.tone))}>{view.expiry.label}</TableCell>
      <TableCell title={view.fetch.title} className={cn('text-xs whitespace-nowrap', valueTone(view.fetch.tone))}>{view.fetch.label}{view.fetch.format && item.last_fetch_at !== null ? ` · ${view.fetch.format}` : ''}</TableCell>
      <TableCell className="text-muted-foreground max-w-56 truncate text-xs" title={item.groups.join('、')}>{[item.bridged ? '原系统' : '', ...item.groups].filter(Boolean).join('、') || '—'}</TableCell>
    </TableRow>)}</TableBody>
  </Table></div>
}

function RenewalTable({ entries, base }: { entries: RenewalEntry[]; base?: string }) {
  return <div className={cn(surface, 'max-w-full min-w-0 overflow-hidden')}><Table>
    <TableHeader><TableRow><TableHead>续费项目</TableHead><TableHead>费用 / 周期</TableHead><TableHead>折合每月</TableHead><TableHead>下次续费</TableHead><TableHead>续费方式</TableHead><TableHead>分类 / 付款</TableHead></TableRow></TableHeader>
    <TableBody>{entries.map(({ key, item, view }) => <TableRow key={key} data-stopped={item.inactive || undefined}>
      <TableCell>{nameCell(`#/plugins/expenses/items/${item.id}`, view.status.tone, view.status.label, item.name)}{view.reasons.length > 0 && <small className="text-warning block text-xs">{view.reasons.join(' · ')}</small>}</TableCell>
      <TableCell className="text-xs whitespace-nowrap tabular-nums">{view.price}<small className="text-muted-foreground block">{view.cycle}</small></TableCell>
      <TableCell className="text-xs whitespace-nowrap tabular-nums">{view.monthly !== null && base ? money(view.monthly, base) : '—'}</TableCell>
      <TableCell title={view.next.title} className={cn('text-xs whitespace-nowrap', valueTone(view.next.tone))}>{item.inactive ? '已停用' : item.next_payment}{!item.inactive && <small className="block">{view.next.label}</small>}</TableCell>
      <TableCell className="text-xs whitespace-nowrap">{item.inactive ? '—' : `${view.renewal}续费`}</TableCell>
      <TableCell className="text-muted-foreground max-w-56 truncate text-xs">{[view.category, view.payment].filter(Boolean).join(' · ') || '—'}</TableCell>
    </TableRow>)}</TableBody>
  </Table></div>
}

function SourceTable({ entries }: { entries: SourceEntry[] }) {
  return <div className={cn(surface, 'max-w-full min-w-0 overflow-hidden')}><Table>
    <TableHeader><TableRow><TableHead>订阅来源</TableHead><TableHead>状态</TableHead><TableHead className="min-w-44">提供方用量</TableHead><TableHead>到期</TableHead><TableHead>可用节点</TableHead><TableHead>最近更新</TableHead></TableRow></TableHeader>
    <TableBody>{entries.map(({ key, item, view }) => <TableRow key={key}>
      <TableCell>{nameCell('#/plugins/sing-box/nodes?view=sources', item.tone, item.label, item.name)}{item.host && <small className="text-muted-foreground block text-xs">{item.host}</small>}</TableCell>
      <TableCell className={cn('text-xs', statusText(item.tone))}>{item.label}{item.error && <small className="text-warning block max-w-56 truncate" title={item.error}>{item.error}</small>}</TableCell>
      <TableCell><span className="block text-xs tabular-nums">{item.used === null ? '未上报' : bytes(item.used)} / {item.total ? bytes(item.total) : '未知额度'}</span>{view.usage !== null && <Track value={view.usage} tone={view.usage >= 90 ? 'danger' : view.usage >= 75 ? 'warning' : 'info'} />}</TableCell>
      <TableCell className={cn('text-xs whitespace-nowrap', view.expiry ? valueTone(view.expiry.tone) : 'text-muted-foreground')}>{view.expiry?.label ?? '未上报'}</TableCell>
      <TableCell className="text-xs tabular-nums">{item.supported}</TableCell>
      <TableCell className="text-muted-foreground text-xs whitespace-nowrap" title={item.lastSuccess ? fullTime(item.lastSuccess) : undefined}>{item.lastSuccess ? ago(item.lastSuccess) : '尚未成功'}</TableCell>
    </TableRow>)}</TableBody>
  </Table></div>
}

/** May this administrator read a panel-wide feature? The same rule as the sidebar; the panel still checks every read. */
const mayRead = (actor: ShellActor | undefined, feature: string) => actor?.role === 'owner' || Boolean(actor?.all_servers && actor.capabilities.includes(`${feature}:read`))

export default function SubscriptionBoard() {
  const clock = useNow(30_000)
  const now = Math.floor(clock / 1000)
  const me = useResource<ShellActor>('/api/control-center/me', 30_000, { keep: true })
  const [remembered] = useState(readShellActor)
  const actor = me.data ?? remembered
  const proxy = mayRead(actor, 'proxy'), cloud = mayRead(actor, 'cloud')

  const board = useResource<unknown>(proxy ? `${singbox}/subscription-board` : null, 30_000, { keep: true })
  const migration = useResource<unknown>(proxy ? sourceMigrationPath : null, 300_000, { keep: true })
  const migrated = validSourceMigration(migration.data) ? migration.data.migrated : undefined
  const ordered = useResource<unknown>(proxy ? sourceRoot : null, 60_000, { keep: true })
  // Before the source migration the numbered sources still feed the node catalog.
  const numbered = useResource<unknown>(proxy && migrated === false ? `${singbox}/subscription-sources` : null, 60_000, { keep: true })
  const renewals = useResource<unknown>(cloud ? `${expenses}/subscriptions` : null, 60_000, { keep: true })
  const labels = useResource<unknown>(cloud ? `${expenses}/labels` : null, 300_000, { keep: true })
  const summary = useResource<unknown>(cloud ? `${expenses}/summary` : null, 300_000, { keep: true })

  const [view, setView] = useState<View>(() => readPreference('view') === 'table' ? 'table' : 'cards')
  const [sort, setSort] = useState<BoardSort>(() => savedSort(readPreference('sort')))
  const [kind, setKind] = useState<BoardKind | 'all'>('all')
  const [filter, setFilter] = useState<BoardFilter>('all')
  const [query, setQuery] = useState('')
  const [optionsOpen, setOptionsOpen] = useState(false)

  const users = validUserBoard(board.data) ? board.data : undefined
  const items = validRenewals(renewals.data) ? renewals.data : undefined
  const ledger = validRenewalSummary(summary.data) ? summary.data : undefined
  const names = validLabels(labels.data) ? labels.data.entries : undefined
  const malformed = [board.data !== undefined && !users ? '代理用户订阅' : '', renewals.data !== undefined && !items ? '续费项目' : ''].filter(Boolean)
  const sourceCards = useMemo<SourceCard[]>(() => [
    ...(migrated === false && validNumberedSources(numbered.data) ? numbered.data.filter(source => !source.archived).map(numberedSourceCard) : []),
    ...(validOrderedSources(ordered.data) ? ordered.data.filter(source => !source.archived).map(orderedSourceCard) : []),
  ], [migrated, numbered.data, ordered.data])

  const entries = useMemo<Entry[]>(() => [
    ...(users?.subscriptions.map(item => ({ kind: 'users' as const, key: `user-${item.id}`, name: item.name, search: [item.name, item.package_name ?? '', ...item.groups], item, view: userView(item, now, users.fetches_recorded, users.fetch_retention_days) })) ?? []),
    ...(items?.map(item => {
      const result = renewalView(item, ledger?.from, ledger?.base_currency, ledger?.exchange, names ?? [])
      return { kind: 'renewals' as const, key: `renewal-${item.id}`, name: item.name, search: [item.name, result.category, result.payment, item.notes], item, view: result }
    }) ?? []),
    ...sourceCards.map(card => ({ kind: 'sources' as const, key: card.key, name: card.name, search: [card.name, card.host ?? ''], item: card, view: sourceView(card, now) })),
  ], [users, items, ledger, names, sourceCards, now])
  const available = kinds.filter(entry => entry.kind === 'users' ? proxy : entry.kind === 'renewals' ? cloud : sourceCards.length > 0)
  const shownKind = kind !== 'all' && !available.some(entry => entry.kind === kind) ? 'all' : kind
  const scoped = useMemo(() => entries.filter(entry => shownKind === 'all' || entry.kind === shownKind), [entries, shownKind])
  const visible = useMemo(() => selectEntries(entries, shownKind, query, filter, sort), [entries, shownKind, query, filter, sort])
  const counts = boardCounts(entries), scopedCounts = boardCounts(scoped)
  // Until the administrator is known nothing can be read yet, so the board is still loading.
  const waiting = !actor || (proxy && board.data === undefined && !board.error) || (cloud && renewals.data === undefined && !renewals.error)

  const base = ledger?.base_currency
  const budget = ledger?.budget_units ? Number(ledger.budget_units) / 10000 : null
  const recorded = users?.fetches_recorded
  const overBudget = Boolean(items && base && budget !== null && counts.renewals.monthly > budget)
  const attentionParts = [
    counts.users.runningOut && `${counts.users.runningOut} 个流量将尽`, counts.users.expiring && `${counts.users.expiring} 个将到期`,
    counts.renewals.attention && `${counts.renewals.attention} 项待续费`, counts.sources.attention && `${counts.sources.attention} 个来源异常`,
  ].filter(Boolean)
  const stats: ReactNode[] = [
    proxy && <Stat key="users" icon={UsersIcon} label="可用订阅" value={users ? counts.users.usable : '—'} unit={users ? `/ ${counts.users.all} 个` : undefined} tone="good" title="可以正常使用的代理用户订阅">{!users ? '等待数据' : !counts.users.all ? '还没有代理用户' : counts.users.stopped ? `${counts.users.stopped} 个已停用` : '全部可用'}</Stat>,
    proxy && <Stat key="traffic" icon={DatabaseIcon} label="本期用量" value={users ? bytes(counts.users.used) : '—'} title="各代理用户当前周期的已用流量合计">{users ? counts.users.metered ? `${counts.users.metered} 个限量订阅共 ${bytes(counts.users.quota)}` : '没有限量订阅' : '等待数据'}</Stat>,
    cloud && <Stat key="cost" icon={WalletIcon} label="每月费用" value={items && base ? money(counts.renewals.monthly, base) : '—'} tone={overBudget ? 'warning' : undefined}
      title={`使用中的续费项目按周期折合到每月${base ? `，以 ${base} 计` : ''}${budget !== null && base ? `；月预算 ${money(budget, base)}` : ''}。服务器的费用在服务器看板。`}>
      {!items ? '等待数据' : counts.renewals.unconverted ? <span className="text-warning">{counts.renewals.unconverted} 项缺少汇率，未计入</span>
        : overBudget && base ? <span className="text-warning">超出月预算 {money(counts.renewals.monthly - budget!, base)}</span> : `${counts.renewals.active} 项使用中 · ${counts.renewals.month} 项 30 天内续费`}
    </Stat>,
    <Stat key="attention" icon={AlertTriangleIcon} label="需关注" value={waiting && !entries.length ? '—' : counts.attention} tone={counts.attention ? 'warning' : undefined} title={attentionParts.length ? attentionParts.join('；') : '流量将尽或按当前速度会在重置前用完、7 天内到期或续费、续费日已过、订阅来源更新失败'}>{waiting && !entries.length ? '等待数据' : attentionParts.length ? attentionParts.join(' · ') : '没有需要处理的订阅'}</Stat>,
    proxy && recorded !== false && <Stat key="fetches" icon={DownloadIcon} label="24 小时拉取" value={users ? counts.users.fetched : '—'} unit={users ? `/ ${counts.users.all} 个` : undefined} title="经 apsp 地址拉取过订阅的代理用户；仍使用原系统地址的客户端不在此统计">{users ? `共 ${counts.users.fetches} 次请求` : '等待数据'}</Stat>,
  ].filter(Boolean)

  const refresh = () => { for (const resource of [board, ordered, numbered, migration, renewals, labels, summary]) resource.reload() }
  const reset = () => { setQuery(''); setFilter('all') }
  const changed = Boolean(query) || filter !== 'all'
  const customised = filter !== 'all' || sort !== 'attention' || view !== 'cards'
  const chooseView = (value: View) => { setView(value); savePreference('view', value) }
  const chooseSort = (value: string) => { const next = savedSort(value); setSort(next); savePreference('sort', next) }
  const sections = available.filter(entry => shownKind === 'all' || entry.kind === shownKind)
  const ofKind = <K extends BoardKind>(target: K) => visible.filter((entry): entry is Extract<Entry, { kind: K }> => entry.kind === target)

  return <div data-slot="subscription-board" className="flex flex-col gap-3">
    <PageHeader title="订阅看板" description="代理用户订阅、续费项目与订阅来源：一处看用量、续费与到期。"><Refresh onClick={refresh} /></PageHeader>
    <ErrorNotice message={board.error} retry={board.reload} />
    <ErrorNotice message={renewals.error} retry={renewals.reload} />
    <ErrorNotice message={malformed.length ? `${malformed.join('与')}的数据不完整，请刷新重试。` : ''} retry={refresh} />
    {stats.length > 1 && <StatStrip label="订阅总览" columns={Math.min(5, Math.max(2, stats.length)) as 2 | 3 | 4 | 5}>{stats}</StatStrip>}

    <div data-slot="subscription-toolbar" className="min-w-0">
      <div className="flex min-h-9 items-center gap-3 max-lg:flex-wrap max-lg:gap-2">
        <label className="relative flex min-w-44 flex-[0_1_300px] items-center max-lg:flex-[1_1_180px] max-sm:min-w-24 max-sm:flex-[1_1_100px]">
          <SearchIcon className="text-muted-foreground pointer-events-none absolute left-2.5 size-4" aria-hidden="true" />
          <Input type="search" aria-label="搜索订阅" placeholder="搜索名称、套餐、分类" value={query} onChange={event => setQuery(event.target.value)} className="pl-8" />
        </label>
        {available.length > 1 && <SegmentedGroup aria-label="订阅类型" className="no-scrollbar max-w-full flex-nowrap overflow-x-auto *:shrink-0 max-lg:order-3 max-lg:flex-[1_1_100%]">
          <SegmentedButton aria-pressed={shownKind === 'all'} onClick={() => setKind('all')}>全部<span data-slot="filter-count" className="text-muted-foreground text-xs tabular-nums" aria-hidden="true">{counts.all}</span></SegmentedButton>
          {available.map(entry => <SegmentedButton key={entry.kind} aria-pressed={shownKind === entry.kind} onClick={() => setKind(entry.kind)}>{entry.label}<span data-slot="filter-count" className="text-muted-foreground text-xs tabular-nums" aria-hidden="true">{entries.filter(item => item.kind === entry.kind).length}</span></SegmentedButton>)}
        </SegmentedGroup>}
        <div className="ml-auto flex flex-none items-center gap-1">
          <span className="text-muted-foreground mr-1.5 text-xs whitespace-nowrap">{waiting && !entries.length ? '等待数据' : `${visible.length} 项`}</span>
          <Button variant="ghost" size="icon" className={cn(customised && 'bg-primary/10 text-primary')} aria-label="筛选与视图" title="筛选与视图" aria-expanded={optionsOpen} aria-controls="subscription-board-options" onClick={() => setOptionsOpen(value => !value)}><ListFilterIcon /></Button>
        </div>
      </div>
      <AnimatePresence initial={false}>
        {optionsOpen && <m.div key="options" {...revealMotion} className="overflow-hidden">
          <div id="subscription-board-options" className="mt-3 flex flex-wrap items-center gap-3 border-t pt-3">
            <SegmentedGroup aria-label="订阅状态筛选" className="max-w-full max-sm:flex-[1_1_100%] max-sm:*:flex-1">
              {filters.map(([value, label]) => <SegmentedButton key={value} aria-label={label} aria-pressed={filter === value} onClick={() => setFilter(value)}>{label}<span data-slot="filter-count" className="text-muted-foreground text-xs tabular-nums" aria-hidden="true">{scopedCounts[value]}</span></SegmentedButton>)}
            </SegmentedGroup>
            <label className="text-muted-foreground flex items-center gap-2 text-xs max-sm:flex-auto max-sm:justify-between">排序<NativeSelect size="sm" aria-label="排序" value={sort} onChange={event => chooseSort(event.target.value)} className="max-w-44"><option value="attention">需关注的优先</option><option value="usage">用量或费用高的优先</option><option value="expiry">先到期的优先</option><option value="fetch">最近拉取的优先</option><option value="name">按名称</option></NativeSelect></label>
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

    {waiting && !entries.length ? <CardsLoading />
      : !visible.length && changed ? <div className="text-muted-foreground flex min-h-60 flex-col items-center justify-center gap-3 p-6 text-center"><RssIcon className="size-8" aria-hidden="true" /><strong className="text-foreground text-sm font-medium">没有匹配的订阅</strong><Button onClick={reset}>清除筛选</Button></div>
        : sections.map(({ kind: section, label, icon, manage }) => {
          const total = entries.filter(entry => entry.kind === section).length
          const shown = section === 'users' ? ofKind('users') : section === 'renewals' ? ofKind('renewals') : ofKind('sources')
          // While filtering, sections without a match step aside.
          if (changed && !shown.length) return null
          return <section key={section} data-kind={section} aria-label={label} className="grid gap-3 not-first:mt-2">
            {sections.length > 1 || shownKind !== 'all' ? <KindHeading icon={icon} title={label} count={total ? shown.length === total ? `${total}` : `${shown.length} / ${total}` : ''} manage={manage} /> : null}
            {!total ? <EmptySection kind={section} waiting={section === 'users' ? board.data === undefined && !board.error : section === 'renewals' ? renewals.data === undefined && !renewals.error : false} />
              : section === 'users' ? (view === 'cards' ? <div className={grid}>{(shown as UserEntry[]).map(entry => <div key={entry.key} className="min-w-0"><UserCard entry={entry} /></div>)}</div> : <UserTable entries={shown as UserEntry[]} />)
                : section === 'renewals' ? (view === 'cards' ? <div className={grid}>{(shown as RenewalEntry[]).map(entry => <div key={entry.key} className="min-w-0"><RenewalCard entry={entry} base={base} /></div>)}</div> : <RenewalTable entries={shown as RenewalEntry[]} base={base} />)
                  : view === 'cards' ? <div className="grid grid-cols-[repeat(auto-fill,minmax(260px,1fr))] items-start gap-3 max-sm:grid-cols-1">{(shown as SourceEntry[]).map(entry => <SourceTile key={entry.key} entry={entry} />)}</div> : <SourceTable entries={shown as SourceEntry[]} />}
          </section>
        })}
    {view === 'cards' && entries.length > 0 && <p className="text-muted-foreground text-xs">圆环：代理用户是本期已用流量，小圆点是周期进度，弧线越过圆点说明流量用得比时间快；续费项目是本期已过的时间，中间是距下次续费的天数。</p>}
  </div>
}

/** A section's name and count, with the page that manages it; one row at every width. */
function KindHeading({ icon: Icon, title, count, manage }: { icon: LucideIcon; title: string; count: string; manage: { href: string; label: string } }) {
  return <div className="flex min-w-0 items-center justify-between gap-3">
    <h2 className="flex min-w-0 items-center gap-2 text-sm font-semibold"><Icon className="text-primary size-4 flex-none" aria-hidden="true" /><span className="truncate">{title}</span>{count && <span className="text-muted-foreground text-xs font-normal tabular-nums">{count}</span>}</h2>
    <a className={buttonVariants({ variant: 'ghost', size: 'xs' })} href={manage.href}>{manage.label}</a>
  </div>
}

function EmptySection({ kind, waiting }: { kind: BoardKind; waiting: boolean }) {
  if (waiting) return <CardsLoading count={2} />
  const [title, href, action] = kind === 'users' ? ['还没有代理用户，创建后每个人的订阅会出现在这里。', '#/plugins/sing-box/users', '前往代理用户']
    : kind === 'renewals' ? ['还没有续费项目。添加域名、AI 等周期付费的服务，或从 Wallos 导入。', '#/plugins/expenses', '添加续费项目'] : ['还没有订阅来源。', '#/plugins/sing-box/nodes?view=sources', '添加订阅来源']
  return <div className={cn(surface, 'text-muted-foreground flex flex-wrap items-center justify-between gap-3 px-4 py-3.5 text-[13px]')}><span>{title}</span><a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={href}>{action}</a></div>
}

/** First read: card outlines in the same grid as the subscriptions to come. */
function CardsLoading({ count = 3 }: { count?: number }) {
  return <div data-slot="loading" role="status" className={grid}>
    <span className="sr-only">正在读取订阅…</span>
    {Array.from({ length: count }, (_, index) => <div key={index} aria-hidden="true" className={cn(surface, 'grid gap-3 p-3.5')}>
      <Skeleton className="h-4 w-1/2" />
      <div className="flex items-center gap-3"><Skeleton className="size-14 rounded-full" /><div className="grid flex-1 gap-1.5"><Skeleton className="h-4 w-2/3" /><Skeleton className="h-3 w-1/2" /></div></div>
      <div className="grid grid-cols-3 gap-1.5">{[0, 1, 2].map(cell => <Skeleton key={cell} className="h-9" />)}</div>
    </div>)}
  </div>
}
