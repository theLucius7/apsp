import { useEffect, useState } from 'react'
import { api } from '../../api'
import { Badge, ErrorNotice, Field, Loading, Refresh, DialogPresence } from '../../components'
import { bytes, time } from '../../format'
import { resourceWriteError, settingsPoll, useAction, useResource } from '../../hooks'
import OperationReview from './OperationReview'
import type { WorkflowOperation } from './OperationReview'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'

type Diagnosis = {
  account: { name: string; portal_created: boolean; keys: number; active_sessions: number; activation_expires_at: number | null }
  subscription: { status: string; message: string; granted_nodes: number; ready_managed_nodes: number; ready_external_nodes: number }
  legacy: { package_name: string | null; allowed: boolean; status: string | null; legacy_group_id: number | null; group_name?: string | null; lines: { name: string; server_id: number; taken_over: boolean; served?: boolean }[] } | null
  permissions: { node_id: number; name: string; server_id: number; enabled: boolean; effective: boolean; policy_groups: { id: number; name: string; chain_id: number | null }[]; deployment: { target_revision: number | null; applied_revision: number | null; healthy: boolean | null; pending: boolean; last_error: string | null } }[]
  ledger: { server_id: number; epoch: string; sequence: string; node_id: number; period_start: number; period_end: number; uplink: string; downlink: string; in_current_cycle: boolean | null; received_at: number | null; replay_count: string; delivery_delay_seconds: number | null }[]
  rotations: { id: string; server_id: number; node_ids: number[]; state: string; requested_at: number; applied_revision: number | null; old_downloaded_credentials_revoked: boolean }[]
  events: { id: number; action: string; created_at: number; administrator_id: number | null }[]
  limitations: Record<string, { available: boolean; reason: string }>
}
type Template = { template: { revision: number; definition: unknown } | null; limitations: string; definition_redacted: boolean; credential_access_reason: string }
const auditNames: Record<string, string> = { user_limits_updated: '修改用户限制', subscription_fetch: '获取订阅', security_subscription_reset: '重置订阅地址', security_subscription_credentials_read: '再验证后读取连接配置', security_subscription_addresses_read: '再验证后读取订阅地址', security_node_credentials_read: '再验证后读取节点连接身份', security_client_template_read: '再验证后读取完整客户端模板', client_template_update: '修改客户端模板', security_portal_login: 'Passkey 登录', security_portal_login_failed: 'Passkey 验证失败', security_portal_logout: '主动退出会话', security_portal_key_register: '登记 Passkey', security_portal_key_remove: '移除 Passkey并撤销其他会话', security_portal_invitation: '生成自助入口邀请' }

