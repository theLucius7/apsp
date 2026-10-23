import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Field, Loading } from '../components'
import { useAction, useResource } from '../hooks'

type Policy = { history_retention_days: number }

function Form({ initial }: { initial: Policy }) {
  const [savedDays, setSavedDays] = useState(initial.history_retention_days)
  const [days, setDays] = useState(String(initial.history_retention_days))
  const [confirmed, setConfirmed] = useState(false), [saved, setSaved] = useState(false)
  const action = useAction()
  const decreasing = Number(days) < savedDays
  return <form onSubmit={event => {
    event.preventDefault()
    if (decreasing && !confirmed) return
    void action.run(() => api<Policy>('/api/telemetry/policy', 'PATCH', { history_retention_days: Number(days) }), result => {
      setSavedDays(result.history_retention_days); setDays(String(result.history_retention_days)); setConfirmed(false); setSaved(true)
    })
  }}><fieldset disabled={action.busy}>
    <Field label="监控历史保留天数" hint="1–3650 天，默认 30 天。只影响系统指标，不改变流量账本或代理用户用量。"><input type="number" required min={1} max={3650} step={1} value={days} onChange={event => { setDays(event.target.value); setSaved(false); setConfirmed(false) }} /></Field>
    <p className="helper">近 2 小时保留原始采样；7 天内按分钟汇总，7–30 天按 5 分钟汇总，更早按小时汇总。平均值按实际样本数计算，保留峰值；缺失数据不补零。超过所设期限的历史会分批清理。</p>
    {decreasing && <label><input type="checkbox" required checked={confirmed} onChange={event => setConfirmed(event.target.checked)} />我了解缩短期限会永久清理更早的监控历史，之后调长也无法恢复。</label>}
    <ErrorNotice message={action.error} />
    <button className="button button-primary" disabled={action.busy || (decreasing && !confirmed)}>{action.busy ? '正在保存…' : '保存历史策略'}</button>
    {saved && <p role="status">历史保存策略已更新，后台会分批执行。</p>}
  </fieldset></form>
}

export default function TelemetryPolicy() {
  const resource = useResource<Policy>('/api/telemetry/policy', 0)
  return <section className="panel"><div className="panel-heading"><h2>监控历史保存</h2></div><div className="panel-body">
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.data ? <Form initial={resource.data} /> : resource.loading ? <Loading /> : null}
  </div></section>
}
