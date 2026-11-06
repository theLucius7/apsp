// The subscription board (ADR 0117): every subscription in one place, as cards.
// Three kinds share it: proxy users' subscriptions (the sing-box plugin's
// /subscription-board, which carries no addresses or credentials), recurring
// renewal items from the costs and renewals ledger (Wallos style), and the
// subscription sources the panel reads nodes from. Each kind comes from its own
// plugin's endpoints; a kind the administrator may not read stays off the board.
import type { SubscriptionSource as OrderedSource } from '../plugins/singbox/orderedSourceTypes'
import type { SubscriptionSource as NumberedSource } from '../plugins/singbox/sourceTypes'
import { sourceReason, sourceStatus } from '../plugins/singbox/sourceTypes'
import { amount, cycleDays, cycles, daysUntil, monthlyAmount } from '../plugins/expenses/types'
import type { Expense, Label } from '../plugins/expenses/types'
import { convert } from '../display/currency'
import type { ExchangeRates } from '../display/currency'
import { ago } from '../format'

export type Tone = 'good' | 'warning' | 'danger' | 'muted' | 'info'
export type BoardKind = 'users' | 'renewals' | 'sources'
export type BoardFilter = 'all' | 'usable' | 'attention' | 'stopped'
export type BoardSort = 'attention' | 'usage' | 'expiry' | 'fetch' | 'name'

const DAY = 86_400
/** Expiring or renewing within this many days needs attention. */
export const SOON_DAYS = 7
/** Share of a quota from which a subscription is about to run out. */
export const USAGE_WARNING_PERCENT = 90

const day = (seconds: number) => new Date(seconds * 1000).toLocaleDateString('zh-CN', { month: 'numeric', day: 'numeric' })
export const fullTime = (seconds: number) => new Date(seconds * 1000).toLocaleString('zh-CN', { hour12: false })
const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value)
const integer = (value: unknown) => typeof value === 'number' && Number.isSafeInteger(value)
const nullableInteger = (value: unknown) => value === null || integer(value)
const nullableText = (value: unknown) => value === null || typeof value === 'string'
const counter = (value: unknown) => typeof value === 'string' && /^\d{1,20}$/.test(value)

/** What every card answers the same way, whatever its kind. */
export type Common = {
  status: { label: string; tone: Tone }
  /** Usable now: the subscription works, the renewal item is active, the source is not failing. */
  usable: boolean
  /** Why it needs looking at; empty when it does not. */
  reasons: string[]
  /** For "highest first": the share of a quota used, or a renewal item's monthly cost. */
  weight: number | null
  /** For "soonest first": when it expires or renews, in seconds. */
  due: number | null
}

// Proxy users' subscriptions.

export type UserSubscription = {
  id: number; name: string; status: string; allowed: boolean
  package_name: string | null; monthly_bytes: string | null; used_bytes: string
  starts_at: number | null; expires_at: number | null; cycle_start: number | null; next_reset: number | null
  groups: string[]; bridged: boolean
  last_fetch_at: number | null; last_fetch_format: string | null; last_fetch_result: string | null; fetches_today: number
}
export type UserBoard = { now: number; fetches_recorded: boolean; fetch_retention_days: number; subscriptions: UserSubscription[] }

export function validUserBoard(value: unknown): value is UserBoard {
  return record(value) && integer(value.now) && typeof value.fetches_recorded === 'boolean' && integer(value.fetch_retention_days) && Array.isArray(value.subscriptions)
    && value.subscriptions.every(item => record(item) && integer(item.id) && typeof item.name === 'string' && typeof item.status === 'string' && typeof item.allowed === 'boolean'
      && nullableText(item.package_name) && (item.monthly_bytes === null || counter(item.monthly_bytes)) && counter(item.used_bytes)
      && ['starts_at', 'expires_at', 'cycle_start', 'next_reset', 'last_fetch_at'].every(key => nullableInteger(item[key]))
      && Array.isArray(item.groups) && item.groups.every(group => typeof group === 'string') && typeof item.bridged === 'boolean'
      && nullableText(item.last_fetch_format) && nullableText(item.last_fetch_result) && integer(item.fetches_today))
}

