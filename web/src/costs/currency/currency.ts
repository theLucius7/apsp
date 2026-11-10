import type { Server, ServerAssets } from '../../types'

export type ExchangeRates = {
  base: 'CNY'; rates: Record<string, number>; rate_dates: Record<string, string>;
  rate_date: string | null; source: 'frankfurter' | 'frankfurter-ecb' | null;
  source_url: string | null; fetched_at: number | null; attempted_at: number | null;
  next_refresh_at: number; stale: boolean; status: 'fresh' | 'stale' | 'unavailable'; error_code: string | null;
}
export const displayCurrencies = ['CNY', 'USD', 'EUR', 'HKD', 'JPY', 'GBP', 'SGD', 'CAD', 'AUD', 'CHF', 'KRW', 'RUB', 'VND']
export const currencyCode = (value: unknown) => typeof value === 'string' && /^[A-Z]{3}$/.test(value) ? value : 'CNY'

export function quoteState(quote: ExchangeRates | undefined, error = ''): 'fresh' | 'stale' | 'read-error' | 'unavailable' | 'unknown' {
  if (error) return 'read-error'
  if (!quote || quote.status === 'unavailable') return 'unavailable'
  if (quote.stale || quote.status === 'stale' || quote.error_code) return 'stale'
  return quote.status === 'fresh' && quote.stale === false ? 'fresh' : 'unknown'
}

export function price(value: unknown): number | null {
  if (typeof value !== 'string' || !/^\d+(?:\.\d+)?$/.test(value)) return null
  const amount = Number(value)
  return Number.isFinite(amount) && amount >= 0 ? amount : null
}

export function convert(amount: number | null, from: string, to: string, quote: ExchangeRates | undefined): number | null {
  if (amount === null || !Number.isFinite(amount) || amount < 0 || !/^[A-Z]{3}$/.test(from) || !/^[A-Z]{3}$/.test(to)) return null
  if (from === to || amount === 0) return amount
  if (quote?.base !== 'CNY') return null
  const rate = (code: string) => code === 'CNY' ? 1 : quote.rates[code]
  const source = rate(from), target = rate(to)
  if (![source, target].every(value => typeof value === 'number' && Number.isFinite(value) && value > 0)) return null
  const result = amount / source * target
  return Number.isFinite(result) ? result : null
}

export function money(amount: number | null, currency: string): string {
  if (amount === null || !Number.isFinite(amount)) return '—'
  return new Intl.NumberFormat('zh-CN', { style: 'currency', currency: currencyCode(currency), currencyDisplay: 'code', maximumFractionDigits: 2 }).format(amount)
}

export function remainingCost(asset: ServerAssets | undefined, now: number): number | null {
  const amount = price(asset?.price)
  if (amount === null || !asset || !Number.isFinite(asset.billing_cycle) || asset.billing_cycle < 0 || asset.expires_at === null || !Number.isFinite(asset.expires_at) || !Number.isFinite(now)) return null
  const remainingDays = Math.max(0, (asset.expires_at * 1000 - now) / 86_400_000)
  if (remainingDays === 0) return 0
  // A prepaid term may span several billing periods. One-time purchases retain
  // their entered value until expiry, but never contribute to monthly spending.
  return asset.billing_cycle === 0 ? amount : amount * remainingDays / asset.billing_cycle
}

export function costSummary(servers: Server[], currency: string, quote: ExchangeRates | undefined, now: number) {
  const visible = servers.filter(server => !server.public_view && !server.asset_settings?.hidden)
  let total = 0, recurring = 0, remaining = 0, priced = 0, converted = 0, recurringCount = 0, remainingCount = 0, missingRates = 0
  for (const server of visible) {
    const asset = server.asset_settings, amount = price(asset?.price)
    if (!asset || amount === null) continue
    priced++
    const value = convert(amount, asset.currency, currency, quote)
    if (value === null) { missingRates++; continue }
    total += value; converted++
    if (asset.billing_cycle > 0) { recurring += value / asset.billing_cycle * 30; recurringCount++ }
    const remainder = convert(remainingCost(asset, now), asset.currency, currency, quote)
    if (remainder !== null) { remaining += remainder; remainingCount++ }
  }
  return { total: converted ? total : null, recurring: recurringCount ? recurring : null, remaining: remainingCount ? remaining : null, priced, converted, recurringCount, remainingCount, missingRates, missingPrices: visible.length - priced }
}

export function quoteDate(quote: ExchangeRates | undefined, from: string, to: string): string | null {
  if (from === to || convert(1, from, to, quote) === null) return null
  return [from, to].filter(code => code !== 'CNY').map(code => quote?.rate_dates[code]).filter((date): date is string => Boolean(date)).sort()[0] ?? quote?.rate_date ?? null
}
