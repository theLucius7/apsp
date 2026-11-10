import { useCurrency } from './CurrencyContext'
import { convert, quoteDate, quoteState } from './currency'

export default function CurrencyReference({ from, to, compact = false }: { from: string; to: string; compact?: boolean }) {
  const { quote, error } = useCurrency()
  if (from === to) return null
  const state = quoteState(quote, error), available = convert(1, from, to, quote) !== null
  const date = quoteDate(quote, from, to)
  const label = state === 'read-error' ? `汇率读取失败 · ${available ? '使用上次数据' : '暂无所需汇率'}`
    : !available || state === 'unavailable' ? '暂无所需汇率'
      : state === 'stale' ? '使用上次数据' : state === 'unknown' ? '汇率状态未知' : '参考汇率'
  const source = quote?.source === 'frankfurter-ecb' ? 'Frankfurter · ECB' : quote?.source === 'frankfurter' ? 'Frankfurter' : ''
  const fetched = quote?.fetched_at != null && Number.isFinite(quote.fetched_at) ? new Date(quote.fetched_at * 1000).toLocaleString('zh-CN', { hour12: false }) : ''
  const provenance = [date, source, quote?.source_url?.startsWith('https://') ? quote.source_url : '', fetched ? `获取于 ${fetched}` : ''].filter(Boolean).join(' · ')
  return <small role="status" aria-label="参考汇率状态" className={state !== 'fresh' || !available ? 'text-warning' : 'text-muted-foreground'} title={provenance || '缺少可用参考汇率'}>{compact ? label : [date, label, source, fetched ? `获取于 ${fetched}` : ''].filter(Boolean).join(' · ')}</small>
}