const statusLabels: Record<string, [string, Tone]> = {
  active: ['使用中', 'good'], unmetered: ['不限流量', 'good'], not_started: ['尚未生效', 'info'], expired: ['已到期', 'danger'],
  exhausted: ['流量已用完', 'danger'], no_package: ['未分配套餐', 'muted'], pending: ['等待开通', 'muted'], suspended: ['已暂停', 'muted'],
  archived: ['已归档', 'muted'], staged: ['等待导入', 'muted'],
}
export function userStatus(subscription: Pick<UserSubscription, 'status' | 'allowed'>): { label: string; tone: Tone } {
  const [label, tone] = statusLabels[subscription.status] ?? [subscription.allowed ? '可用' : '不可用', subscription.allowed ? 'good' : 'muted']
  return { label, tone }
}

const formatLabels: Record<string, string> = { clash: 'Clash', singbox: 'sing-box', links: '分享链接', renderer: '原系统生成' }
export const fetchFormat = (format: string | null) => format ? formatLabels[format] ?? format : ''

export type UserView = Common & {
  /** Share of this cycle's quota used, or null without a quota. */
  usage: number | null
  usageTone: Tone
  /** Share of the current cycle that has passed, or null when the cycle is unknown. */
  elapsed: number | null
  /** When the quota runs out at the current pace, if that is before the cycle ends. */
  runsOutAt: number | null
  /** Short values for labelled cells; the full dates are in the titles. */
  expiry: { label: string; title: string; tone: Tone; days: number | null }
  reset: { label: string; title: string }
  fetch: { label: string; title: string; tone: Tone; format: string }
}

export function userView(item: UserSubscription, now: number, recorded: boolean, retention: number): UserView {
  const status = userStatus(item)
  const usage = item.monthly_bytes && Number(item.monthly_bytes) > 0 ? Number(item.used_bytes) / Number(item.monthly_bytes) * 100 : null
  const usageTone: Tone = usage === null ? 'info' : usage >= USAGE_WARNING_PERCENT ? 'danger' : usage >= 75 ? 'warning' : 'good'
  // The bridge reports 0 for a cycle start it does not know.
  const start = item.cycle_start && item.cycle_start > 0 ? item.cycle_start : null
  const cycle = start !== null && item.next_reset !== null && item.next_reset > start ? item.next_reset - start : null
  const elapsed = cycle === null ? null : Math.min(100, Math.max(0, (now - start!) / cycle * 100))
  let runsOutAt: number | null = null
  if (item.allowed && item.monthly_bytes && start !== null && now > start) {
    const used = Number(item.used_bytes), limit = Number(item.monthly_bytes)
    const rate = used / (now - start)
    if (rate > 0 && used < limit) {
      const at = now + (limit - used) / rate
      const end = Math.min(item.next_reset ?? Infinity, item.expires_at ?? Infinity)
      // Early in a cycle one busy hour says little about the month.
      if (at < end && (elapsed === null || elapsed >= 10)) runsOutAt = Math.floor(at)
    }
  }

  const days = item.expires_at === null ? null : Math.ceil((item.expires_at - now) / DAY)
  const starting = item.starts_at !== null && item.starts_at > now ? `；${fullTime(item.starts_at)} 生效` : ''
  const expiry = item.expires_at === null
    ? { label: '长期有效', title: `套餐没有到期时间${starting}`, tone: 'muted' as Tone, days }
    : item.expires_at <= now ? { label: '已到期', title: `${fullTime(item.expires_at)} 到期`, tone: 'danger' as Tone, days }
      : { label: days! <= SOON_DAYS ? `${days} 天后` : day(item.expires_at), title: `${fullTime(item.expires_at)} 到期，还剩 ${days} 天${starting}`, tone: (days! <= SOON_DAYS ? 'warning' : 'muted') as Tone, days }

  const reset = item.next_reset !== null
    ? { label: day(item.next_reset), title: `下次重置 ${fullTime(item.next_reset)}${start !== null ? `，本期自 ${fullTime(start)} 起` : ''}` }
    : { label: item.monthly_bytes ? '不重置' : '—', title: item.monthly_bytes ? '套餐流量不按周期重置' : '没有流量额度' }

  const format = fetchFormat(item.last_fetch_format)
  const fetch = !recorded ? { label: '未记录', title: '面板设置中关闭了订阅拉取记录', tone: 'muted' as Tone, format }
    : item.last_fetch_at === null ? { label: '无记录', title: `最近 ${retention} 天没有经过 apsp 的拉取；仍使用原系统地址的客户端不会记录在这里`, tone: 'muted' as Tone, format }
      : { label: ago(item.last_fetch_at, now), format,
        title: `最近拉取 ${fullTime(item.last_fetch_at)}${format ? `（${format}）` : ''}${item.fetches_today ? `；24 小时内 ${item.fetches_today} 次` : ''}${item.last_fetch_result === 'restricted' ? '；套餐不可用，未下发节点' : item.last_fetch_result === 'unavailable' ? '；订阅暂时不可用' : ''}`,
        tone: (item.last_fetch_result === 'restricted' || item.last_fetch_result === 'unavailable' ? 'warning' : 'muted') as Tone }

  const reasons: string[] = []
  if (item.allowed) {
    if (usage !== null && usage >= USAGE_WARNING_PERCENT) reasons.push('流量即将用完')
    else if (runsOutAt !== null) reasons.push(`按当前速度 ${day(runsOutAt)} 用完`)
    if (days !== null && days > 0 && days <= SOON_DAYS) reasons.push(`${days} 天后到期`)
    if (item.last_fetch_result === 'unavailable') reasons.push('最近一次拉取失败')
  }
  return { status, usable: item.allowed, reasons, weight: usage, due: item.expires_at, usage, usageTone, elapsed, runsOutAt, expiry, reset, fetch }
}

