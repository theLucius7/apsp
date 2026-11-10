import { useState } from 'react'
import type { ReactNode } from 'react'
import { api } from '../../api'
import { Badge, Confirm, Empty, ErrorNotice, Modal, DialogPresence } from '../../components'
import type { Tone } from '../../components'
import { useAction } from '../../hooks'
import { charge, message, powerSource, rotationActive, rotationPhase, rotationStatus, rotationVerdict, status, stopMode, time } from '../../resources/cloud/types'
import type { EipRotation, Operation, Overview, PowerJob, RotationAttempt } from '../../resources/cloud/types'
import { POWER_RESULT, activityItems, deliveryText } from '../../costs/cloud/history'
import type { ActivityItem } from '../../costs/cloud/history'
import { checked, operationError, rotationError } from '../../operations/cloud/guards'
import { EipRotationReconcile } from '../../operations/cloud/EipRotation'
import type { CurrentCloud } from '../../operations/cloud/guards'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { HistoryIcon } from 'lucide-react'

const LIMIT = 12
/** A finished or administrator-closed entry: its old error and request ID no longer need attention. */
const settled = (value: string) => value === 'succeeded' || value === 'dismissed'
const tone = (value: string): Tone => value === 'succeeded' ? 'good' : ['uncertain', 'failed'].includes(value) ? 'warm' : 'neutral'
const rotationTone = (value: string): Tone => value === 'succeeded' ? 'good' : rotationActive(value) || value === 'restored' || value === 'failed' ? 'warm' : 'neutral'
/** One candidate address and what the mainland probes found. */
const candidate = (item: RotationAttempt) => {
  const tally = item.confirm ?? item.screen
  return `${item.ip} ${rotationVerdict(item.verdict)}${tally ? `（${tally.reached}/${tally.total}）` : ''}`
}
/** After a rotation, the node addresses on the instance's servers are the step left to the administrator. */
function NodeFollowUp({ rotation }: { rotation: EipRotation }) {
  const servers = rotation.before.server_ids ?? []
  const link = (href: string, label: string) => <a key={href} className="text-primary underline-offset-4 hover:underline" href={href}>{label}</a>
  return <small className="text-muted-foreground text-xs wrap-anywhere">写着 {rotation.before.public_ip} 的节点公开地址需改为新地址：{servers.length
    ? servers.map((id, index) => <span key={id}>{index > 0 && '、'}{link(`#/nodes?server=${id}`, servers.length > 1 ? `检查服务器 ${index + 1} 的节点` : '检查此服务器的节点')}</span>)
    : link('#/nodes', '检查代理节点')}</small>
}

