import { useState } from 'react'
import { api } from '../api'
import { Badge, ErrorNotice, Field, Loading, Refresh, DialogPresence } from '../components'
import { time } from '../format'
import { resourceWriteError, useAction, useResource } from '../hooks'
import type { Node } from '../types'
import type { PolicyGroup, ProxyResource } from './groupTypes'
import OperationReview from './OperationReview'
import DeploymentPreflight from '../traffic/DeploymentPreflight'
import type { DeploymentPreflightState } from '../traffic/DeploymentPreflight'
import type { WorkflowOperation } from './OperationReview'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Notice, PanelHeader } from '@/components/panel'

type View = {
  runtime?: { supported_versions?: string[]; selected_version: string; reason: string; compatibility_metadata?: { upstream_release: string; upstream_commit: string; protocols: string[]; acceptance_scope: string } }
  preflight: DeploymentPreflightState
  drift: { state: string; target_revision: number | null; applied_revision: number | null; last_observed_at: number | null; reason: string; checkpoint_supported: boolean; checkpoint: { state: string; observed_at?: number | null; reason: string } }
  changes: { node_id: number; change: string; before: { name: string; public_host: string; port: number; user_ids: number[] } | null; after: { name: string; public_host: string; port: number; user_ids: number[] } | null; affected_users: number[] }[]
  history: { revision: number; created_at: number; runtime_version: string | null; artifact_sha256?: string | null }[]
  paths: { id: number; name: string; phase: string; desired_generation: number; applied_generation: number | null; candidate_generation: number | null; last_error: string | null }[]
  hop_observations: { server_id: number; name: string; last_seen: number | null; healthy: boolean | null; target_revision: number | null; applied_revision: number | null }[]
  path_diagnosis: string
}
const driftNames: Record<string, string> = { needs_preflight: '配置等待合并发布', stale: '设备离线，旧证据过期', pending_publish: '业务配置等待发布', unknown: '尚无配置证据', revision_mismatch: '运行版本与目标不同', unhealthy: '健康状态待处理', revision_aligned: '已回报版本一致', configuration_mismatch: '发现配置或运行进程偏差', configuration_verified: '实际配置与进程已核对', inspection_pending: '等待实际配置核对' }

