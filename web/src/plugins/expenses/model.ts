// The costs and renewals ledger as one list (ADR 0118): renewal items added by hand
// (Wallos style) and servers listed from their asset settings. The same due-date
// language is used wherever the ledger appears: this page, the subscription board
// and the overview.
import { convert } from '../../display/currency'
import type { ExchangeRates } from '../../display/currency'
import { amount, cycles, daysUntil, minorToUnits, monthlyAmount } from './types'
import type { Expense, Label } from './types'

export type Tone = 'good' | 'warning' | 'danger' | 'muted' | 'info'
/** Renewing within this many days needs attention. */
export const SOON_DAYS = 7
const DAYS_PER_MONTH = 365.25 / 12

export type ServerRenewal = { id: number; name: string; amount_units: string | null; currency: string; cycle_days: number; next_renewal: string; auto_renewal: boolean }
export type Due = { kind: 'item' | 'server' | 'reference'; id: number; name: string; date: string; amount_units: string | null; currency: string; auto_renew: boolean; path?: string }
export type LedgerSummary = { from: string; base_currency: string; budget_units: string | null; exchange: ExchangeRates; upcoming?: Due[] }

const record = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value)
const integer = (value: unknown) => typeof value === 'number' && Number.isSafeInteger(value)
const nullableInteger = (value: unknown) => value === null || value === undefined || integer(value)
const counter = (value: unknown) => typeof value === 'string' && /^\d{1,20}$/.test(value)
const day = (value: unknown) => typeof value === 'string' && /^\d{4}-\d{2}-\d{2}$/.test(value)
const signedCounter = (value: unknown) => typeof value === 'string' && /^-?\d{1,20}$/.test(value)
/** Minor currency units: the cost records send numbers, the payment history text. */
const minor = (value: unknown) => (typeof value === 'number' && Number.isSafeInteger(value) && value >= 0) || counter(value)
const text = (value: unknown) => typeof value === 'string'

export function validItems(value: unknown): value is Expense[] {
  return Array.isArray(value) && value.every(item => record(item) && integer(item.id) && typeof item.name === 'string' && counter(item.amount_units) && typeof item.currency === 'string'
    && day(item.next_payment) && ['day', 'week', 'month', 'year'].includes(String(item.cycle)) && integer(item.frequency) && Number(item.frequency) > 0
    && typeof item.inactive === 'boolean' && typeof item.auto_renew === 'boolean' && (item.cancellation_date === null || item.cancellation_date === undefined || day(item.cancellation_date))
    && ['category_id', 'payment_method_id', 'payer_id'].every(key => nullableInteger(item[key])))
}
export function validServers(value: unknown): value is { entries: ServerRenewal[] } {
  return record(value) && Array.isArray(value.entries) && value.entries.every(server => record(server) && integer(server.id) && typeof server.name === 'string'
    && (server.amount_units === null || counter(server.amount_units)) && typeof server.currency === 'string' && integer(server.cycle_days) && day(server.next_renewal) && typeof server.auto_renewal === 'boolean')
}
export const validLabels = (value: unknown): value is { entries: Label[] } => record(value) && Array.isArray(value.entries) && value.entries.every(label => record(label) && integer(label.id) && typeof label.name === 'string' && ['category', 'payment_method', 'payer'].includes(String(label.kind)))
export function validSummary(value: unknown): value is LedgerSummary {
  return record(value) && day(value.from) && typeof value.base_currency === 'string' && /^[A-Z]{3}$/.test(value.base_currency)
    && (value.budget_units === null || counter(value.budget_units)) && record(value.exchange) && (value.upcoming === undefined || Array.isArray(value.upcoming))
}

/** "11/9" for a ledger date; ledger dates are calendar days in the ledger's own time zone. */
export function shortDate(date: string) {
  const [, month, dayOfMonth] = date.split('-').map(Number)
  return month && dayOfMonth ? `${month}/${dayOfMonth}` : date
}
const weekdays = ['周日', '周一', '周二', '周三', '周四', '周五', '周六']
export const weekday = (date: string) => { const at = Date.parse(`${date}T00:00:00Z`); return Number.isFinite(at) ? weekdays[new Date(at).getUTCDay()] : '' }
export const todayUtc = () => new Date().toISOString().slice(0, 10)
export function addDays(date: string, days: number) {
  const at = Date.parse(`${date}T00:00:00Z`)
  return Number.isFinite(at) ? new Date(at + days * 86_400_000).toISOString().slice(0, 10) : date
}