export default function UserDiagnostics({ userId, userError, onChanged, active = true }: { userId: number; userError: () => string; onChanged: () => void; active?: boolean }) {
  // A hidden section stays mounted to keep drafts but stops polling until shown again.
  const resource = useResource<Diagnosis>(`/api/plugins/sing-box/users/${userId}/diagnosis`, active ? settingsPoll : 0)
  const template = useResource<Template>(`/api/plugins/sing-box/users/${userId}/client-template`, active ? settingsPoll : 0)
  const action = useAction()
  const [days, setDays] = useState('30')
  const [rotationNodes, setRotationNodes] = useState<number[]>([])
  const [operation, setOperation] = useState<WorkflowOperation | null>(null)
  const [definition, setDefinition] = useState('')
  const [draftDirty, setDraftDirty] = useState(false)
  const [revealedRevision, setRevealedRevision] = useState<number | null>(null)
  const [client, setClient] = useState('singbox')
  const [version, setVersion] = useState('1.14.4')
  const [compatibility, setCompatibility] = useState<{ available: boolean; reason: string; runtime_validation: boolean }>()
  const [notice, setNotice] = useState('')
  useEffect(() => { if (!draftDirty && template.data) setDefinition(JSON.stringify(template.data.template?.definition ?? { selection_groups: [], dns: null, route: null }, null, 2)) }, [template.data, draftDirty])
  const disabled = action.busy || Boolean(userError() || resourceWriteError(resource))
  const e = resource.data
  const changed = () => { setOperation(null); resource.reload(); onChanged(); setNotice('变更已保存，请按设备确认状态核对实际生效结果。') }
  const saveTemplate = () => {
    if (userError() || resourceWriteError(template) || (template.data?.definition_redacted && revealedRevision === null)) return
    void action.run(async () => {
      let parsed: unknown
      try { parsed = JSON.parse(definition) } catch { throw new Error('客户端模板 JSON 格式无效；草稿保留。') }
      return api(`/api/plugins/sing-box/users/${userId}/client-template`, 'PUT', { client: 'singbox', client_version: '1.14.4', expected_revision: revealedRevision ?? template.getCurrent()?.template?.revision ?? 0, definition: parsed })
    }, () => { setDraftDirty(false); setRevealedRevision(null); template.reload(); resource.reload(); setNotice('模板已保存，后续完整 JSON 订阅会使用模板。尚未在实际客户端验证。') })
  }
  return <Panel><PanelHeader><PanelTitle>用户诊断与安全操作</PanelTitle><Refresh onClick={() => { resource.reload(); template.reload() }} /></PanelHeader><div className="p-4">
    <ErrorNotice message={resource.error || template.error || action.error || userError()} retry={resource.reload} />{notice && <Notice tone="success" role="status">{notice}</Notice>}
    {!e ? <Loading /> : <>
      <dl className="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[7rem_minmax(0,1fr)]">
        <dt className="text-muted-foreground">订阅</dt><dd className="m-0 wrap-anywhere"><Badge tone={e.subscription.status === 'ready' || e.subscription.status === 'legacy' ? 'good' : 'warm'}>{e.subscription.status === 'ready' ? '可以使用' : e.subscription.status === 'legacy' ? '原系统提供' : '不能使用'}</Badge> {e.subscription.message}</dd>
        <dt className="text-muted-foreground">自助登录</dt><dd className="m-0">{e.account.portal_created ? `已开通，Passkey ${e.account.keys} 把，当前登录 ${e.account.active_sessions} 处` : '尚未开通（可在上方「登录地址」生成开通链接）'}{e.account.activation_expires_at && <small className="text-muted-foreground ml-2">开通链接 {time(e.account.activation_expires_at)} 失效</small>}</dd>
        {!e.legacy && <><dt className="text-muted-foreground">可用节点</dt><dd className="m-0">受管 {e.subscription.ready_managed_nodes} 个、外部 {e.subscription.ready_external_nodes} 个，来自已分配的策略组</dd></>}
      </dl>
      {e.legacy ? <>
        <h3 className="mt-4 mb-2 text-sm font-semibold">原权限组中的线路</h3>
        {e.legacy.lines.length ? <ul className="grid gap-1.5 text-sm sm:grid-cols-2">{e.legacy.lines.map((line, index) => <li key={`${line.server_id}-${index}`} className="flex items-center justify-between gap-2 rounded-md border px-2.5 py-1.5"><span className="min-w-0 truncate">{line.name || `服务器 #${line.server_id}`}</span><Badge tone={line.served === false ? 'warm' : line.taken_over ? 'good' : 'neutral'}>{line.served === false ? '策略组未授权' : line.taken_over ? 'apsp 运行' : '原节点运行'}</Badge></li>)}</ul> : <p className="text-muted-foreground text-xs">所属原权限组中没有可见线路。</p>}
        <p className="text-muted-foreground mt-2 text-xs leading-relaxed">迁移用户的订阅仍由原系统生成，列出所属原权限组{e.legacy.group_name ? `（策略组「${e.legacy.group_name}」）` : ''}中的线路；已由 apsp 运行的线路还要有策略组授权。调整线路请修改用户的策略组，修改套餐不再改变线路。</p>
      </> : e.permissions.length > 0 && <>
        <h3 className="mt-4 mb-2 text-sm font-semibold">可用节点与来源</h3><div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>节点</TableHead><TableHead>来自策略组</TableHead><TableHead>状态</TableHead><TableHead>轮换凭据</TableHead></TableRow></TableHeader><TableBody>{e.permissions.map(permission => <TableRow key={permission.node_id}><TableCell>{permission.name}<small className="text-muted-foreground block">服务器 #{permission.server_id}</small></TableCell><TableCell>{permission.policy_groups.map(group => `${group.name}${group.chain_id ? `（链路 #${group.chain_id}）` : ''}`).join('、')}</TableCell><TableCell>{!permission.enabled ? '节点已停用' : !permission.effective ? '套餐不允许' : permission.deployment.pending ? '等待发布' : permission.deployment.healthy ? '已生效' : '等待设备确认'}{permission.deployment.last_error && <small className="text-destructive block">{permission.deployment.last_error}</small>}</TableCell><TableCell><Checkbox aria-label={`轮换 ${permission.name} 的连接凭据`} checked={rotationNodes.includes(permission.node_id)} disabled={disabled} onCheckedChange={checked => setRotationNodes(values => checked === true ? [...values, permission.node_id] : values.filter(value => value !== permission.node_id))} /></TableCell></TableRow>)}</TableBody></Table></div>
      </>}
      <div className="my-3 flex flex-wrap gap-2"><Field label="在原到期日后增加天数"><Input type="number" min={1} max={36500} value={days} onChange={event => setDays(event.target.value)} /></Field><Button variant="outline" disabled={disabled || !Number.isInteger(Number(days)) || Number(days) < 1 || Number(days) > 36500} onClick={() => setOperation({ operation: 'extend_validity', user_id: userId, days: Number(days) })}>预览延期</Button><Button variant="outline" disabled={disabled} onClick={() => setOperation({ operation: 'reset_quota', user_id: userId })}>预览重置本期额度</Button>{!e.legacy && e.permissions.length > 0 && <Button variant="outline" disabled={disabled || !rotationNodes.length || rotationNodes.some(id => !e.permissions.some(p => p.node_id === id))} onClick={() => setOperation({ operation: 'rotate_node_credentials', user_id: userId, node_ids: rotationNodes })}>预览轮换所选节点凭据</Button>}</div>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">延期只改到期日；额度重置采用明确补偿记录，历史用量与设备计量周期保留。订阅地址重置不会撤销已下载节点凭据。外部凭据需由提供方撤销。</p>
      {e.rotations.length > 0 && <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>轮换时间</TableHead><TableHead>设备与节点</TableHead><TableHead>设备确认</TableHead></TableRow></TableHeader><TableBody>{e.rotations.map(rotation => <TableRow key={rotation.id}><TableCell>{time(rotation.requested_at)}</TableCell><TableCell>服务器 #{rotation.server_id} · 节点 {rotation.node_ids.join('、')}</TableCell><TableCell>{rotation.state === 'device_confirmed' ? '设备已确认应用，旧下载凭据已从该版本移除' : rotation.state === 'superseded' ? '已被后续凭据变更替代' : rotation.state === 'confirmation_stale' ? '历史确认已过期，当前等待核对' : '等待设备确认'}<small>应用版本 {rotation.applied_revision ?? '—'}；现有连接是否已断开需实际检查</small></TableCell></TableRow>)}</TableBody></Table></div>}
      <details><summary className="cursor-pointer text-sm font-medium">最近 50 个计量批次</summary><p>设备 epoch 与套餐账期分别显示；批次按服务器、epoch、序号去重。新批次显示首次接收与重放次数；延迟按设备窗口结束与面板接收时刻计算，需考虑时钟偏差。旧接收时间保持未知。</p><div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>计量窗口</TableHead><TableHead>设备批次</TableHead><TableHead>节点</TableHead><TableHead>上传 / 下载</TableHead><TableHead>接收与重放</TableHead><TableHead>当前账期</TableHead></TableRow></TableHeader><TableBody>{e.ledger.map((row, index) => <TableRow key={`${row.server_id}-${row.epoch}-${row.sequence}-${row.node_id}-${index}`}><TableCell>{time(row.period_start)}<small>{time(row.period_end)}</small></TableCell><TableCell>#{row.server_id} / {row.sequence}<small>{row.epoch}</small></TableCell><TableCell>#{row.node_id}</TableCell><TableCell>{bytes(row.uplink)} / {bytes(row.downlink)}</TableCell><TableCell>{row.received_at ? time(row.received_at) : '旧批次时间未知'}<small>重放 {row.replay_count} 次；延迟 {row.delivery_delay_seconds === null ? '未知' : `${row.delivery_delay_seconds} 秒`}</small></TableCell><TableCell>{row.in_current_cycle === null ? '未设置账期' : row.in_current_cycle ? '是' : '历史账期'}</TableCell></TableRow>)}</TableBody></Table></div></details>
      <details><summary className="cursor-pointer text-sm font-medium">当前能力限制与审计记录</summary>{Object.entries(e.limitations).map(([key, limit]) => <p key={key}>{limit.reason}</p>)}{e.events.map(event => <p key={event.id}>{time(event.created_at)} · {event.administrator_id ? `管理员 #${event.administrator_id}` : event.action.startsWith('security_portal_') ? '用户自助入口' : '订阅客户端'} · {auditNames[event.action] ?? '已记录管理操作'}</p>)}</details>
    </>}
    <details><summary className="cursor-pointer text-sm font-medium">高级：sing-box 完整配置模板</summary><p>{template.data?.limitations}</p><Field label="客户端"><NativeSelect className="w-full" value={client} onChange={event => { setClient(event.target.value); setCompatibility(undefined) }}><option value="singbox">sing-box</option><option value="other">其他客户端（当前未支持）</option></NativeSelect></Field><Field label="版本"><Input value={version} onChange={event => { setVersion(event.target.value); setCompatibility(undefined) }} /></Field><Button variant="outline" disabled={disabled} onClick={() => void action.run(() => api(`/api/plugins/sing-box/users/${userId}/compatibility`, 'POST', { client, version, format: 'singbox' }), value => setCompatibility(value as { available: boolean; reason: string; runtime_validation: boolean }))}>检查协议与字段兼容</Button>{compatibility && <Notice>{compatibility.available ? '配置可生成' : '当前不可用'}：{compatibility.reason}</Notice>}
      {template.data?.definition_redacted && revealedRevision === null && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">模板敏感字段已脱敏。{template.data.credential_access_reason} <Button variant="ghost" size="sm" disabled={action.busy || Boolean(userError())} onClick={() => void action.run(() => api<Template>(`/api/plugins/sing-box/users/${userId}/client-template?reveal=true`), value => { setDefinition(JSON.stringify(value.template?.definition ?? {selection_groups: [], dns: null, route: null}, null, 2)); setRevealedRevision(value.template?.revision ?? 0); setDraftDirty(true) })}>再次验证并读取完整模板</Button></p>}
      <Button variant="outline" className="mb-3" disabled={Boolean(template.data?.definition_redacted && revealedRevision === null)} onClick={() => { setDraftDirty(true); setDefinition(JSON.stringify({ selection_groups: [{ tag: '自动选择', type: 'urltest', filter: ['*'], interval: '5m', tolerance: 50 }, { tag: 'proxy', type: 'selector', outbounds: ['自动选择'], filter: ['*'] }], route: { final: 'proxy' }, clash: { 'proxy-groups': [{ name: '自动选择', type: 'url-test', filter: ['*'] }, { name: '节点选择', type: 'select', proxies: ['自动选择'], filter: ['*'] }], rules: ['MATCH,节点选择'] } }, null, 2)) }}>填入双格式自动测速模板</Button>
      <Field label="sing-box 1.14.4 模板"><Textarea rows={12} value={definition} disabled={Boolean(template.data?.definition_redacted && revealedRevision === null)} onChange={event => { setDraftDirty(true); setDefinition(event.target.value) }} spellCheck={false} /></Field><p className="text-muted-foreground mt-3 text-xs leading-relaxed">配置 selection_groups、dns、route 和 clash。分组支持 type（selector/urltest）、filter（名称包含词数组，* 表示全部）、测速地址与间隔；route.rule_set 支持远程和内联规则集。clash 使用 JSON 对象配置 proxy-groups、filter、DNS 和规则。选择组成员使用授权出站标签，例如 node-节点编号；保存核对授权引用。订阅生成还会重新核对套餐、部署及当前可用节点，失败保留草稿与原字段。</p><Button variant="outline" disabled={action.busy || Boolean(userError() || resourceWriteError(template)) || Boolean(template.data?.definition_redacted && revealedRevision === null)} onClick={saveTemplate}>保存模板并核对授权引用</Button>
    </details>
  </div><DialogPresence>{operation && <OperationReview request={operation} onClose={() => setOperation(null)} onApplied={changed} />}</DialogPresence></Panel>
}