export default function OperationsWorkflows({ serverId }: { serverId: number }) {
  const resource = useResource<View>(`/api/traffic/servers/${serverId}/operations-view`)
  const [editing, setEditing] = useState(false)
  const action = useAction()
  const [inspectionNotice, setInspectionNotice] = useState('')
  const current = resource.data
  return <section data-slot="runtime-operations" className="pt-4"><PanelHeader className="mb-3 px-0 pt-0 pb-2"><h3 className="text-sm font-semibold">配置预检、差异与链路证据</h3><Refresh onClick={resource.reload} /></PanelHeader><ErrorNotice message={resource.error || action.error} retry={resource.reload} />
    {!current ? <Loading /> : <>
      {current.runtime && <><p>内置运行时版本：{current.runtime.selected_version}。{current.runtime.reason}</p>{current.runtime.compatibility_metadata && <p className="text-muted-foreground mt-3 text-xs leading-relaxed"><a className="text-primary underline-offset-4 hover:underline" href={current.runtime.compatibility_metadata.upstream_release} target="_blank" rel="noreferrer">官方版本材料</a> · 固定上游提交 {current.runtime.compatibility_metadata.upstream_commit.slice(0, 12)}；{current.runtime.compatibility_metadata.acceptance_scope}。</p>}</>}
      <p><Badge tone={current.drift.state === 'revision_aligned' ? 'good' : 'warm'}>{driftNames[current.drift.state] ?? '状态未知'}</Badge> 目标 {current.drift.target_revision ?? '—'} / 已应用 {current.drift.applied_revision ?? '—'} · 观测 {current.drift.last_observed_at ? time(current.drift.last_observed_at) : '未提供'}</p><p className="text-muted-foreground mt-3 text-xs leading-relaxed">{current.drift.reason}</p>
      <p>{current.drift.checkpoint.reason}{current.drift.checkpoint.observed_at ? ` · 核对于 ${time(current.drift.checkpoint.observed_at)}` : ''}</p><Button variant="outline" size="sm" disabled={action.busy || Boolean(resourceWriteError(resource)) || !current.drift.checkpoint_supported || ['inspection_pending', 'pending_publish', 'needs_preflight', 'stale', 'unknown'].includes(current.drift.state)} onClick={() => void action.run(() => api(`/api/traffic/servers/${serverId}/operations-view/checkpoint`, 'POST'), () => { setInspectionNotice('已请求目标 Agent 核对实际配置文件、内置运行时和受管进程；等待回执。'); resource.reload() })}>主动核对实际配置与进程</Button>{!current.drift.checkpoint_supported && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">此 Agent 尚未声明实际配置核对能力，请先升级。</p>}{inspectionNotice && <p role="status">{inspectionNotice}</p>}
      <DeploymentPreflight serverId={serverId} current={current.preflight} unavailable={Boolean(resourceWriteError(resource))} reload={resource.reload} />
      <details className="mt-4"><summary className="cursor-pointer text-sm font-medium">业务目标与上次应用的节点差异（{current.changes.length} 项）</summary><p>按节点与关联用户显示，敏感值隐藏；链路代数和候选状态在下方分别核对。列表变更尚未表示设备应用成功。</p><div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>节点</TableHead><TableHead>变化</TableHead><TableHead>应用前 / 业务目标</TableHead><TableHead>影响用户</TableHead></TableRow></TableHeader><TableBody>{current.changes.map(change => <TableRow key={change.node_id}><TableCell>{change.after?.name ?? change.before?.name ?? `#${change.node_id}`}</TableCell><TableCell>{change.change === 'added' ? '新增' : change.change === 'removed' ? '移除' : '参数或凭据变化'}</TableCell><TableCell>{change.before ? `${change.before.public_host}:${change.before.port}` : '无'} → {change.after ? `${change.after.public_host}:${change.after.port}` : '无'}</TableCell><TableCell>{change.affected_users.join('、') || '无有效用户'}</TableCell></TableRow>)}</TableBody></Table></div></details>
      {current.paths.length > 0 && <><h4 className="text-sm font-medium">链路候选与切换确认</h4>{current.paths.map(path => <p key={path.id}><a className="text-primary underline-offset-4 hover:underline" href="#/nodes">{path.name}</a> · 目标代数 {path.desired_generation} / 应用 {path.applied_generation ?? '—'} / 候选 {path.candidate_generation ?? '无'} · {path.phase === 'applied' ? '已应用' : path.candidate_generation ? '候选流程进行中' : '查看节点页阶段'}{path.last_error ? `；${path.last_error}` : ''}</p>)}<p>{current.path_diagnosis}</p><div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>受管跳</TableHead><TableHead>部署</TableHead><TableHead>观测与检查入口</TableHead></TableRow></TableHeader><TableBody>{current.hop_observations.map(hop => <TableRow key={hop.server_id}><TableCell>{hop.name}</TableCell><TableCell>{hop.target_revision ?? '—'} / {hop.applied_revision ?? '—'} · {hop.healthy ? '健康' : '待确认'}</TableCell><TableCell>{hop.last_seen ? time(hop.last_seen) : '无连接时间'} · <a className="text-primary underline-offset-4 hover:underline" href={`#/servers/${hop.server_id}`}>资源与网络检测</a></TableCell></TableRow>)}</TableBody></Table></div></>}
      <details className="mt-4"><summary className="cursor-pointer text-sm font-medium">配置发布历史</summary>{current.history.map(record => <p key={record.revision}>{time(record.created_at)} · 配置 {record.revision}{record.runtime_version ? ` · 运行时 ${record.runtime_version}` : ''}</p>)}</details>
      <details className="mt-4" onToggle={event => { if (event.currentTarget.open) setEditing(true) }}><summary className="cursor-pointer text-sm font-medium">节点迁移与显式备选链路</summary>{editing && <MigrationAndFailoverForms serverId={serverId} onChanged={resource.reload} />}</details>
    </>}
  </section>
}

function MigrationAndFailoverForms({ serverId, onChanged }: { serverId: number; onChanged: () => void }) {
  const nodes = useResource<Node[]>('/api/traffic/nodes')
  const resources = useResource<ProxyResource[]>('/api/traffic/ordered-proxy-resources')
  const policies = useResource<PolicyGroup[]>('/api/access/policy-groups')
  const [source, setSource] = useState('')
  const [candidate, setCandidate] = useState('')
  const [sourceChain, setSourceChain] = useState('')
  const [alternateChain, setAlternateChain] = useState('')
  const [failoverReason, setFailoverReason] = useState('')
  const [selectedGroups, setSelectedGroups] = useState<number[]>([])
  const [operation, setOperation] = useState<WorkflowOperation | null>(null)
  const [notice, setNotice] = useState('')
  const error = resourceWriteError(nodes, resources, policies)
  const changed = () => { setOperation(null); onChanged(); setNotice('已保存操作，设备确认前保持待应用状态。') }
  const candidates = nodes.data?.filter(node => node.server_id !== serverId) ?? []
  const chains = resources.data?.filter(resource => resource.kind === 'chain') ?? []
  return <div className="p-4"><ErrorNotice message={nodes.error || resources.error || policies.error} retry={() => { nodes.reload(); resources.reload(); policies.reload() }} />{notice && <Notice tone="success">{notice}</Notice>}
    <h4 className="text-sm font-medium">普通节点迁移助手</h4><p>先在目标服务器克隆独立节点，确认候选服务器的当前目标部署。此处核对入口、协议、授权与订阅关系；历史用量和旧节点保留，Agent 身份不会复制。链路节点使用节点页的候选流程。</p><Field label="当前服务器上的原节点"><NativeSelect className="w-full" value={source} onChange={event => setSource(event.target.value)}><option value="">选择原节点</option>{nodes.data?.filter(node => node.server_id === serverId).map(node => <option key={node.id} value={node.id}>{node.name}</option>)}</NativeSelect></Field><Field label="另一服务器的候选节点"><NativeSelect className="w-full" value={candidate} onChange={event => setCandidate(event.target.value)}><option value="">选择独立候选</option>{candidates.map(node => <option key={node.id} value={node.id}>{node.name} · 服务器 #{node.server_id} · {node.public_host}:{node.port}</option>)}</NativeSelect></Field><Button variant="outline" disabled={Boolean(error) || !nodes.data?.some(node => node.id === Number(source) && node.server_id === serverId) || !candidates.some(node => node.id === Number(candidate))} onClick={() => setOperation({ operation: 'migrate_node', source_node_id: Number(source), candidate_node_id: Number(candidate) })}>预览迁移影响</Button>
    <h4 className="mt-4 text-sm font-medium">显式备选链路切换</h4><p>仅切换选定策略组到已经完成真实路径验证的备选链路。原链路保留，变更有记录；不会绕过链路改为直连。</p><Field label="原链路"><NativeSelect className="w-full" value={sourceChain} onChange={event => setSourceChain(event.target.value)}><option value="">选择原链路</option>{chains.map(chain => <option key={chain.id} value={chain.id}>{chain.name}</option>)}</NativeSelect></Field><Field label="明确备选链路"><NativeSelect className="w-full" value={alternateChain} onChange={event => setAlternateChain(event.target.value)}><option value="">选择已验证备选</option>{chains.filter(chain => String(chain.id) !== sourceChain).map(chain => <option key={chain.id} value={chain.id}>{chain.name}</option>)}</NativeSelect></Field><div className="my-3 mb-4 grid gap-2.5 border-0 p-0">{policies.data?.filter(policy => policy.chain_ids.includes(Number(sourceChain))).map(policy => <label key={policy.id} className="flex cursor-pointer items-center gap-3 py-2.5 text-sm"><Checkbox checked={selectedGroups.includes(policy.id)} onCheckedChange={checked => setSelectedGroups(previous => checked === true ? [...previous, policy.id] : previous.filter(id => id !== policy.id))} /><span>{policy.name}</span></label>)}</div><Field label="切换原因"><Input value={failoverReason} maxLength={128} onChange={event => setFailoverReason(event.target.value)} placeholder="例如：已确认原链路故障，切换到选定备选" /></Field><Button variant="outline" disabled={Boolean(error) || !failoverReason.trim() || !sourceChain || !alternateChain || !selectedGroups.length || selectedGroups.some(id => !policies.data?.some(policy => policy.id === id && policy.chain_ids.includes(Number(sourceChain))))} onClick={() => setOperation({ operation: 'failover', source_chain_id: Number(sourceChain), alternate_chain_id: Number(alternateChain), group_ids: [...selectedGroups], reason: failoverReason.trim() })}>预览策略组切换</Button>
    <DialogPresence>{operation && <OperationReview request={operation} onClose={() => setOperation(null)} onApplied={changed} />}</DialogPresence>
  </div>
}