export const cycleText = (item: Pick<Expense, 'cycle' | 'frequency'>) => item.frequency === 1 ? `每${cycles[item.cycle]}` : `每 ${item.frequency} ${cycles[item.cycle]}`
export const serverCycleText = (days: number) => days > 0 ? `每 ${days} 天` : '一次性'
/** A server's cost per month in its own currency; one-time purchases have none. */
export function serverMonthly(server: Pick<ServerRenewal, 'amount_units' | 'cycle_days'>): number | null {
  if (server.amount_units === null || server.cycle_days <= 0) return null
  const value = Number(server.amount_units) / 10000 / server.cycle_days * DAYS_PER_MONTH
  return Number.isFinite(value) ? value : null
}

export type DueState = {
  days: number | null
  status: { label: string; tone: Tone }
  /** A short value for a labelled cell; the full date goes beside or in a title. */
  next: { label: string; tone: Tone }
  /** Why an active entry needs looking at; empty when it does not. */
  reasons: string[]
}

/**
 * How a renewal date reads. Items renew; servers expire. Automatic renewals move
 * on by themselves, so a passed date only waits for the next sweep; manual ones
 * wait for someone to confirm the payment.
 */
export function dueState({ kind = 'item', next, today, inactive, auto, cancelled = false, recurring = true }: { kind?: 'item' | 'server'; next: string; today?: string; inactive: boolean; auto: boolean; cancelled?: boolean; recurring?: boolean }): DueState {
  const days = daysUntil(next, today)
  const server = kind === 'server'
  const passed = days !== null && days < 0, soon = days !== null && days >= 0 && days <= SOON_DAYS
  const status: { label: string; tone: Tone } = inactive ? { label: '已停用', tone: 'muted' }
    : passed ? server ? { label: auto ? '等待顺延' : '已到期', tone: auto ? 'warning' : 'danger' } : { label: auto ? '等待顺延' : '待确认续费', tone: auto ? 'warning' : 'danger' }
      : cancelled ? { label: '到期不续', tone: 'info' }
        : soon ? { label: days === 0 ? (server ? '今天到期' : '今天续费') : server ? '即将到期' : '即将续费', tone: 'warning' }
          : { label: server ? (!recurring ? '按到期日' : auto ? '自动顺延' : '按到期日') : auto ? '自动续费' : '手动续费', tone: 'good' }
  const label = inactive ? '—' : days === null ? next : passed ? `已过 ${-days} 天` : days === 0 ? '今天' : soon ? `${days} 天后` : shortDate(next)
  const reasons: string[] = []
  if (!inactive && days !== null) {
    if (passed) reasons.push(server ? auto ? '到期日已过，等待顺延' : '到期日已过，请登记续费' : auto ? '续费日已过，等待顺延' : '续费日已过，等待确认')
    else if (soon && !cancelled) reasons.push(days === 0 ? (server ? '今天到期' : '今天续费') : `${days} 天后${server ? '到期' : '续费'}`)
  }
  return { days, status, next: { label, tone: inactive ? 'muted' : passed ? 'danger' : soon ? 'warning' : 'muted' }, reasons }
}

type RowBase = {
  key: string; id: number; name: string
  /** "USD 20", or null for a server without a recorded price. */
  price: string | null
  cycle: string
  /** Cost per month in the ledger's currency, null when there is no price, no rate or no recurrence. */
  monthly: number | null
  /** Cost per month in the entry's own currency. */
  ownMonthly: number | null
  currency: string
  next: string
  auto: boolean
  inactive: boolean
  due: DueState
}
export type ItemRow = RowBase & { kind: 'item'; item: Expense; category: string; payment: string; payer: string; cancelled: boolean }
export type ServerRow = RowBase & { kind: 'server'; server: ServerRenewal }
export type LedgerRow = ItemRow | ServerRow

export type LedgerContext = { today?: string; base?: string; rates?: ExchangeRates; labels: Label[] }
const labelName = (labels: Label[], id: number | null) => labels.find(label => label.id === id)?.name ?? ''

export function itemRow(item: Expense, { today, base, rates, labels }: LedgerContext): ItemRow {
  const own = monthlyAmount(item)
  const cancelled = Boolean(item.cancellation_date && item.cancellation_date <= item.next_payment)
  return {
    kind: 'item', key: `item-${item.id}`, id: item.id, name: item.name, item, price: amount(item.amount_units, item.currency), cycle: cycleText(item),
    ownMonthly: own, monthly: base && own !== null ? convert(own, item.currency, base, rates) : null, currency: item.currency, next: item.next_payment,
    auto: item.auto_renew, inactive: item.inactive, cancelled, due: dueState({ next: item.next_payment, today, inactive: item.inactive, auto: item.auto_renew, cancelled }),
    category: labelName(labels, item.category_id), payment: labelName(labels, item.payment_method_id), payer: labelName(labels, item.payer_id),
  }
}

