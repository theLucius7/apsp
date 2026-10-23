import { useEffect, useState } from 'react'
import { api } from '../api'
import type { ActionRunner, Plan } from './types'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'

type Incident = { id: string; title: string; source_key: string; status: string; server_id: number | null }
type Rule = { id: string; name: string; paused: boolean; run_count: number; max_runs: number; cooldown_secs: number }
export default function RemediationRules({ plan, targets, run, version }: { plan: Plan; targets: number[]; run: ActionRunner; version: number }) {
  const [rules, setRules] = useState<Rule[]>([])
  const [incidents, setIncidents] = useState<Incident[]>([])
  const [incidentId, setIncidentId] = useState('')
  const [cooldown, setCooldown] = useState(1800)
  const [maxRuns, setMaxRuns] = useState(3)
  const refresh = async () => { const [values, events] = await Promise.all([api<Rule[]>('/api/operations/remediation'), api<Incident[]>('/api/operations/incidents')]); setRules(values); setIncidents(events.filter(event => event.source_key.startsWith('service:') && event.status !== 'resolved')) }
  useEffect(() => { run(refresh) }, [version]) // eslint-disable-line react-hooks/exhaustive-deps
  return <section className="bg-card min-w-0 rounded-xl border p-4 wrap-anywhere max-sm:p-3"><h3 className="mt-0 mb-2 text-sm font-semibold">有界自动处置</h3><p>复用上面的固定目标和步骤，仅在同一受管服务故障有两分钟内的新观测时触发。旧数据、任务结果未知、权限撤销或人工暂停时不继续发起。</p><div className="flex flex-wrap gap-2.5"><label className="max-sm:basis-full my-2 grid min-w-0 flex-[1_1_12rem] gap-1.5 text-sm has-[[role=checkbox]]:flex has-[[role=checkbox]]:flex-none has-[[role=checkbox]]:items-center">实际故障来源<NativeSelect className="w-full" value={incidentId} onChange={event => setIncidentId(event.target.value)}><option value="">选择受管服务故障</option>{incidents.filter(event => event.server_id !== null && targets.includes(event.server_id)).map(event => <option key={event.id} value={event.id}>{event.title}</option>)}</NativeSelect></label><label className="max-sm:basis-full my-2 grid min-w-0 flex-[1_1_12rem] gap-1.5 text-sm has-[[role=checkbox]]:flex has-[[role=checkbox]]:flex-none has-[[role=checkbox]]:items-center">冷却期（秒）<Input type="number" min={300} max={604800} value={cooldown} onChange={event => setCooldown(Number(event.target.value))} /></label><label className="max-sm:basis-full my-2 grid min-w-0 flex-[1_1_12rem] gap-1.5 text-sm has-[[role=checkbox]]:flex has-[[role=checkbox]]:flex-none has-[[role=checkbox]]:items-center">最多触发次数<Input type="number" min={1} max={100} value={maxRuns} onChange={event => setMaxRuns(Number(event.target.value))} /></label></div><Button variant="outline" disabled={!incidentId || !targets.length} onClick={() => run(async () => { await api('/api/operations/remediation', 'POST', { name: `${plan.name}自动处置`, incident_id: incidentId, plan, targets, cooldown_secs: cooldown, max_runs: maxRuns }); await refresh() }, true)}>验证身份并启用明确处置规则</Button>{rules.map(rule => <p key={rule.id}>{rule.name} · {rule.paused ? '已暂停' : '已启用'} · {rule.run_count}/{rule.max_runs} 次 · 冷却 {rule.cooldown_secs} 秒 <Button variant="outline" onClick={() => run(async () => { await api(`/api/operations/remediation/${rule.id}/pause`, 'POST', { paused: !rule.paused }); await refresh() }, rule.paused)}>{rule.paused ? '恢复规则' : '人工暂停'}</Button></p>)}</section>
}
