import { useEffect, useRef, useState } from 'react'
import { Badge, ErrorNotice, Field, FormDialog, Loading } from '../../components'
import { bytes } from '../../format'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import type { ResourceState } from '../../hooks'
import { assignmentRequestId, dateText, scheduleText, statusText } from './groupTypes'
import type { Entitlement, PackageGroup, PolicyGroup, UserPolicies } from './groupTypes'
import OperationReview from './OperationReview'
import type { WorkflowOperation } from './OperationReview'
import { Button, buttonVariants } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { NativeSelect } from '@/components/ui/native-select'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'

const root = '/api/plugins/sing-box'
export default function UserEntitlements({ id, entitlement, userError, refreshRevision, onChange }: { id: number; entitlement: ResourceState<Entitlement>; userError: () => string; refreshRevision: number; onChange: () => void }) {
  const policies = useResource<PolicyGroup[]>(`${root}/policy-groups`)
  const packages = useResource<PackageGroup[]>(`${root}/package-groups`)
  const assigned = useResource<UserPolicies>(`${root}/users/${id}/policy-groups`, 0)
  const [selected, setSelected] = useState<number[]>([])
  const dirty = useRef(false)
  const [assignment, setAssignment] = useState<string | null>(null)
  const [packageChoice, setPackageChoice] = useState('')
  const [notice, setNotice] = useState('')
  const action = useAction()
  const [operation, setOperation] = useState<WorkflowOperation | null>(null)
  useEffect(() => { if (assigned.data && !dirty.current) setSelected([...assigned.data.group_ids]) }, [assigned.data])
  useEffect(() => { const timer = window.setInterval(entitlement.reload, 15000); return () => window.clearInterval(timer) }, [entitlement.reload])
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
  return <Panel>
    <PanelHeader><PanelTitle>可用范围与套餐</PanelTitle><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href="#/plugins/sing-box/groups">管理策略与套餐</a></PanelHeader>
    <div className="p-4">
      <ErrorNotice message={policies.error || packages.error || assigned.error || entitlement.error || policyError() || (!assignment ? action.error : '')} retry={refresh} />
      {notice && <Notice tone="success" role="status">{notice}</Notice>}
      <h3 className="text-sm font-semibold">策略组</h3>
      {!assigned.data ? <Loading /> : policies.data?.length || selected.length ? <><div data-slot="group-choices" className="my-3 mb-4 grid gap-2.5 border-0 p-0">{policies.data?.map(p => <label className="flex cursor-pointer items-center gap-3 py-2.5 text-sm" key={p.id}><Checkbox checked={selected.includes(p.id)} disabled={action.busy} onCheckedChange={checked => { dirty.current = true; setSelected(previous => checked === true ? [...previous, p.id] : previous.filter(value => value !== p.id)) }} /><span>{p.name}<small className="text-muted-foreground mt-0.5 block text-xs">{p.node_ids.length} 个节点，{p.chain_ids.length} 条链路</small></span></label>)}{selected.filter(id => !policies.data?.some(value => value.id === id)).map(id => <label className="flex cursor-pointer items-center gap-3 py-2.5 text-sm" key={id}><Checkbox checked disabled={action.busy} onCheckedChange={() => { dirty.current = true; setSelected(previous => previous.filter(value => value !== id)) }} /><span>策略组 #{id}（已不存在，原选择保留）</span></label>)}</div><Button variant="outline" size="sm" disabled={action.busy || Boolean(policyError())} onClick={savePolicies}>{action.busy ? '正在保存…' : '保存策略组分配'}</Button></> : <p className="text-muted-foreground mt-3 text-xs leading-relaxed">尚未创建策略组，仍可使用下方的单独节点授权。</p>}
      <div className="mt-6 mb-3 flex flex-wrap items-center justify-between gap-3"><h3 className="text-sm font-semibold">当前套餐</h3><Button variant="outline" size="sm" disabled={action.busy || Boolean(writeError()) || !packages.data?.length} onClick={() => { if (writeError()) return; action.clearError(); setPackageChoice(''); setAssignment(assignmentRequestId()) }}>分配或更换套餐</Button></div>
      {!e ? <Loading /> : <>
        <div className="mt-6 mb-3 flex flex-wrap items-center justify-between gap-3"><strong>{e.package_name ?? '未设置流量与到期限制'}</strong><Badge tone={e.allowed ? 'good' : 'bad'}>{statusText[e.status]}</Badge></div>
        {e.package_group_id !== null && <>
          <dl className="my-5 grid gap-5 sm:grid-cols-2"><div><dt className="text-muted-foreground text-sm">本期已用 / 每月额度</dt><dd className="mt-1.5 font-semibold wrap-anywhere">{bytes(e.used_bytes)} / {e.monthly_bytes === null ? '不限量' : bytes(e.monthly_bytes)}</dd></div><div><dt className="text-muted-foreground text-sm">本期起点</dt><dd className="mt-1.5 font-semibold wrap-anywhere">{dateText(e.cycle_start, e.timezone)}</dd></div><div><dt className="text-muted-foreground text-sm">下次重置</dt><dd className="mt-1.5 font-semibold wrap-anywhere">{dateText(e.next_reset, e.timezone)}</dd></div><div><dt className="text-muted-foreground text-sm">可使用至</dt><dd className="mt-1.5 font-semibold wrap-anywhere">{dateText(e.expires_at, e.timezone)}</dd></div></dl>
          <p className="text-muted-foreground mt-3 text-xs leading-relaxed">按 {e.timezone} 显示。重置只恢复月度流量，不延长套餐有效期。</p>
        </>}
        <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{e.package_group_id === null ? '未分配套餐时保留兼容模式，不限制流量或有效期。分配套餐后，所有策略组与单独授权共享此用户的套餐额度。' : '到期或用完流量后立即停止提供可用订阅，设备应用新配置后停止服务。设备离线或流量尚未上报时，限制不会瞬间生效。'}</p>
      </>}
    </div>
    {assignment && <FormDialog title="分配套餐" onClose={() => setAssignment(null)} onSubmit={assign} busy={action.busy} submitDisabled={Boolean(packageError())} error={packageError() || action.error} submitLabel="确认分配">
      <Field label="套餐组"><NativeSelect className="w-full" name="package_group_id" required value={packageChoice} onChange={event => setPackageChoice(event.target.value)}><option value="" disabled>选择套餐</option>{packageChoice && !packages.data?.some(value => String(value.id) === packageChoice) && <option value={packageChoice}>套餐组 #{packageChoice}（已不存在，原选择保留）</option>}{packages.data?.map(p => <option key={p.id} value={p.id}>{p.name} · {p.monthly_bytes === null ? '不限量' : bytes(p.monthly_bytes)} / 月 · {p.duration_days} 天</option>)}</NativeSelect></Field>
      {packages.data?.map(p => <p className="text-muted-foreground mt-3 text-xs leading-relaxed" key={p.id}>{p.name}：{scheduleText(p)}</p>)}
      <p>此操作替换当前套餐，使用期限从现在重新计算，不是在原到期日上续加。本月已记录的流量不会清空。变更月度重置规则会重新按新规则统计当前周期。</p>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">套餐与节点权限分别分配；仅分配套餐不会自动授予节点。</p>
    </FormDialog>}
    {operation && <OperationReview request={operation} onClose={() => setOperation(null)} onApplied={() => { setOperation(null); setAssignment(null); dirty.current = false; assigned.reload(); entitlement.reload(); onChange(); setNotice('变更已确认保存；历史账本保留，请继续查看设备应用与订阅状态。') }} />}
  </Panel>
}