export function serverRow(server: ServerRenewal, { today, base, rates }: LedgerContext): ServerRow {
  const own = serverMonthly(server)
  return {
    kind: 'server', key: `server-${server.id}`, id: server.id, name: server.name, server, price: server.amount_units === null ? null : amount(server.amount_units, server.currency),
    cycle: serverCycleText(server.cycle_days), ownMonthly: own, monthly: base && own !== null ? convert(own, server.currency, base, rates) : null, currency: server.currency,
    next: server.next_renewal, auto: server.auto_renewal, inactive: false,
    due: dueState({ kind: 'server', next: server.next_renewal, today, inactive: false, auto: server.auto_renewal, recurring: server.cycle_days > 0 }),
  }
}

export type KindFilter = 'all' | 'item' | 'server'
export type LedgerSort = 'next' | 'cost' | 'name'
export const savedSort = (value: string | null): LedgerSort => value === 'cost' || value === 'name' ? value : 'next'

/** Search, filter and order the list: stopped entries last, then the soonest date. */
export function selectRows(rows: readonly LedgerRow[], { query = '', kind = 'all', inactive = false, category = '', sort = 'next' }: { query?: string; kind?: KindFilter; inactive?: boolean; category?: string; sort?: LedgerSort }) {
  const needle = query.trim().toLocaleLowerCase()
  return rows.filter(row => (kind === 'all' || row.kind === kind) && (inactive || !row.inactive)
    && (!category || (row.kind === 'item' && String(row.item.category_id ?? '') === category))
    && (!needle || [row.name, ...(row.kind === 'item' ? [row.category, row.payment, row.payer, row.item.notes] : [])].some(text => text.toLocaleLowerCase().includes(needle))))
    .sort((left, right) => Number(left.inactive) - Number(right.inactive)
      || (sort === 'cost' ? (right.monthly ?? -1) - (left.monthly ?? -1) : sort === 'name' ? left.name.localeCompare(right.name, 'zh-CN') : (left.due.days ?? Number.MAX_SAFE_INTEGER) - (right.due.days ?? Number.MAX_SAFE_INTEGER))
      || left.name.localeCompare(right.name, 'zh-CN') || left.key.localeCompare(right.key))
}

export function ledgerTotals(rows: readonly LedgerRow[]) {
  const active = rows.filter(row => !row.inactive)
  const recurring = active.filter(row => row.ownMonthly !== null)
  return {
    items: active.filter(row => row.kind === 'item').length,
    servers: active.filter(row => row.kind === 'server').length,
    inactive: rows.length - active.length,
    monthly: recurring.reduce((sum, row) => sum + (row.monthly ?? 0), 0),
    /** Entries with a price but no rate to the ledger's currency: left out of the total. */
    unconverted: recurring.filter(row => row.monthly === null).length,
    attention: active.filter(row => row.due.reasons.length).length,
    overdue: active.filter(row => row.due.days !== null && row.due.days < 0).length,
    soon: active.filter(row => row.due.days !== null && row.due.days >= 0 && row.due.days <= SOON_DAYS).length,
  }
}

/** The run rate by category: what each kind of spending costs per month, largest first. */
export function monthlyByCategory(rows: readonly LedgerRow[]) {
  const groups = new Map<string, { name: string; monthly: number; count: number }>()
  for (const row of rows) {
    if (row.inactive || row.monthly === null) continue
    const name = row.kind === 'server' ? '服务器' : row.category || '未分类'
    const group = groups.get(name) ?? { name, monthly: 0, count: 0 }
    group.monthly += row.monthly; group.count++
    groups.set(name, group)
  }
  const total = [...groups.values()].reduce((sum, group) => sum + group.monthly, 0)
  return [...groups.values()].sort((left, right) => right.monthly - left.monthly).map(group => ({ ...group, share: total > 0 ? group.monthly / total * 100 : 0 }))
}

export type MonthGroup = { month: string; label: string; entries: Due[]; total: number; unconverted: number; count: number }
/** Due dates grouped by calendar month, each with what it costs in the ledger's currency. */
export function monthGroups(dues: readonly Due[], base?: string, rates?: ExchangeRates): MonthGroup[] {
  const groups = new Map<string, MonthGroup>()
  for (const due of [...dues].sort((left, right) => left.date.localeCompare(right.date) || left.name.localeCompare(right.name, 'zh-CN'))) {
    const month = due.date.slice(0, 7)
    const [year, number] = month.split('-').map(Number)
    const group = groups.get(month) ?? { month, label: `${year} 年 ${number} 月`, entries: [], total: 0, unconverted: 0, count: 0 }
    group.entries.push(due); group.count++
    if (due.amount_units !== null) {
      const value = base ? convert(Number(due.amount_units) / 10000, due.currency, base, rates) : null
      if (value === null) group.unconverted++; else group.total += value
    }
    groups.set(month, group)
  }
  return [...groups.values()]
}

