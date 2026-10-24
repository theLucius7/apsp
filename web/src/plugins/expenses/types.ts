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
export const optionalId = (value: FormDataEntryValue | null) => value ? Number(value) : null
export const optionalText = (value: FormDataEntryValue | null) => value ? String(value) : null
export async function download(path: string, filename: string) {
  const response = await fetch(path, { credentials: 'same-origin', cache: 'no-store' })
  if (!response.ok) throw new Error('下载失败，请确认登录状态并重试。')
  const url = URL.createObjectURL(await response.blob()), anchor = document.createElement('a')
  anchor.href = url; anchor.download = filename; anchor.click(); window.setTimeout(() => URL.revokeObjectURL(url), 1000)
}
