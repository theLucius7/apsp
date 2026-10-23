import { useState } from 'react'
import { api } from '../api'
import { Badge, ErrorNotice, Field, Loading } from '../components'
import { useAction, useResource } from '../hooks'
import { currencyCode, displayCurrencies } from '../display/currency'
import type { ExchangeRates } from '../display/currency'

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
  return <section className="panel exchange-rate-settings" aria-labelledby="exchange-rate-settings-title">
    <div className="panel-heading"><h2 id="exchange-rate-settings-title">币种与汇率</h2><Badge tone={unavailable || stale || rates.error ? 'warm' : 'good'}>{unavailable ? '暂无外币汇率' : stale ? '缓存已过期' : rates.error ? '读取失败，保留报价' : '汇率可用'}</Badge></div>
    <div className="panel-body">
      <div className="exchange-rate-preference"><Field label="显示币种" hint="仅保存到当前浏览器，用于看板成本折算；不修改服务器的原币价格。"><select value={currency} onChange={event => { setCurrency(event.target.value); setSaved(false); setPreferenceError('') }}>{currencies.map(code => <option key={code} value={code}>{currencyNames[code] ? `${currencyNames[code]} · ` : ''}{code}</option>)}</select></Field><button type="button" className="button button-primary" onClick={save}>保存显示币种</button></div>
      <ErrorNotice message={preferenceError} />{saved && <p role="status">显示币种已保存为 {currency}，返回看板后生效。</p>}
      {missingCurrency && <p className="helper">当前没有 {currency} 参考汇率，无法折算的费用会显示为“—”。</p>}
      <div className="exchange-rate-heading"><h3>每日参考汇率</h3><div className="monitoring-actions"><button type="button" className="button button-secondary" disabled={action.busy || rates.loading} onClick={rates.reload}>重新读取</button><button type="button" className="button button-secondary" disabled={action.busy || rates.loading} onClick={refresh}>{action.busy ? '正在更新…' : '更新汇率'}</button></div></div>
      <ErrorNotice message={action.error || rates.error} />
      {feedback && <p role="status">{feedback}</p>}
      {rates.loading && !quote ? <Loading /> : <>
        <dl className="exchange-rate-meta"><div><dt>报价日期</dt><dd>{quote?.rate_date ?? '—'}</dd></div><div><dt>汇率来源</dt><dd>{sourceUrl ? <a href={sourceUrl} target="_blank" rel="noreferrer">{source}</a> : source}</dd></div><div><dt>最近成功获取</dt><dd>{dateTime(quote?.fetched_at)}</dd></div><div><dt>最近尝试</dt><dd>{dateTime(quote?.attempted_at)}</dd></div><div><dt>下次自动尝试</dt><dd>{dateTime(quote?.next_refresh_at)}</dd></div></dl>
        <p className="helper">面板每日自动获取参考汇率，获取失败时保留旧报价。手动更新受频率限制；“重新读取”只读取面板缓存。</p>
        {quote?.error_code && <p className="helper">最近获取失败，{unavailable ? '暂时只能使用原币金额。' : '当前仍按上次成功获取的汇率折算。'}</p>}
        <details className="exchange-rate-table"><summary>查看汇率表（{entries.length} 个币种）</summary><div className="table-scroll"><table><thead><tr><th>币种</th><th>1 人民币可兑换</th><th>报价日期</th></tr></thead><tbody>{entries.map(([code, value]) => <tr key={code}><td>{currencyNames[code] ? `${currencyNames[code]} · ` : ''}{code}</td><td>{new Intl.NumberFormat('zh-CN', { maximumSignificantDigits: 8 }).format(value)}</td><td>{code === 'CNY' ? '基准币种' : quote?.rate_dates[code] ?? '—'}</td></tr>)}</tbody></table>{!entries.length && <p className="helper">暂无报价。</p>}</div></details>
      </>}
    </div>
  </section>
}
