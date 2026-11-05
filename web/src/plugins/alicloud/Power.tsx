import { useState } from 'react'
import { api } from '../../api'
import { ErrorNotice, Field, Modal } from '../../components'
import { useAction } from '../../hooks'
import { defaultPowerPolicy, powerStatus, stopMode, time, utcOffset } from './types'
import type { PowerJob, PowerPolicy, Resource } from './types'
import { checked, operationError, powerError, resourceError } from './guards'
import type { CurrentCloud } from './guards'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { CdtGuardFields, defaultCdtGuard } from './CdtGuard'

type Events = { close: () => void; saved: () => void; available: boolean; current: CurrentCloud; refresh: () => void }
export function PowerPolicyEditor({ resource, close, saved, current }: Events & { resource: Resource }) {
  const [value, setValue] = useState<PowerPolicy>(resource.power_policy ?? defaultPowerPolicy), action = useAction()
  const set = <K extends keyof PowerPolicy>(key: K, next: PowerPolicy[K]) => setValue(previous => ({ ...previous, [key]: next }))
  const [accountRevision] = useState(() => current()?.accounts.find(a => a.id === resource.account_id)?.revision)
  const writeError = () => resourceError(current, resource, { power: true, accountRevision })
  return <Modal title={`自动启停 · ${resource.name}`} busy={action.busy} onClose={close}><form onSubmit={e => { e.preventDefault(); void action.run(() => checked(writeError, () => api(`/api/plugins/alicloud/resources/${resource.id}/power-policy`, 'PATCH', { revision: resource.revision, policy: value })), saved) }}>
    <div data-slot="modal-body" className="p-4"><ErrorNotice message={action.error || writeError()} /><fieldset disabled={action.busy}>
      <label className="my-4 flex items-start gap-2.5 text-sm leading-relaxed"><Checkbox checked={value.enabled} onCheckedChange={checked => set('enabled', checked === true)} className="mt-0.5" />启用自动启停策略</label>
      <Field label="启停策略类型"><NativeSelect className="w-full" value={value.cdt_guard ? 'cdt' : 'billing'} onChange={e => setValue(previous => e.target.value === 'cdt' ? { ...previous, cdt_guard: { ...defaultCdtGuard }, stop_mode: 'StopCharging', threshold_action: 'off', schedule_enabled: false, keepalive: false } : { ...previous, cdt_guard: null })}><option value="billing">账单阈值与每日计划</option><option value="cdt">CDT 脚本守护</option></NativeSelect></Field>
      {value.cdt_guard ? <CdtGuardFields value={value.cdt_guard} onChange={next => set('cdt_guard', next)} /> : <>
      <Field label="自动停机模式"><NativeSelect className="w-full" value={value.stop_mode} onChange={e => set('stop_mode', e.target.value as PowerPolicy['stop_mode'])}><option value="KeepCharging">普通停机（KeepCharging）</option><option value="StopCharging">节省停机（StopCharging）</option></NativeSelect></Field>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">普通停机继续收费并保留计算资源。节省停机仍可能收取磁盘等费用，固定公网 IP 可能变化，重新启动依赖库存；不支持时云端可能按普通模式停机，面板会核对实际模式。</p>
      <Field label="流量阈值动作"><NativeSelect className="w-full" value={value.threshold_action} onChange={e => set('threshold_action', e.target.value as PowerPolicy['threshold_action'])}><option value="off">关闭阈值策略</option><option value="notify">仅通知</option><option value="stop">停机并通知</option></NativeSelect></Field>
      {value.threshold_action !== 'off' && <><div className="grid gap-x-4 sm:grid-cols-2"><Field label="账号 CDT 月流量额度（GB）"><Input required type="number" min={1} max={1000000000} step={1} value={value.limit_gb} onChange={e => set('limit_gb', Number(e.target.value))} /></Field><Field label="触发百分比（%）"><Input required type="number" min={1} max={100} step={1} value={value.threshold_percent} onChange={e => set('threshold_percent', Number(e.target.value))} /></Field></div><p className="text-muted-foreground mt-3 text-xs leading-relaxed">按账号合计已出账 CDT 用量判断，达到 {value.limit_gb * value.threshold_percent / 100} GB 时触发；云账单有延迟，不能作为实时硬限额。停机保护优先于任何开机，账单失效不会解除保护。通知使用面板设置中的 Telegram / Webhook，事件同时保存在本页。</p></>}
      <label className="my-4 flex items-start gap-2.5 text-sm leading-relaxed"><Checkbox checked={value.schedule_enabled} onCheckedChange={checked => set('schedule_enabled', checked === true)} className="mt-0.5" />每日定时开关机</label><p className="text-muted-foreground mt-3 text-xs leading-relaxed">计划与保活由面板执行，目标停机期间面板需保持在线。</p>
      {value.schedule_enabled && <><div className="grid gap-x-4 sm:grid-cols-2"><Field label="每日开机时间"><Input type="time" required value={value.start_time} onChange={e => set('start_time', e.target.value)} /></Field><Field label="每日停机时间"><Input type="time" required value={value.stop_time} onChange={e => set('stop_time', e.target.value)} /></Field></div><Field label="计划时区（固定 UTC 偏移分钟）"><Input type="number" required min={-720} max={840} step={15} value={value.utc_offset_minutes} onChange={e => set('utc_offset_minutes', Number(e.target.value))} /></Field><p className="text-muted-foreground mt-3 text-xs leading-relaxed">当前 {utcOffset(value.utc_offset_minutes)}{value.utc_offset_minutes === 480 ? '（北京时间）' : ''}，不跟随浏览器时区或夏令时。支持跨午夜，仅补执行最近 10 分钟的计划；手动停机后暂停自动开机。</p></>}
      <label className="my-4 flex items-start gap-2.5 text-sm leading-relaxed"><Checkbox checked={value.keepalive} onCheckedChange={checked => set('keepalive', checked === true)} className="mt-0.5" />抢占式实例保活</label><p className="text-muted-foreground mt-3 text-xs leading-relaxed">仅尝试启动仍存在的已停止抢占式实例，不购买或重建资源。遵循流量保护、手动暂停及计划运行时段；明确拒绝后至少等待 15 分钟，结果不确定时只核对状态。</p>
      </>}
    </fieldset></div><footer><Button variant="outline" type="button" disabled={action.busy} onClick={close}>取消</Button><Button disabled={action.busy || Boolean(writeError()) || value.start_time === value.stop_time}>保存启停策略</Button></footer>
  </form></Modal>
}
export function PowerEditor({ resource, action: verb, close, saved, current, refresh }: Events & { resource: Resource; action: 'start' | 'stop' }) {
  const [mode, setMode] = useState(resource.power_policy?.stop_mode ?? 'KeepCharging'), [preview, setPreview] = useState<PowerJob | null>(null), [confirmed, setConfirmed] = useState(false), action = useAction()
  const [accountRevision] = useState(() => current()?.accounts.find(a => a.id === resource.account_id)?.revision)
  const writeError = () => powerError(current, resource, verb, accountRevision) || (preview ? operationError(current, preview, true, ['preview'], true) : '')
  return <Modal title={`${verb === 'start' ? '开机' : '停机'} · ${resource.name}`} busy={action.busy} onClose={close}><form onSubmit={e => { e.preventDefault(); if (preview) { if (confirmed) void action.run(() => checked(writeError, () => api(`/api/plugins/alicloud/power-jobs/${preview.id}/confirm`, 'POST')), saved) } else void action.run(() => checked(writeError, () => api<PowerJob>(`/api/plugins/alicloud/resources/${resource.id}/power-preview`, 'POST', { revision: resource.revision, action: verb, stop_mode: mode })), value => { setPreview(value); setConfirmed(false); refresh() }) }}>
    <div data-slot="modal-body" className="p-4"><ErrorNotice message={action.error || writeError()} />{preview ? <><dl className="my-5 grid gap-4 sm:grid-cols-2"><div><dt className="text-muted-foreground mb-1.5 text-xs">实例</dt><dd className="m-0 wrap-anywhere">{preview.before_state.cloud_id} · {preview.before_state.region}</dd></div><div><dt className="text-muted-foreground mb-1.5 text-xs">云端状态</dt><dd className="m-0 wrap-anywhere">{powerStatus(preview.before_state.status)}</dd></div><div><dt className="text-muted-foreground mb-1.5 text-xs">本次操作</dt><dd className="m-0 wrap-anywhere">{verb === 'start' ? '开机' : stopMode(preview.stop_mode)}</dd></div><div><dt className="text-muted-foreground mb-1.5 text-xs">预览有效期</dt><dd className="m-0 wrap-anywhere">{time(preview.expires_at)}</dd></div></dl><label className="my-4 flex items-start gap-2.5 text-sm leading-relaxed"><Checkbox checked={confirmed} onCheckedChange={checked => setConfirmed(checked === true)} className="mt-0.5" />已核对实例，确认执行{verb === 'start' ? '开机并承担运行费用' : '停机及相应收费方式'}</label></> : verb === 'stop' ? <Field label="本次停机模式"><NativeSelect className="w-full" value={mode} disabled={action.busy} onChange={e => setMode(e.target.value as PowerPolicy['stop_mode'])}><option value="KeepCharging">普通停机（KeepCharging）</option><option value="StopCharging">节省停机（StopCharging）</option></NativeSelect></Field> : <p>将启动此 ECS 实例并恢复运行计费。节省停机后的固定公网 IP 可能变化，启动依赖可用库存。</p>}
      {verb === 'stop' && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">正常关机，不强制断电。普通停机继续收费；节省停机仍可能收取磁盘等费用，并可能释放固定公网 IP。手动停机会暂停计划开机与保活，直到手动开机或恢复自动策略。</p>}<p className="text-muted-foreground mt-3 text-xs leading-relaxed">确认后进入任务队列，只有云端状态核对完成才显示成功。已发送的命令不能取消，未知结果不会重复发送。</p>
    </div><footer><Button variant="outline" type="button" disabled={action.busy} onClick={preview ? () => { setPreview(null); setConfirmed(false) } : close}>{preview ? '重新预览' : '取消'}</Button><Button disabled={action.busy || Boolean(writeError()) || (!!preview && !confirmed)}>{preview ? '确认执行启停' : '预览启停操作'}</Button></footer>
  </form></Modal>
}
