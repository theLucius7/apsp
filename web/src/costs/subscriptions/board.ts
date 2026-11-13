// The subscription board (ADR 0117): the administrator's own subscriptions, the
// services they pay for and record Wallos style in 费用与续费, as cards beside
// the server board. Proxy users' subscriptions belong to other people and stay
// on the proxy user pages; servers have their own board.
import { convert } from '../currency/currency'
import type { ExchangeRates } from '../currency/currency'
import { SOON_DAYS, itemRow } from '../expenses/model'
import type { LedgerSummary } from '../expenses/model'
import type { Tone } from '@/components/board'
import { cycleDays } from '../expenses/types'
import type { Expense, Label } from '../expenses/types'

export type RenewalSummary = LedgerSummary
// The ledger's own validators: the board reads the same endpoints as the page.
export { validItems as validRenewals, validLabels, validSummary as validRenewalSummary } from '../expenses/model'

export type BoardFilter = 'all' | 'active' | 'attention' | 'stopped'
export type BoardSort = 'next' | 'cost' | 'name'

export type RenewalView = {
  status: { label: string; tone: Tone }
  /** In use: not stopped. */
  usable: boolean
  /** Why it needs looking at; empty when it does not. */
  reasons: string[]
  price: string
  cycle: string
  /** Cost per month in the ledger's currency, or null without a rate. */
  monthly: number | null
  /** The next payment in the ledger's currency, or null without a rate. */
  nextAmount: number | null
  days: number | null
  /** Share of the current billing period that has passed. */
  progress: number | null
  next: { label: string; title: string; tone: Tone }
  renewal: string
  category: string
  payment: string
}

/** `today` is the ledger's own date (its time zone); `base` and `rates` convert to its currency. */
export function renewalView(item: Expense, today: string | undefined, base: string | undefined, rates: ExchangeRates | undefined, labels: Label[]): RenewalView {
  // The ledger page reads the same item the same way (ADR 0118).
  const row = itemRow(item, { today, base, rates, labels })
  const { days, status, reasons } = row.due
  const period = cycleDays(item)
  const progress = item.inactive || days === null || period === null ? null : Math.min(100, Math.max(0, (1 - days / period) * 100))
  const next = item.inactive ? { label: '—', title: '已停用，不再续费', tone: 'muted' as Tone }
    : { ...row.due.next, title: `下次续费 ${item.next_payment}${row.cancelled ? `；已安排 ${item.cancellation_date} 取消` : ''}` }
  return {
    status, usable: !item.inactive, reasons, price: row.price!, cycle: row.cycle, monthly: row.monthly,
    nextAmount: base ? convert(Number(item.amount_units) / 10000, item.currency, base, rates) : null,
    days, progress, next, renewal: item.auto_renew ? '自动' : '手动', category: row.category, payment: row.payment,
  }
}

export type Entry = { key: string; item: Expense; view: RenewalView }

export function boardCounts(entries: readonly Entry[]) {
  const active = entries.filter(entry => entry.view.usable)
  const within = (entry: Entry, limit: number) => entry.view.days !== null && entry.view.days >= 0 && entry.view.days <= limit
  const month = active.filter(entry => within(entry, 30))
  return {
    all: entries.length,
    active: active.length,
    stopped: entries.length - active.length,
    attention: entries.filter(entry => entry.view.reasons.length).length,
    overdue: active.filter(entry => entry.view.days !== null && entry.view.days < 0).length,
    soon: active.filter(entry => within(entry, SOON_DAYS)).length,
    monthly: active.reduce((sum, entry) => sum + (entry.view.monthly ?? 0), 0),
    /** In use but without a rate to the ledger's currency: left out of the totals. */
    unconverted: active.filter(entry => entry.view.monthly === null).length,
    month: month.length,
    monthTotal: month.reduce((sum, entry) => sum + (entry.view.nextAmount ?? 0), 0),
  }
}

/** Search, filter and order the cards: the soonest renewal first, stopped ones last. */
export function selectEntries(entries: readonly Entry[], { query = '', filter = 'all', category = '', sort = 'next' }: { query?: string; filter?: BoardFilter; category?: string; sort?: BoardSort }): Entry[] {
  const needle = query.trim().toLocaleLowerCase()
  return entries.filter(({ item, view }) => (filter === 'all' || (filter === 'active' ? view.usable : filter === 'stopped' ? !view.usable : view.reasons.length > 0))
    && (!category || String(item.category_id ?? '') === category)
    && (!needle || [item.name, view.category, view.payment, item.notes].some(text => text.toLocaleLowerCase().includes(needle))))
    .sort((left, right) => Number(!left.view.usable) - Number(!right.view.usable)
      || (sort === 'cost' ? (right.view.monthly ?? -1) - (left.view.monthly ?? -1)
        : sort === 'name' ? left.item.name.localeCompare(right.item.name, 'zh-CN')
          : (left.view.days ?? Number.MAX_SAFE_INTEGER) - (right.view.days ?? Number.MAX_SAFE_INTEGER))
      || left.item.name.localeCompare(right.item.name, 'zh-CN') || left.item.id - right.item.id)
}
