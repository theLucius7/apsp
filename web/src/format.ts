// How the console writes times, sizes, money and numbers: every page formats them here,
// so the same reading looks the same everywhere (tests/one-implementation.test.ts).

/** A size in binary units with one decimal, such as 1.5 GiB; `missing` when the value is absent or not a whole number of bytes. */
export function bytes(value: string | number | bigint | undefined | null, missing = '暂无数据'): string {
  if (value === undefined || value === null) return missing
  try {
    const amount = typeof value === 'number' ? BigInt(Math.max(0, Math.trunc(value))) : BigInt(value)
    if (amount < 0n) return missing
    const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB', 'EiB']
    let divisor = 1n, unit = 0
    while (amount >= divisor * 1024n && unit < units.length - 1) { divisor *= 1024n; unit++ }
    if (!unit) return `${amount} B`
    const scaled = amount * 10n / divisor
    return `${scaled / 10n}.${scaled % 10n} ${units[unit]}`
  } catch { return missing }
}
/** Upload plus download, or undefined when either counter is not a whole number, so `bytes` shows it as missing. */
export function totalBytes(up: string, down: string): string | undefined {
  try { return (BigInt(up) + BigInt(down)).toString() } catch { return undefined }
}
export const percent = (value?: number) => value === undefined ? '暂无数据' : `${value.toFixed(1)}%`
/** A plain number grouped in thousands, with at most `digits` decimals. */
export const numberText = (value: number, digits = 3) => value.toLocaleString('zh-CN', { maximumFractionDigits: digits })

/** Decimals a currency normally uses (JPY none, most two), at most four. */
export function currencyDigits(code: string): number {
  try { return Math.min(4, new Intl.NumberFormat('en', { style: 'currency', currency: code }).resolvedOptions().maximumFractionDigits ?? 2) } catch { return 2 }
}
/**
 * A sum of money with its currency code, such as CNY 1,234.50: the currency's usual
 * decimals, at most `digits`. Recorded amounts pass 4 so nothing entered is rounded away.
 */
export function money(amount: number | null | undefined, currency: string, digits = 2): string {
  if (amount === null || amount === undefined || !Number.isFinite(amount)) return '—'
  try { return new Intl.NumberFormat('zh-CN', { style: 'currency', currency, currencyDisplay: 'code', maximumFractionDigits: digits }).format(amount) }
  catch { return `${currency} ${numberText(amount, digits)}` }
}

// Times read in Chinese date order on a 24-hour clock, in the browser's time zone unless one is named.
const local = (seconds: number, options: Intl.DateTimeFormatOptions = {}, zone?: string | null) =>
  new Date(seconds * 1000).toLocaleString('zh-CN', { hour12: false, timeZone: zone || undefined, ...options })
/** A moment in a chosen time zone (the browser's when none is given); a zone the browser does not know falls back to UTC and says so. */
export function dateText(seconds: number | null, zone?: string | null): string {
  if (seconds === null) return '—'
  if (!Number.isFinite(new Date(seconds * 1000).getTime())) return '时间不可用'
  try { return local(seconds, {}, zone) }
  catch { return `${local(seconds, {}, 'UTC')}（UTC；浏览器不支持套餐时区）` }
}
/** Date and time, such as 2026/11/14 08:30:00; `missing` when there is no moment. */
export const time = (seconds?: number | null, missing = '尚未上报') => seconds ? dateText(seconds) : missing
/** The time of day, such as 08:30:00. */
export const clock = (seconds: number) => local(seconds, { hour: '2-digit', minute: '2-digit', second: '2-digit' })
/** The calendar day, such as 2026/11/14. */
export const day = (seconds: number) => local(seconds, { year: 'numeric', month: 'numeric', day: 'numeric' })
/** Month, day and minute, such as 11/14 08:30, for chart axes that span days. */
export const shortTime = (seconds: number) => local(seconds, { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
export function uptime(value?: number) {
  if (value === undefined) return '暂无数据'
  const days = Math.floor(value / 86400), hours = Math.floor(value % 86400 / 3600), minutes = Math.floor(value % 3600 / 60)
  return days ? `${days} 天 ${hours} 小时` : `${hours} 小时 ${minutes} 分钟`
}
export { navigate } from '@/lib/leave-guard'

/** Coarse relative time for status lines, such as "12 分钟前"; the exact time belongs in a tooltip. */
export function ago(seconds?: number | null, now = Date.now() / 1000) {
  if (!seconds) return '时间未知'
  const elapsed = Math.max(0, now - seconds)
  if (elapsed < 60) return '刚刚'
  if (elapsed < 3600) return `${Math.floor(elapsed / 60)} 分钟前`
  if (elapsed < 86400) return `${Math.floor(elapsed / 3600)} 小时前`
  return `${Math.floor(elapsed / 86400)} 天前`
}
