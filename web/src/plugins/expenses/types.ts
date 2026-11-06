export const root = '/api/plugins/expenses'
export type Label = { id: number; kind: 'category' | 'payment_method' | 'payer'; name: string; enabled: boolean }
export type Expense = { id: number; name: string; amount_units: string; currency: string; start_date: string | null; next_payment: string; anchor_payment: string; cycle: 'day' | 'week' | 'month' | 'year'; frequency: number; category_id: number | null; payment_method_id: number | null; payer_id: number | null; notes: string; url: string; inactive: boolean; auto_renew: boolean; notify: boolean; notify_days_before: number; cancellation_date: string | null; replacement_id: number | null; version: number }
export type Settings = { base_currency: string; budget_units: string | null; timezone: string; reminders_enabled: boolean; version: number }
export const cycles = { day: '天', week: '周', month: '月', year: '年' }
export function parseAmount(value: FormDataEntryValue | null): string {
  const raw = String(value ?? '').trim()
  if (!/^\d{1,12}(?:\.\d{1,4})?$/.test(raw)) throw new Error('金额最多保留四位小数，且不能为负数。')
  const [whole, fraction = ''] = raw.split('.')
  const result = BigInt(whole) * 10000n + BigInt(fraction.padEnd(4, '0'))
  if (result > 9000000000000000n) throw new Error('金额超出支持范围。')
  return result.toString()
}
export function decimal(units: string): string {
  const number = BigInt(units), fraction = (number % 10000n).toString().padStart(4, '0').replace(/0+$/, '')
  return `${number / 10000n}${fraction ? `.${fraction}` : ''}`
}
export const amount = (units: string, currency: string) => `${currency} ${decimal(units)}`

/** Whole days from `today` to a ledger date (both YYYY-MM-DD in the ledger's time zone); negative once it has passed. */
export function daysUntil(date: string, today = new Date().toISOString().slice(0, 10)): number | null {
  const days = (Date.parse(`${date}T00:00:00Z`) - Date.parse(`${today}T00:00:00Z`)) / 86_400_000
  return Number.isFinite(days) ? Math.round(days) : null
}
const cycleMonths = { day: 12 / 365.25, week: 7 * 12 / 365.25, month: 1, year: 12 }
/** One record's cost per month in its own currency, spreading longer and shorter cycles evenly. */
export function monthlyAmount({ amount_units, cycle, frequency }: Pick<Expense, 'amount_units' | 'cycle' | 'frequency'>): number | null {
  const months = (cycleMonths[cycle] ?? Number.NaN) * frequency
  const value = Number(amount_units) / 10000 / months
  return Number.isFinite(value) && value >= 0 ? value : null
}
/** The length of one billing period in days. */
export function cycleDays({ cycle, frequency }: Pick<Expense, 'cycle' | 'frequency'>): number | null {
  const days = ({ day: 1, week: 7, month: 365.25 / 12, year: 365.25 })[cycle] * frequency
  return Number.isFinite(days) && days > 0 ? days : null
}
export const optionalId = (value: FormDataEntryValue | null) => value ? Number(value) : null
export const optionalText = (value: FormDataEntryValue | null) => value ? String(value) : null
export async function download(path: string, filename: string) {
  const response = await fetch(path, { credentials: 'same-origin', cache: 'no-store' })
  if (!response.ok) throw new Error('下载失败，请确认登录状态并重试。')
  const url = URL.createObjectURL(await response.blob()), anchor = document.createElement('a')
  anchor.href = url; anchor.download = filename; anchor.click(); window.setTimeout(() => URL.revokeObjectURL(url), 1000)
}

// Server cost records (fleet_cost_records) keep amounts in the currency's minor
// unit; Intl knows how many decimals each currency uses (JPY has none).
export function currencyDigits(code: string): number {
  try { return Math.min(4, new Intl.NumberFormat('en', { style: 'currency', currency: code }).resolvedOptions().maximumFractionDigits ?? 2) } catch { return 2 }
}
export function minorToUnits(minor: string, code: string): string {
  return (BigInt(minor) * 10n ** BigInt(4 - currencyDigits(code))).toString()
}
export function parseMinor(value: FormDataEntryValue | null, code: string): number {
  const raw = String(value ?? '').trim(), digits = currencyDigits(code)
  const pattern = digits ? new RegExp(`^\\d{1,12}(?:\\.\\d{1,${digits}})?$`) : /^\d{1,12}$/
  if (!pattern.test(raw)) throw new Error(digits ? `金额最多保留 ${digits} 位小数，且不能为负数。` : '此币种的金额只能是整数。')
  const [whole, fraction = ''] = raw.split('.')
  return Number(BigInt(whole) * 10n ** BigInt(digits) + BigInt(fraction.padEnd(digits, '0') || '0'))
}