export type ItemPayment = { kind: 'item'; id: string; subscription_id: number; name: string; paid_on: string; period_start: string; amount_units: string; currency: string }
export type ServerPayment = { kind: 'server'; id: string; server_id: number; name: string; record_kind: string; paid_on: string; valid_until: number | null; amount_minor: string; currency: string; reference: string }
export type Payment = ItemPayment | ServerPayment

export function validPayments(value: unknown): value is { items: ItemPayment[]; servers: ServerPayment[] } {
  return record(value) && Array.isArray(value.items) && Array.isArray(value.servers)
    && value.items.every(payment => record(payment) && (text(payment.id) || integer(payment.id)) && integer(payment.subscription_id) && text(payment.name) && day(payment.paid_on) && day(payment.period_start) && counter(payment.amount_units) && text(payment.currency))
    && value.servers.every(payment => record(payment) && text(payment.id) && integer(payment.server_id) && text(payment.name) && text(payment.record_kind) && day(payment.paid_on) && minor(payment.amount_minor) && text(payment.currency) && nullableInteger(payment.valid_until))
}
export type ItemPaymentRow = { id: string | number; period_start: string; paid_on: string; amount_units: string; currency: string }
export const validItemPayments = (value: unknown): value is { entries: ItemPaymentRow[] } => record(value) && Array.isArray(value.entries)
  && value.entries.every(payment => record(payment) && (text(payment.id) || integer(payment.id)) && day(payment.paid_on) && day(payment.period_start) && counter(payment.amount_units) && text(payment.currency))
export type CostRecord = { id: string; kind: string; amount_minor: number | string; currency: string; occurred_at: number; valid_until: number | null; reference: string }
export type Costs = { records: CostRecord[]; annual: { year: number; currency: string; amount_minor: string }[] }
export function validCosts(value: unknown): value is Costs {
  return record(value) && Array.isArray(value.records) && Array.isArray(value.annual)
    && value.records.every(entry => record(entry) && text(entry.id) && text(entry.kind) && minor(entry.amount_minor) && text(entry.currency) && integer(entry.occurred_at) && nullableInteger(entry.valid_until) && (entry.reference === null || entry.reference === undefined || text(entry.reference)))
    && value.annual.every(entry => record(entry) && integer(entry.year) && text(entry.currency) && signedCounter(entry.amount_minor))
}
export const recordKinds: Record<string, string> = { renewal: '续费', purchase: '购买', price_change: '价格变化', refund: '退款', cancellation: '退订' }

/** Purchases, renewals and confirmed item renewals count; refunds subtract; price changes and cancellations do not count. */
export function signedUnits(payment: Payment): bigint {
  if (payment.kind === 'item') return BigInt(payment.amount_units)
  const units = BigInt(minorToUnits(payment.amount_minor, payment.currency))
  return payment.record_kind === 'refund' ? -units : ['purchase', 'renewal'].includes(payment.record_kind) ? units : 0n
}

/** Actual payments by year, newest first, per currency and, where rates allow, in the ledger's currency. */
export function paymentYears(payments: readonly Payment[], base?: string, rates?: ExchangeRates) {
  type Year = { year: string; payments: Payment[]; totals: Map<string, bigint> }
  const years = new Map<string, Year>()
  for (const payment of [...payments].sort((left, right) => right.paid_on.localeCompare(left.paid_on) || left.id.localeCompare(right.id))) {
    const year = payment.paid_on.slice(0, 4)
    const entry: Year = years.get(year) ?? { year, payments: [], totals: new Map() }
    entry.payments.push(payment)
    entry.totals.set(payment.currency, (entry.totals.get(payment.currency) ?? 0n) + signedUnits(payment))
    years.set(year, entry)
  }
  return [...years.values()].sort((left, right) => right.year.localeCompare(left.year)).map(entry => {
    const totals = [...entry.totals].sort(([left], [right]) => left.localeCompare(right)).map(([currency, units]) => ({ currency, units }))
    let converted: number | null = 0
    for (const total of totals) {
      const value = base ? convert(Math.abs(Number(total.units)) / 10000, total.currency, base, rates) : null
      if (value === null) { converted = null; break }
      converted += total.units < 0n ? -value : value
    }
    return { year: entry.year, payments: entry.payments, totals, converted }
  })
}
