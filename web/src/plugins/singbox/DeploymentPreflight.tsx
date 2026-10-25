import { useState } from 'react'
import { api } from '../../api'
import { Badge, ErrorNotice } from '../../components'
import { time } from '../../format'
import { useAction } from '../../hooks'
import { Button } from '@/components/ui/button'
import { Notice } from '@/components/panel'

export type DeploymentPreflightState = {
  id: string | null
  ready: boolean
  confirmed: boolean
  device_checks_pending: boolean
  created_at?: number
  expires_at?: number
  confirmed_at?: number | null
  application?: { requires_preflight: boolean; changed_node_ids: number[]; state: string }
  bootstrap?: { available: boolean; ready: boolean; reason: string; checks?: DeploymentPreflightState['checks'] }
  operations?: { id: string; status: string; agent_completed: boolean }[]
  checks: { key: string; name: string; state: string; blocking: boolean; observed_at: number | null; source: string; evidence: unknown; detail: string }[]
}
const names: Record<string, string> = { passed: '通过', failed: '不满足', unknown: '未知', not_applicable: '不适用' }
const phases: Record<string, string> = { queued: '排队', dispatched: '已分发，等待 Agent', succeeded: '收到成功回执', failed: '收到失败回执', unknown: '执行结果未知', cancelled: '已取消', reconciled: '人工核对结束' }

type FleetProfile = { digest: string; asset: Record<string, unknown>; policy: Record<string, unknown> & { runtime_inspection?: boolean } }

