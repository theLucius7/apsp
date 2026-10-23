import { useState } from 'react'
import { api } from '../api'
import { Badge, ErrorNotice, Field, Loading } from '../components'
import { useAction, useResource } from '../hooks'
import { currencyCode, displayCurrencies } from '../display/currency'
import type { ExchangeRates } from '../display/currency'
import { Button } from '@/components/ui/button'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

const preferenceKey = 'apsp-display-currency'
const currencyNames: Record<string, string> = { CNY: '人民币', USD: '美元', EUR: '欧元', HKD: '港币', JPY: '日元', GBP: '英镑', SGD: '新加坡元', CAD: '加元', AUD: '澳元', CHF: '瑞士法郎', KRW: '韩元', RUB: '俄罗斯卢布', VND: '越南盾' }
const dateTime = (value: number | null | undefined) => value && Number.isFinite(value) ? new Date(value * 1000).toLocaleString('zh-CN', { hour12: false }) : '—'

function readPreference() {
  try { return { currency: currencyCode(localStorage.getItem(preferenceKey)), error: '' } }
  catch { return { currency: 'CNY', error: '无法读取浏览器偏好，暂用人民币。' } }
}

export default function ExchangeRateSettings() {
  const rates = useResource<ExchangeRates>('/api/exchange-rates', 0)
  const action = useAction()
  const [initial] = useState(readPreference)
  const [currency, setCurrency] = useState(initial.currency), [preferenceError, setPreferenceError] = useState(initial.error)
  const [saved, setSaved] = useState(false), [feedback, setFeedback] = useState('')
  const [manualQuote, setManualQuote] = useState<{ previousRead: ExchangeRates | undefined; value: ExchangeRates }>()
  // The POST response is already a complete cache snapshot. Retain it until a
  // later successful GET replaces the read, including when that GET fails.
  const quote = manualQuote && rates.data === manualQuote.previousRead ? manualQuote.value : rates.data
  const entries = Object.entries(quote?.rates ?? {}).filter(([code, value]) => /^[A-Z]{3}$/.test(code) && Number.isFinite(value) && value > 0).sort(([left], [right]) => left.localeCompare(right))
  const currencies = [...new Set([...displayCurrencies, currency, ...entries.map(([code]) => code)])]
  const unavailable = !quote || quote.status === 'unavailable'
  const stale = quote?.stale || quote?.status === 'stale' || Boolean(quote?.error_code)
  const source = quote?.source === 'frankfurter-ecb' ? 'Frankfurter · ECB' : quote?.source === 'frankfurter' ? 'Frankfurter' : '—'
  const sourceUrl = quote?.source_url?.startsWith('https://') ? quote.source_url : null
  const missingCurrency = currency !== 'CNY' && quote && !entries.some(([code]) => code === currency)
  const save = () => {
    setSaved(false); setPreferenceError('')
    try {
      localStorage.setItem(preferenceKey, currency)
      setSaved(true)
    } catch { setPreferenceError('浏览器不允许保存偏好，显示币种未保存；请允许本站存储后重试。') }
  }
  const refresh = () => {
    setFeedback('')
    void action.run(() => api<ExchangeRates>('/api/exchange-rates/refresh', 'POST', {}), result => {
      setManualQuote({ previousRead: rates.data, value: result })
      setFeedback(result.error_code || result.status === 'unavailable' ? '本次未取得新汇率，已有缓存仍保留。' : result.stale ? '已读取报价，当前仍使用较早的参考汇率。' : '汇率已更新。')
      rates.reload()
    })
  }
  return <Panel data-slot="exchange-rate-settings" aria-labelledby="exchange-rate-settings-title">
    <PanelHeader><PanelTitle id="exchange-rate-settings-title">币种与汇率</PanelTitle><Badge tone={unavailable || stale || rates.error ? 'warm' : 'good'}>{unavailable ? '暂无外币汇率' : stale ? '缓存已过期' : rates.error ? '读取失败，保留报价' : '汇率可用'}</Badge></PanelHeader>
    <div className="p-4">
      <div className="flex flex-wrap items-start gap-4"><Field className="m-0 flex-[1_1_240px]" label="显示币种" hint="仅保存到当前浏览器，用于看板成本折算；不修改服务器的原币价格。"><NativeSelect className="w-full" value={currency} onChange={event => { setCurrency(event.target.value); setSaved(false); setPreferenceError('') }}>{currencies.map(code => <option key={code} value={code}>{currencyNames[code] ? `${currencyNames[code]} · ` : ''}{code}</option>)}</NativeSelect></Field><Button type="button" className="mt-6 max-sm:mt-0" onClick={save}>保存显示币种</Button></div>
      <ErrorNotice message={preferenceError} />{saved && <p role="status">显示币种已保存为 {currency}，返回看板后生效。</p>}
      {missingCurrency && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">当前没有 {currency} 参考汇率，无法折算的费用会显示为“—”。</p>}
      <div className="mt-6 flex flex-wrap items-center justify-between gap-3"><h3 className="m-0 text-sm font-semibold">每日参考汇率</h3><div className="flex flex-wrap items-center gap-2.5"><Button type="button" variant="outline" disabled={action.busy || rates.loading} onClick={rates.reload}>重新读取</Button><Button type="button" variant="outline" disabled={action.busy || rates.loading} onClick={refresh}>{action.busy ? '正在更新…' : '更新汇率'}</Button></div></div>
      <ErrorNotice message={action.error || rates.error} />
      {feedback && <p role="status">{feedback}</p>}
      {rates.loading && !quote ? <Loading /> : <>
        <dl data-slot="exchange-rate-meta" className="my-5 grid gap-4 text-xs [grid-template-columns:repeat(auto-fit,minmax(180px,1fr))] max-sm:grid-cols-1"><div><dt className="text-muted-foreground mb-1.5">报价日期</dt><dd className="m-0 wrap-anywhere">{quote?.rate_date ?? '—'}</dd></div><div><dt className="text-muted-foreground mb-1.5">汇率来源</dt><dd className="m-0 wrap-anywhere">{sourceUrl ? <a className="text-primary underline-offset-4 hover:underline" href={sourceUrl} target="_blank" rel="noreferrer">{source}</a> : source}</dd></div><div><dt className="text-muted-foreground mb-1.5">最近成功获取</dt><dd className="m-0 wrap-anywhere">{dateTime(quote?.fetched_at)}</dd></div><div><dt className="text-muted-foreground mb-1.5">最近尝试</dt><dd className="m-0 wrap-anywhere">{dateTime(quote?.attempted_at)}</dd></div><div><dt className="text-muted-foreground mb-1.5">下次自动尝试</dt><dd className="m-0 wrap-anywhere">{dateTime(quote?.next_refresh_at)}</dd></div></dl>
        <p className="text-muted-foreground mt-3 text-xs leading-relaxed">面板每日自动获取参考汇率，获取失败时保留旧报价。手动更新受频率限制；“重新读取”只读取面板缓存。</p>
        {quote?.error_code && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">最近获取失败，{unavailable ? '暂时只能使用原币金额。' : '当前仍按上次成功获取的汇率折算。'}</p>}
        <details><summary className="text-primary cursor-pointer text-sm">查看汇率表（{entries.length} 个币种）</summary><div className="mt-3.5 overflow-x-auto"><Table><TableHeader><TableRow><TableHead>币种</TableHead><TableHead>1 人民币可兑换</TableHead><TableHead>报价日期</TableHead></TableRow></TableHeader><TableBody>{entries.map(([code, value]) => <TableRow key={code}><TableCell>{currencyNames[code] ? `${currencyNames[code]} · ` : ''}{code}</TableCell><TableCell>{new Intl.NumberFormat('zh-CN', { maximumSignificantDigits: 8 }).format(value)}</TableCell><TableCell>{code === 'CNY' ? '基准币种' : quote?.rate_dates[code] ?? '—'}</TableCell></TableRow>)}</TableBody></Table>{!entries.length && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">暂无报价。</p>}</div></details>
      </>}
    </div>
  </Panel>
}