/** One history for the page: what changed on which resource, its state and the step left to take. */
export function Activity({ data, available, current, reload }: { data: Overview; available: boolean; current: CurrentCloud; reload: () => void }) {
  const action = useAction(), [all, setAll] = useState(false)
  const [operation, setOperation] = useState<Operation | null>(null), [verified, setVerified] = useState(false), [job, setJob] = useState<PowerJob | null>(null)
  const [rotation, setRotation] = useState<EipRotation | null>(null)
  const items = activityItems(data), shown = all ? items : items.slice(0, LIMIT)
  const name = (id: string, fallback: string) => data.resources.find(resource => resource.id === id)?.name ?? fallback
  const run = (error: () => string, request: () => Promise<unknown>, after: () => void = reload) => void action.run(() => checked(error, request), after)
  const finish = () => { setOperation(null); setJob(null); setRotation(null); reload() }
  const disabled = !available || action.busy

  const row = (item: ActivityItem) => {
    let resource: string, what: string, source: string, state: { label: string; tone: Tone } | null, notes: string[], controls: ReactNode = null, follow: ReactNode = null
    if (item.kind === 'bandwidth') {
      const { operation: o } = item, before = o.before_state, target = o.target
      resource = name(o.resource_id, before.cloud_id)
      what = `带宽 ${before.bandwidth_mbps} → ${target.bandwidth_mbps} Mbps${before.charge_type !== target.charge_type ? ` · ${charge(before.charge_type)} → ${charge(target.charge_type)}` : ''}`
      source = o.source === 'manual' ? '手动' : '自动降速'
      state = { label: status(o.status), tone: tone(o.status) }
      notes = settled(o.status) ? [] : [message(o.error_code), o.request_id ? `云请求：${o.request_id}` : '']
      if (['preview', 'queued'].includes(o.status)) controls = <Button variant="ghost" size="sm" disabled={disabled} onClick={() => run(() => operationError(current, o, false, ['preview', 'queued']), () => api(`/api/plugins/alicloud/operations/${o.id}/cancel`, 'POST'))}>取消</Button>
      if (o.status === 'uncertain') controls = <Button variant="ghost" size="sm" disabled={disabled} onClick={() => { action.clearError(); setVerified(false); setOperation(o) }}>已人工核对</Button>
    } else if (item.kind === 'power') {
      const { job: j } = item
      resource = name(j.resource_id, j.before_state.cloud_id)
      what = j.action === 'start' ? '开机' : stopMode(j.stop_mode)
      source = powerSource(j.source)
      state = { label: j.status === 'failed' ? '未按预期完成' : status(j.status), tone: tone(j.status) }
      notes = [...settled(j.status) ? [] : [message(j.error_code), j.request_id ? `云请求：${j.request_id}` : ''], deliveryText(item.event)]
      if (['preview', 'queued'].includes(j.status)) controls = <Button variant="ghost" size="sm" disabled={disabled} onClick={() => run(() => operationError(current, j, true, ['preview', 'queued']), () => api(`/api/plugins/alicloud/power-jobs/${j.id}/cancel`, 'POST'))}>取消任务</Button>
      if (j.status === 'uncertain') controls = <Button variant="ghost" size="sm" disabled={disabled} onClick={() => { action.clearError(); setJob(j) }}>核对并结束跟踪</Button>
    } else if (item.kind === 'rotation') {
      const { rotation: r } = item
      resource = name(r.resource_id, r.before.instance_id)
      what = `更换公网 IP · ${r.before.public_ip}${r.new_ip ? ` → ${r.new_ip}` : ''}`
      source = '手动'
      state = { label: rotationStatus(r.status), tone: rotationTone(r.status) }
      notes = [r.status === 'running' ? `第 ${r.attempt} / ${r.max_attempts} 个新地址 · ${rotationPhase(r.phase)}` : '',
        r.attempts.length ? `候选：${r.attempts.map(candidate).join('、')}` : '', message(r.error_code), deliveryText(item.event)]
      if (r.status === 'preview') controls = <Button variant="ghost" size="sm" disabled={disabled} onClick={() => run(() => rotationError(current, r, ['preview']), () => api(`/api/plugins/alicloud/eip-rotations/${r.id}/cancel`, 'POST'))}>取消</Button>
      if (r.status === 'unknown') controls = <Button variant="ghost" size="sm" disabled={disabled} onClick={() => { action.clearError(); setRotation(r) }}>已人工核对</Button>
      if (r.status === 'succeeded') follow = <NodeFollowUp rotation={r} />
    } else {
      const { event } = item
      resource = name(event.resource_id, '已移除资源')
      what = event.title
      source = ''
      state = event.title === POWER_RESULT ? null : { label: '提醒', tone: 'warm' }
      notes = [event.message, deliveryText(event)]
    }
    return <li key={item.key} data-slot="cloud-activity-item" className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2 px-4 py-3">
      <div className="grid min-w-0 gap-0.5">
        <span className="text-sm wrap-anywhere"><strong className="font-medium">{resource}</strong> · {what}</span>
        <small className="text-muted-foreground text-xs">{time(item.at)}{source && ` · ${source}`}</small>
        {notes.filter(Boolean).map(note => <small key={note} className="text-muted-foreground text-xs wrap-anywhere">{note}</small>)}
        {follow}
      </div>
      <div className="flex flex-wrap items-center gap-2">{state && <Badge tone={state.tone}>{state.label}</Badge>}{controls}</div>
    </li>
  }

  return <Panel data-slot="cloud-activity">
    <PanelHeader><PanelTitle>操作记录</PanelTitle><span className="text-muted-foreground text-xs">保留 180 天 · <a className="text-primary underline-offset-4 hover:underline" href="#/alerts/settings">通知渠道</a></span></PanelHeader>
    {action.error && <div className="px-4 pt-3"><ErrorNotice message={action.error} /></div>}
    {!items.length ? <Empty icon={HistoryIcon} title="暂无操作记录" description="带宽调整、开关机（手动、CDT 守护、每日计划与保活）、更换公网 IP 和流量阈值提醒会记录在这里。" />
      : <ul className="divide-border/70 divide-y">{shown.map(row)}</ul>}
    {items.length > LIMIT && <div className="border-t px-4 py-2"><Button variant="ghost" size="sm" onClick={() => setAll(value => !value)}>{all ? '只显示最近' : `显示全部 ${items.length} 条`}</Button></div>}
    <DialogPresence>{operation && <Modal title="结束不确定操作的跟踪" busy={action.busy} onClose={() => setOperation(null)}><div data-slot="modal-body" className="p-4"><ErrorNotice message={action.error} /><p>请先在阿里云控制台核对资源与订单。结束跟踪不会撤销已提交的调整，会同时暂停此资源的自动降速与自动启停，并保留手动暂停开机标记，避免立即再次执行。</p><label className="my-4 flex items-start gap-2.5 text-sm leading-relaxed"><Checkbox checked={verified} onCheckedChange={checked => setVerified(checked === true)} className="mt-0.5" />已核对云端结果，确认结束跟踪</label></div><footer><Button variant="outline" disabled={action.busy} onClick={() => setOperation(null)}>返回</Button><Button disabled={action.busy || !verified || Boolean(operationError(current, operation, false, ['uncertain']))} onClick={() => { if (verified) run(() => operationError(current, operation, false, ['uncertain']), () => api(`/api/plugins/alicloud/operations/${operation.id}/dismiss`, 'POST'), finish) }}>结束跟踪并暂停资源策略</Button></footer></Modal>}</DialogPresence>
    <DialogPresence>{rotation && <EipRotationReconcile rotation={rotation} current={current} close={() => setRotation(null)} saved={finish} />}</DialogPresence>
    <DialogPresence>{job && <Confirm title="结束启停跟踪" confirmLabel="结束跟踪" busyLabel="正在处理…" busy={action.busy} error={action.error} confirmDisabled={Boolean(operationError(current, job, true, ['uncertain']))} onClose={() => setJob(null)} onConfirm={() => run(() => operationError(current, job, true, ['uncertain']), () => api(`/api/plugins/alicloud/power-jobs/${job.id}/dismiss`, 'POST'), finish)}>请先在阿里云核对实例状态及收费模式。结束跟踪不会撤回命令，会暂停此资源的自动启停和降速，并保留手动暂停标记。</Confirm>}</DialogPresence>
  </Panel>
}