// Renewal items of the costs and renewals ledger.

export type RenewalSummary = { from: string; base_currency: string; budget_units: string | null; exchange: ExchangeRates }

export function validRenewals(value: unknown): value is Expense[] {
  return Array.isArray(value) && value.every(item => record(item) && integer(item.id) && typeof item.name === 'string' && counter(item.amount_units) && typeof item.currency === 'string'
    && typeof item.next_payment === 'string' && ['day', 'week', 'month', 'year'].includes(String(item.cycle)) && integer(item.frequency) && Number(item.frequency) > 0
    && typeof item.inactive === 'boolean' && typeof item.auto_renew === 'boolean' && nullableText(item.cancellation_date ?? null)
    && ['category_id', 'payment_method_id', 'payer_id'].every(key => nullableInteger(item[key] ?? null)))
}
export function validRenewalSummary(value: unknown): value is RenewalSummary {
  return record(value) && typeof value.from === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(value.from) && typeof value.base_currency === 'string' && /^[A-Z]{3}$/.test(value.base_currency)
    && (value.budget_units === null || counter(value.budget_units)) && record(value.exchange)
}
export const validLabels = (value: unknown): value is { entries: Label[] } => record(value) && Array.isArray(value.entries) && value.entries.every(label => record(label) && integer(label.id) && typeof label.name === 'string')

export type RenewalView = Common & {
  price: string
  cycle: string
  /** This item's cost per month in the ledger's base currency, or null without a rate. */
  monthly: number | null
  days: number | null
  /** Share of the current billing period that has passed. */
  progress: number | null
  next: { label: string; title: string; tone: Tone }
  renewal: string
  category: string
  payment: string
}

const ledgerDay = (date: string) => { const [, month, dayOfMonth] = date.split('-').map(Number); return month && dayOfMonth ? `${month}/${dayOfMonth}` : date }

/** `today` is the ledger's own date (its time zone), from the summary; `base` and `rates` convert to its currency. */
export function renewalView(item: Expense, today: string | undefined, base: string | undefined, rates: ExchangeRates | undefined, labels: Label[]): RenewalView {
  const days = daysUntil(item.next_payment, today)
  const period = cycleDays(item)
  const cancelled = item.cancellation_date !== null && item.cancellation_date !== undefined && item.cancellation_date <= item.next_payment
  const status: { label: string; tone: Tone } = item.inactive ? { label: '已停用', tone: 'muted' }
    : days !== null && days < 0 ? { label: '待确认续费', tone: 'danger' }
      : cancelled ? { label: '到期不续', tone: 'info' }
        : days !== null && days <= SOON_DAYS ? { label: days === 0 ? '今天续费' : '即将续费', tone: 'warning' }
          : { label: item.auto_renew ? '自动续费' : '手动续费', tone: 'good' }
  const own = monthlyAmount(item)
  const monthly = base && own !== null ? convert(own, item.currency, base, rates) : null
  const progress = item.inactive || days === null || period === null ? null : Math.min(100, Math.max(0, (1 - days / period) * 100))
  const next = item.inactive ? { label: '—', title: '已停用，不再续费', tone: 'muted' as Tone }
    : { label: days === null ? item.next_payment : days < 0 ? `已过 ${-days} 天` : days === 0 ? '今天' : days <= SOON_DAYS ? `${days} 天后` : ledgerDay(item.next_payment),
      title: `下次续费 ${item.next_payment}${cancelled ? `；已安排 ${item.cancellation_date} 取消` : ''}`, tone: (days !== null && days < 0 ? 'danger' : days !== null && days <= SOON_DAYS ? 'warning' : 'muted') as Tone }
  const label = (id: number | null) => labels.find(entry => entry.id === id)?.name ?? ''
  const reasons: string[] = []
  if (!item.inactive && days !== null) {
    if (days < 0) reasons.push(item.auto_renew ? '续费日已过，等待顺延' : '续费日已过，等待确认')
    else if (days <= SOON_DAYS && !cancelled) reasons.push(days === 0 ? '今天续费' : `${days} 天后续费`)
  }
  const dueAt = Date.parse(`${item.next_payment}T00:00:00Z`) / 1000
  return {
    status, usable: !item.inactive, reasons, weight: monthly ?? own, due: item.inactive || !Number.isFinite(dueAt) ? null : dueAt,
    price: amount(item.amount_units, item.currency), cycle: item.frequency === 1 ? `每${cycles[item.cycle]}` : `每 ${item.frequency} ${cycles[item.cycle]}`,
    monthly, days, progress, next, renewal: item.auto_renew ? '自动' : '手动', category: label(item.category_id), payment: label(item.payment_method_id),
  }
}