// The deployment gate needs the read-only runtime preflight grant on both
// sides. The panel half used to live only in the experimental 运维工具 page,
// so the preflight offers it in place; the Agent half stays a local file.
function RuntimeInspectionGrant({ serverId, reload }: { serverId: number; reload: () => void }) {
  const action = useAction()
  const grant = () => action.run(async () => {
    const profile = await api<FleetProfile>(`/api/servers/${serverId}/fleet`)
    if (profile.policy.runtime_inspection) return
    await api(`/api/servers/${serverId}/fleet`, 'PUT', { asset: profile.asset, policy: { ...profile.policy, runtime_inspection: true }, expected_digest: profile.digest })
  }, reload)
  return <Notice><div className="grid gap-2">
    <strong className="font-medium">先允许只读运行时预检</strong>
    <p>面板策略还没有允许这台服务器的只读运行时预检，部署无法确认。允许后只读取运行时目录、服务权限和监听证据，不修改服务器。</p>
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
  // The gate opens by itself when a new listener waits for it.
  return <details className="min-w-0 wrap-anywhere" open={Boolean(current.application?.requires_preflight) || undefined}><summary className="cursor-pointer text-sm font-medium">完整部署预检与确认</summary>
    <ErrorNotice message={action.error} />
    {policyMissing && <RuntimeInspectionGrant serverId={serverId} reload={reload} />}
    {current.application?.requires_preflight && <p role="status">普通节点 {current.application.changed_node_ids.map(id => `#${id}`).join('、')} 的新增监听或部署配置尚未发布，须完整预检确认；保存目标不等于设备应用。确认后自动继续重新核对并发布。</p>}
    <p>从面板实际解析入口域名，逐一核对返回的 IPv4／IPv6 地址，向目标 Agent 请求只读目录、系统服务权限与监听证据。记录当前有效授权、入口及签名制品；采集只读证据。确认后会继续已保存节点配置的正常发布，实际设备应用可能重启运行时。</p>
    <p>首次自动证书部署须满足真实签发准备条件；“已签发”和“已部署握手”另列状态。外部 CA 能否访问挑战端口尚未实测，签发仍可能失败。实际部署后，目标运行时必须完成 TLS 或 QUIC 信任链及域名握手健康检查。</p>
    <p><Badge tone={current.confirmed ? 'good' : current.ready ? 'warm' : 'neutral'}>{current.confirmed ? '当前完整预检已确认' : current.ready ? '完整预检通过，等待确认' : expired ? '证据已过期，请重新采集' : current.id && current.device_checks_pending ? '等待目标设备回执' : '存在未知或未满足条件'}</Badge>{current.expires_at ? ` · 有效至 ${time(current.expires_at)}` : ''}</p>
    <div className="my-3 flex flex-wrap gap-2"><Button variant="outline" size="sm" disabled={action.busy || unavailable || Boolean(current.id && current.device_checks_pending && !expired)} onClick={() => void action.run(() => api(`/api/plugins/sing-box/servers/${serverId}/operations-view/preflight`, 'POST'), () => { setNotice('已登记固定预检请求，等待 Agent 实际采样；排队不表示已通过。'); reload() })}>采集完整部署预检</Button>
      <Button variant="outline" size="sm" disabled={action.busy || unavailable || !current.id || !current.ready || current.confirmed || expired} onClick={() => void action.run(() => api(`/api/plugins/sing-box/servers/${serverId}/operations-view/preflight/confirm`, 'POST', { id: current.id, confirm: true }), () => { setNotice('已确认本次证据。候选发布仍会重新核对业务指纹与五分钟时效；已触发正常发布重新核对；实际应用仍等待目标设备回执。'); reload() })}>再次验证并确认继续发布</Button></div>
    {current.bootstrap?.available && <Notice><div><strong className="font-medium">首次安装空运行时</strong><p>尚无部署记录时，可先安装无业务监听及用户凭据的固定签名运行时，创建实际服务账号。保存的普通节点目标保留；安装健康回执到达后，重新采集完整预检并确认业务发布。已有部署不能使用此入口覆盖或重放。</p><p>{current.bootstrap.reason}</p><Button variant="outline" size="sm" disabled={action.busy || unavailable || !current.id || !current.bootstrap.ready || expired} onClick={() => void action.run(() => api(`/api/plugins/sing-box/servers/${serverId}/operations-view/preflight/bootstrap`, 'POST', { id: current.id, confirm: true }), () => { setNotice('已创建首次空运行时目标，等待 Agent 安装与健康回执。业务目标保留，原预检已失效；安装完成后请重新采集。'); reload() })}>再次验证并首次安装空运行时</Button>{current.bootstrap.checks && <details><summary className="cursor-pointer text-sm font-medium">首次安装逐项证据</summary>{current.bootstrap.checks.map(check => <div key={check.key}><p><Badge tone={check.state === 'passed' ? 'good' : check.state === 'failed' ? 'bad' : 'neutral'}>{check.name} · {names[check.state] ?? '未知'}</Badge> {check.detail} · {check.observed_at ? time(check.observed_at) : '无新采样'} · {check.source}</p><pre className="max-h-[300px] overflow-auto rounded-lg border p-3 text-sm leading-relaxed max-w-full whitespace-pre-wrap wrap-anywhere">{JSON.stringify(check.evidence, null, 2)}</pre></div>)}</details>}</div></Notice>}
    {notice && <p role="status">{notice}</p>}
    {current.operations?.map(operation => <p key={operation.id}><code>{operation.id}</code> · {phases[operation.status] ?? operation.status}{!operation.agent_completed && operation.status === 'unknown' ? '；请从任务历史发起新的只读核对，不能把未知当成功' : ''}</p>)}
    <ul className="grid list-none gap-4 p-0 text-sm">{current.checks.map(check => <li className="grid items-start gap-3 leading-relaxed max-sm:gap-1 sm:grid-cols-[120px_1fr]" key={check.key}><Badge tone={check.state === 'passed' ? 'good' : check.state === 'failed' ? 'bad' : 'neutral'}>{check.name} · {names[check.state] ?? '未知'}</Badge><div><p className="mt-0">{check.detail}{check.blocking ? ' 此项阻塞确认。' : ''}</p><p className="text-muted-foreground text-xs leading-relaxed mt-0">{check.observed_at ? time(check.observed_at) : '暂无有效采样时间'} · 来源 {check.source}</p><details><summary className="cursor-pointer text-sm font-medium">查看证据</summary><pre className="max-h-[300px] overflow-auto rounded-lg border p-3 text-sm leading-relaxed max-w-full whitespace-pre-wrap wrap-anywhere">{JSON.stringify(check.evidence, null, 2)}</pre></details></div></li>)}</ul>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">面板和 Agent 本机都须启用 runtime_inspection，并声明 fleet:runtime-preflight:v1。Agent 实际检查运行时目录及 data、data/certificates 的现有目录或最近已有父目录，拒绝符号链接；非 root 服务管理授权保持未知。已经占用的端口只有实际服务 PID 和当前配置核对都一致才可确认。</p>
  </details>
}
