import { useEffect, useRef, useState } from 'react'
import { Badge, ErrorNotice, Field, FormDialog, Loading, Meter, DialogPresence } from '../../components'
import { bytes } from '../../format'
import { resourceWriteError, useAction, useResource, settingsPoll } from '../../hooks'
import type { ResourceState } from '../../hooks'
import { assignmentRequestId, dateText, scheduleText, statusText } from './groupTypes'
import type { Entitlement, PackageGroup, PolicyGroup, UserPolicies } from './groupTypes'
import OperationReview from './OperationReview'
import type { WorkflowOperation } from './OperationReview'
import ValidityDialog from './ValidityDialog'
import { expiryText } from './validity'
import { Button, buttonVariants } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { NativeSelect } from '@/components/ui/native-select'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { cn } from '@/lib/utils'

const root = '/api/plugins/sing-box'
export default function UserEntitlements({ id, entitlement, userError, refreshRevision, onChange, bridged = false }: { id: number; entitlement: ResourceState<Entitlement>; userError: () => string; refreshRevision: number; onChange: () => void; bridged?: boolean }) {
  const policies = useResource<PolicyGroup[]>(`${root}/policy-groups`, settingsPoll)
  const packages = useResource<PackageGroup[]>(`${root}/package-groups`, settingsPoll)
  const assigned = useResource<UserPolicies>(`${root}/users/${id}/policy-groups`, 0)
  const [selected, setSelected] = useState<number[]>([])
  const dirty = useRef(false)
  const [assignment, setAssignment] = useState<string | null>(null)
  const [packageChoice, setPackageChoice] = useState('')
  const [notice, setNotice] = useState('')
  const action = useAction()
  const [operation, setOperation] = useState<WorkflowOperation | null>(null)
  const [adjusting, setAdjusting] = useState(false)
  useEffect(() => { if (assigned.data && !dirty.current) setSelected([...assigned.data.group_ids]) }, [assigned.data])
  const refresh = () => { policies.reload(); packages.reload(); assigned.reload(); entitlement.reload() }
  useEffect(() => { if (refreshRevision) { policies.reload(); packages.reload(); assigned.reload() } }, [refreshRevision, policies.reload, packages.reload, assigned.reload])
  const e = entitlement.data
  const writeError = () => userError() || resourceWriteError(policies, packages, assigned, entitlement)
  const policyError = () => writeError() || (selected.some(id => !policies.getCurrent()?.some(value => value.id === id)) ? '已选策略组已不存在。原选择仍保留，请刷新确认，或明确取消这些选择后再保存。' : '')
  const packageError = () => writeError() || (packageChoice && !packages.getCurrent()?.some(value => String(value.id) === packageChoice) ? '已选套餐组已不存在，请重新选择；当前分配草稿已保留。' : '')
  const savePolicies = () => {
    if (policyError()) return
    setOperation({ operation: 'policy_batch', user_ids: [id], group_ids: [...selected] })
  }
  const assign = () => {
    if (!assignment || !packageChoice || packageError()) return
    setOperation({ operation: 'replace_package', user_id: id, package_group_id: Number(packageChoice) })
  }
  const assignedIds = assigned.data?.group_ids ?? []
  const changed = selected.length !== assignedIds.length || selected.some(id => !assignedIds.includes(id))
  const quota = e?.monthly_bytes ? Number(e.used_bytes) / Number(e.monthly_bytes) * 100 : undefined
  return <Panel aria-label="权限与套餐">
    <PanelHeader className="min-h-10 py-1.5"><PanelTitle>权限与套餐</PanelTitle><a className={buttonVariants({ variant: 'ghost', size: 'xs' })} href="#/groups">管理策略与套餐</a></PanelHeader>
    <div className="p-3.5">
      <ErrorNotice message={policies.error || packages.error || assigned.error || entitlement.error || policyError() || (!assignment ? action.error : '')} retry={refresh} />
      {notice && <Notice tone="success" role="status">{notice}</Notice>}
      <div className="grid gap-x-6 gap-y-5 xl:grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)]">
        <section aria-label="策略组" className="min-w-0">
          <div className="mb-2 flex min-h-7 flex-wrap items-center justify-between gap-2">
            <h3 className="text-[13px] font-semibold">能用哪些节点 <span className="text-muted-foreground font-normal">· 策略组</span></h3>
            {(policies.data?.length || selected.length) ? <Button variant={changed ? 'default' : 'outline'} size="sm" disabled={action.busy || Boolean(policyError())} onClick={savePolicies}>{action.busy ? '正在保存…' : '保存策略组分配'}</Button> : null}
          </div>
          {!assigned.data ? <Loading variant="panel" rows={2} /> : policies.data?.length || selected.length ? <div data-slot="group-choices" className="grid gap-1.5 sm:grid-cols-2">
            {policies.data?.map(p => <label key={p.id} className={cn('hover:bg-muted/60 flex cursor-pointer items-center gap-2.5 rounded-md border px-2.5 py-2 text-[13px]', selected.includes(p.id) && 'border-primary/40 bg-accent/50')}>
              <Checkbox checked={selected.includes(p.id)} disabled={action.busy} onCheckedChange={checked => { dirty.current = true; setSelected(previous => checked === true ? [...previous, p.id] : previous.filter(value => value !== p.id)) }} />
              <span className="min-w-0">{p.name}<small className="text-muted-foreground block truncate text-xs">{p.node_ids.length} 个节点，{p.chain_ids.length} 条链路</small></span>
            </label>)}
            {selected.filter(id => !policies.data?.some(value => value.id === id)).map(id => <label key={id} className="flex cursor-pointer items-center gap-2.5 rounded-md border px-2.5 py-2 text-[13px]">
              <Checkbox checked disabled={action.busy} onCheckedChange={() => { dirty.current = true; setSelected(previous => previous.filter(value => value !== id)) }} /><span>策略组 #{id}（已不存在，原选择保留）</span>
            </label>)}
          </div> : <p className="text-muted-foreground text-xs leading-relaxed">尚未创建策略组。用户只通过策略组获得节点，请先在「策略与套餐」中创建。</p>}
        </section>
        <section aria-label="套餐" className="min-w-0">
          <div className="mb-2 flex min-h-7 flex-wrap items-center justify-between gap-2">
            <h3 className="text-[13px] font-semibold">流量与有效期 <span className="text-muted-foreground font-normal">· 套餐</span></h3>
            <Button variant="outline" size="sm" disabled={action.busy || Boolean(writeError()) || !packages.data?.length} onClick={() => { if (writeError()) return; action.clearError(); setPackageChoice(''); setAssignment(assignmentRequestId()) }}>分配或更换套餐</Button>
          </div>
          {!e ? <Loading variant="panel" rows={2} /> : <>
            <div className="flex flex-wrap items-center gap-2 text-[13px]"><strong className="font-medium">{e.package_name ?? '未设置流量与到期限制'}</strong><Badge tone={e.allowed ? 'good' : 'bad'}>{statusText[e.status]}</Badge></div>
            {e.package_group_id !== null ? <>
              <div className="text-muted-foreground mt-2 flex flex-wrap items-center gap-x-2 text-xs tabular-nums"><span>本期已用 <span className="text-foreground">{bytes(e.used_bytes)}</span> / {e.monthly_bytes === null ? '不限量' : bytes(e.monthly_bytes)}</span>
                {e.monthly_bytes !== null && <Button variant="ghost" size="xs" disabled={action.busy || Boolean(writeError()) || e.used_bytes === '0'} title={e.used_bytes === '0' ? '本期还没有用量' : '把本期已用流量清零，历史记录保留'} onClick={() => { if (!writeError()) setOperation({ operation: 'reset_quota', user_id: id }) }}>重置流量</Button>}
              </div>
              {quota !== undefined && <Meter value={quota} label="本期流量使用比例" className="mt-1 h-1 max-w-none" />}
              <dl className="mt-2.5 grid grid-cols-2 gap-2 text-xs">
                <div><dt className="text-muted-foreground">下次重置</dt><dd className="mt-0.5 wrap-anywhere">{dateText(e.next_reset, e.timezone)}</dd></div>
                <div><dt className="text-muted-foreground">可使用至</dt><dd className="mt-0.5 flex flex-wrap items-center gap-x-2 wrap-anywhere"><span>{expiryText(e.expires_at)}</span><Button variant="ghost" size="xs" disabled={action.busy || Boolean(writeError())} onClick={() => { if (!writeError()) setAdjusting(true) }}>调整</Button></dd></div>
              </dl>
              <p className="text-muted-foreground mt-2 text-xs leading-relaxed" title={`本期起点 ${dateText(e.cycle_start, e.timezone)}。到期或用完流量后立即停止提供可用订阅，设备应用新配置后停止服务。设备离线或流量尚未上报时，限制不会瞬间生效。`}>按 {e.timezone} 显示。重置只恢复月度流量，不延长有效期。</p>
            </> : <p className="text-muted-foreground mt-2 text-xs leading-relaxed">未分配套餐时不限流量和期限；分配后，所有节点共用这一份额度。</p>}
          </>}
        </section>
      </div>
    </div>
    <DialogPresence>{assignment && <FormDialog title="分配套餐" onClose={() => setAssignment(null)} onSubmit={assign} busy={action.busy} submitDisabled={Boolean(packageError())} error={packageError() || action.error} submitLabel="确认分配">
      <Field label="套餐组"><NativeSelect className="w-full" name="package_group_id" required value={packageChoice} onChange={event => setPackageChoice(event.target.value)}><option value="" disabled>选择套餐</option>{packageChoice && !packages.data?.some(value => String(value.id) === packageChoice) && <option value={packageChoice}>套餐组 #{packageChoice}（已不存在，原选择保留）</option>}{packages.data?.map(p => <option key={p.id} value={p.id}>{p.name} · {p.monthly_bytes === null ? '不限量' : bytes(p.monthly_bytes)} / 月 · {p.duration_days} 天</option>)}</NativeSelect></Field>
      {packages.data?.map(p => <p className="text-muted-foreground mt-3 text-xs leading-relaxed" key={p.id}>{p.name}：{scheduleText(p)}</p>)}
      <p>替换当前套餐，期限从现在重新计算（不是续加）；本月已记录的流量不清空。</p>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">套餐与节点权限分别分配；仅分配套餐不会自动授予节点。</p>
    </FormDialog>}</DialogPresence>
    <DialogPresence>{adjusting && e && <ValidityDialog expiresAt={e.expires_at} startsAt={e.starts_at} bridged={bridged} onClose={() => setAdjusting(false)} onSubmit={until => setOperation({ operation: 'set_validity', user_id: id, until })} />}</DialogPresence>
    <DialogPresence>{operation && <OperationReview request={operation} onClose={() => setOperation(null)} onApplied={() => { setOperation(null); setAssignment(null); setAdjusting(false); dirty.current = false; assigned.reload(); entitlement.reload(); onChange(); setNotice('变更已确认保存；历史账本保留，请继续查看设备应用与订阅状态。') }} />}</DialogPresence>
  </Panel>
}