// Subscription sources: the airport subscriptions the panel reads nodes from.
// After the source migration only the ordered sources are live; before it the
// numbered ones still feed the node catalog, so the board shows both.

export type SourceCard = {
  key: string; name: string; host: string | null; label: string; tone: Tone; error: string
  used: number | null; total: number | null; expire: number | null; supported: number; lastSuccess: number | null
}
export type SourceView = Common & { usage: number | null; expiry: { label: string; tone: Tone } | null }

const known = (value: number | undefined) => typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : null
function traffic(value: { upload?: number; download?: number; total?: number; expire?: number } | undefined) {
  const upload = known(value?.upload), download = known(value?.download), expire = known(value?.expire)
  return { used: upload !== null && download !== null ? upload + download : null, total: known(value?.total) || null, expire: expire || null }
}

export function orderedSourceCard(source: OrderedSource): SourceCard {
  const [label, tone]: [string, Tone] = source.active_job ? ['正在更新', 'info']
    : source.last_error ? (source.latest_success ? ['更新失败，使用上次版本', 'warning'] : ['导入失败', 'danger'])
      : source.stale_reason ? ['使用上次版本', 'warning']
        : source.counts.supported ? ['正常', 'good'] : ['没有可用节点', 'muted']
  return { key: `ordered-${source.id}`, name: source.name, host: source.kind === 'url' ? source.host : null, label, tone, error: source.last_error?.message ?? '',
    ...traffic(source.traffic), supported: source.counts.supported, lastSuccess: source.last_success_at }
}

export function numberedSourceCard(source: NumberedSource): SourceCard {
  const tone: Tone = source.active_job_id ? 'info' : source.last_error ? (source.current_revision_id ? 'warning' : 'danger') : source.supported_count ? 'good' : 'muted'
  return { key: `numbered-${source.id}`, name: source.name, host: source.kind === 'url' ? source.source_host : null, label: sourceStatus(source), tone, error: sourceReason(source.last_error),
    ...traffic(source.traffic), supported: source.supported_count, lastSuccess: source.last_success_at }
}

export function sourceView(card: SourceCard, now: number): SourceView {
  const days = card.expire === null ? null : Math.ceil((card.expire - now) / DAY)
  const expiry = days === null ? null : days <= 0 ? { label: `${day(card.expire!)} 已到期`, tone: 'danger' as Tone }
    : { label: days <= SOON_DAYS ? `${days} 天后到期` : `${day(card.expire!)} 到期`, tone: (days <= SOON_DAYS ? 'warning' : 'muted') as Tone }
  const usage = card.used !== null && card.total ? card.used / card.total * 100 : null
  const reasons: string[] = []
  if (card.tone === 'warning' || card.tone === 'danger') reasons.push(card.label)
  if (usage !== null && usage >= USAGE_WARNING_PERCENT) reasons.push('提供方流量将尽')
  if (days !== null && days <= SOON_DAYS) reasons.push(expiry!.label)
  return { status: { label: card.label, tone: card.tone }, usable: card.tone !== 'danger', reasons, weight: usage, due: card.expire, usage, expiry }
}

