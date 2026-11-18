import { useState } from 'react'
import { api } from '../api'
import { Badge, ErrorNotice } from '../components'
import { time } from '../format'
import { useAction } from '../hooks'
import { Button } from '@/components/ui/button'
import { Notice } from '@/components/panel'

export type DeploymentPreflightState = {
  id: string | null
  ready: boolean
  confirmed?: boolean
  device_checks_pending: boolean
  created_at?: number
  expires_at?: number
  confirmed_at?: number | null
  application?: { pending_publish?: boolean; changed_node_ids: number[]; state?: string }
  operations?: { id: string; status: string; agent_completed: boolean }[]
  checks: { key: string; name: string; state: string; blocking: boolean; observed_at: number | null; source: string; evidence: unknown; detail: string }[]
}
const names: Record<string, string> = { passed: '通过', failed: '不满足', unknown: '未知', not_applicable: '不适用' }
const phases: Record<string, string> = { queued: '排队', dispatched: '已分发，等待 Agent', succeeded: '收到成功回执', failed: '收到失败回执', unknown: '执行结果未知', cancelled: '已取消', reconciled: '人工核对结束' }

type FleetProfile = { digest: string; asset: Record<string, unknown>; policy: Record<string, unknown> & { runtime_inspection?: boolean } }

// Read-only runtime preflight inspection needs the grant on both the panel
// policy and the Agent's local fleet-policy.json.
function RuntimeInspectionGrant({ serverId, reload }: { serverId: number; reload: () => void }) {
  const action = useAction()
  const grant = () => action.run(async () => {
    const profile = await api<FleetProfile>(`/api/servers/${serverId}/fleet`)
    if (profile.policy.runtime_inspection) return
    await api(`/api/servers/${serverId}/fleet`, 'PUT', { asset: profile.asset, policy: { ...profile.policy, runtime_inspection: true }, expected_digest: profile.digest })
  }, reload)
  return <Notice><div className="grid gap-2">
    <strong className="font-medium">先允许只读运行时预检</strong>
    <p>面板策略尚未允许这台服务器做只读环境预检；预检只读取本机目录与监听状态，不修改服务器。</p>
    <p className="text-muted-foreground text-xs leading-relaxed">服务器本机也需要同样的授权：在 <code>agent.toml</code> 同目录的 <code>fleet-policy.json</code>（权限 0600）中加入 <code>"runtime_inspection": true</code>，再重启 Agent。</p>
    <ErrorNotice message={action.error} />
    <div><Button variant="outline" size="sm" disabled={action.busy} onClick={() => void grant()}>{action.busy ? '正在保存…' : '在面板策略中允许'}</Button></div>
  </div></Notice>
}

export default function DeploymentPreflight({ serverId, current, unavailable, reload }: { serverId: number; current: DeploymentPreflightState; unavailable: boolean; reload: () => void }) {
  const action = useAction()
  const [notice, setNotice] = useState('')
  const expired = current.expires_at !== undefined && current.expires_at <= Math.floor(Date.now() / 1000)
  const policyMissing = current.checks.some(check => check.key === 'policy' && check.state === 'failed')
  return <details className="min-w-0 wrap-anywhere"><summary className="cursor-pointer text-sm font-medium">只读部署环境预检</summary>
    <ErrorNotice message={action.error} />
    {policyMissing && <RuntimeInspectionGrant serverId={serverId} reload={reload} />}
    {current.application?.pending_publish && current.application.changed_node_ids.length > 0 && <p role="status">普通节点 {current.application.changed_node_ids.map(id => `#${id}`).join('、')} 的目标配置正在等待合并发布与设备应用。</p>}
    <p>按需核对入口域名的解析地址，并向 Agent 只读查询运行时目录、服务权限与监听情况。普通节点保存后自动发布并由 Agent 应用，此预检不阻塞发布。</p>
    <p>首次自动证书须满足签发条件，签发仍可能失败；部署后须通过 TLS 或 QUIC 握手检查。</p>
    <p><Badge tone={current.ready ? 'good' : 'neutral'}>{current.ready ? '最近一次环境预检通过' : expired ? '证据已过期，可重新采集' : current.id && current.device_checks_pending ? '等待目标设备回执' : current.id ? '存在未知或未满足条件' : '尚未采集环境预检'}</Badge>{current.expires_at ? ` · 有效至 ${time(current.expires_at)}` : ''}</p>
    <div className="my-3 flex flex-wrap gap-2"><Button variant="outline" size="sm" disabled={action.busy || unavailable || Boolean(current.id && current.device_checks_pending && !expired)} onClick={() => void action.run(() => api(`/api/traffic/servers/${serverId}/operations-view/preflight`, 'POST'), () => { setNotice('已登记只读预检请求，等待 Agent 实际采样；排队不表示已通过。'); reload() })}>采集部署环境预检</Button></div>
    {notice && <p role="status">{notice}</p>}
    {current.operations?.map(operation => <p key={operation.id}><code>{operation.id}</code> · {phases[operation.status] ?? operation.status}{!operation.agent_completed && operation.status === 'unknown' ? '；请从任务历史发起新的只读核对，不能把未知当成功' : ''}</p>)}
    <ul className="grid list-none gap-4 p-0 text-sm">{current.checks.map(check => <li className="grid items-start gap-3 leading-relaxed max-sm:gap-1 sm:grid-cols-[120px_1fr]" key={check.key}><Badge tone={check.state === 'passed' ? 'good' : check.state === 'failed' ? 'bad' : 'neutral'}>{check.name} · {names[check.state] ?? '未知'}</Badge><div><p className="mt-0">{check.detail}{check.blocking ? ' 请在部署排查时留意此项。' : ''}</p><p className="text-muted-foreground text-xs leading-relaxed mt-0">{check.observed_at ? time(check.observed_at) : '暂无有效采样时间'} · 来源 {check.source}</p><details><summary className="cursor-pointer text-sm font-medium">查看证据</summary><pre className="max-h-[300px] overflow-auto rounded-lg border p-3 text-sm leading-relaxed max-w-full whitespace-pre-wrap wrap-anywhere">{JSON.stringify(check.evidence, null, 2)}</pre></details></div></li>)}</ul>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">需要面板和 Agent 都启用 runtime_inspection 才能采集本机目录与监听详情。</p>
  </details>
}