// One board for all three kinds.

export type Entry =
  | { kind: 'users'; key: string; name: string; search: string[]; item: UserSubscription; view: UserView }
  | { kind: 'renewals'; key: string; name: string; search: string[]; item: Expense; view: RenewalView }
  | { kind: 'sources'; key: string; name: string; search: string[]; item: SourceCard; view: SourceView }

export function boardCounts(entries: readonly Entry[]) {
  const users = entries.filter((entry): entry is Extract<Entry, { kind: 'users' }> => entry.kind === 'users')
  const renewals = entries.filter((entry): entry is Extract<Entry, { kind: 'renewals' }> => entry.kind === 'renewals')
  const usable = (list: readonly Entry[]) => list.filter(entry => entry.view.usable).length
  const active = renewals.filter(entry => entry.view.usable)
  return {
    all: entries.length,
    usable: usable(entries),
    attention: entries.filter(entry => entry.view.reasons.length).length,
    stopped: entries.length - usable(entries),
    users: { all: users.length, usable: usable(users), stopped: users.length - usable(users), attention: users.filter(entry => entry.view.reasons.length).length,
      expiring: users.filter(entry => entry.item.allowed && entry.view.expiry.days !== null && entry.view.expiry.days > 0 && entry.view.expiry.days <= SOON_DAYS).length,
      runningOut: users.filter(entry => entry.view.reasons.some(reason => reason.endsWith('用完'))).length,
      fetched: users.filter(entry => entry.item.fetches_today > 0).length, fetches: users.reduce((sum, entry) => sum + entry.item.fetches_today, 0),
      used: users.reduce((sum, entry) => sum + BigInt(entry.item.used_bytes), 0n),
      quota: users.reduce((sum, entry) => sum + (entry.item.allowed && entry.item.monthly_bytes ? BigInt(entry.item.monthly_bytes) : 0n), 0n),
      metered: users.filter(entry => entry.item.allowed && entry.item.monthly_bytes).length },
    renewals: { all: renewals.length, active: active.length, attention: renewals.filter(entry => entry.view.reasons.length).length,
      monthly: active.reduce((sum, entry) => sum + (entry.view.monthly ?? 0), 0), unconverted: active.filter(entry => entry.view.monthly === null).length,
      month: active.filter(entry => entry.view.days !== null && entry.view.days >= 0 && entry.view.days <= 30).length },
    sources: { all: entries.filter(entry => entry.kind === 'sources').length, attention: entries.filter(entry => entry.kind === 'sources' && entry.view.reasons.length).length },
  }
}

const rank = (entry: Entry) => entry.view.reasons.length ? 0 : entry.view.usable ? 1 : 2
const ascending = (left: number | null, right: number | null) => (left ?? Number.MAX_SAFE_INTEGER) - (right ?? Number.MAX_SAFE_INTEGER)
const descending = (left: number | null, right: number | null) => (right ?? -1) - (left ?? -1)
const fetchedAt = (entry: Entry) => entry.kind === 'users' ? entry.item.last_fetch_at : null

export function selectEntries(entries: readonly Entry[], kind: BoardKind | 'all', query: string, filter: BoardFilter, sort: BoardSort): Entry[] {
  const needle = query.trim().toLocaleLowerCase()
  return entries.filter(entry => (kind === 'all' || entry.kind === kind)
    && (!needle || entry.search.some(text => text.toLocaleLowerCase().includes(needle)))
    && (filter === 'all' || (filter === 'usable' ? entry.view.usable : filter === 'stopped' ? !entry.view.usable : entry.view.reasons.length > 0)))
    .sort((left, right) => {
      const order = sort === 'usage' ? descending(left.view.weight, right.view.weight)
        : sort === 'expiry' ? ascending(left.view.due, right.view.due)
          : sort === 'fetch' ? descending(fetchedAt(left), fetchedAt(right))
            : sort === 'name' ? left.name.localeCompare(right.name, 'zh-CN')
              // Within the same urgency, the soonest due date first, then the heaviest use.
              : rank(left) - rank(right) || ascending(left.view.due, right.view.due) || descending(left.view.weight, right.view.weight)
      return order || left.key.localeCompare(right.key)
    })
}

export const savedSort = (value: string | null): BoardSort => value === 'usage' || value === 'expiry' || value === 'fetch' || value === 'name' ? value : 'attention'
