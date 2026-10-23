import { useState } from 'react'
import { api } from '../api'
import { Badge, Confirm, ErrorNotice, Field, Loading, Modal } from '../components'
import { useAction, useResource } from '../hooks'
import type { Server } from '../types'
import ServerPicker from '../servers/ServerPicker'

const labels = { cpu: '处理器使用率', memory: '内存使用率', disk: '磁盘使用率', net_in: '下行速度', net_out: '上行速度' }
type Spec = { name: string; metric: keyof typeof labels; threshold: number; duration_minutes: number; aggregation: 'average' | 'continuous'; all_servers: boolean; enabled: boolean; server_ids: number[] }
type Rule = { id: string; spec: Spec; revision: number }
const empty: Rule = { id: '', revision: 0, spec: { name: '', metric: 'cpu', threshold: 90, duration_minutes: 5, aggregation: 'average', all_servers: true, enabled: true, server_ids: [] } }

function Editor({ rule, servers, onClose, onSaved }: { rule: Rule; servers: Server[]; onClose: () => void; onSaved: () => void }) {
  const [spec, setSpec] = useState(rule.spec), action = useAction()
  const change = (part: Partial<Spec>) => setSpec(current => ({ ...current, ...part }))
  const unit = spec.metric.startsWith('net_') ? 'MiB/s' : '%'
  return <Modal className="monitoring-editor" title={rule.id ? '编辑资源告警规则' : '新增资源告警规则'} wide busy={action.busy} onClose={onClose}><form onSubmit={event => { event.preventDefault(); void action.run(() => api(rule.id ? `/api/alert-rules/${rule.id}` : '/api/alert-rules', rule.id ? 'PATCH' : 'POST', { spec: { ...spec, server_ids: spec.all_servers ? [] : spec.server_ids }, ...(rule.id ? { revision: rule.revision } : {}) }), onSaved) }}><div className="modal-body"><ErrorNotice message={action.error} /><fieldset disabled={action.busy}>
    <Field label="规则名称"><input required pattern=".*\S.*" maxLength={80} value={spec.name} onChange={event => change({ name: event.target.value })} placeholder="例如：持续高负载" /></Field>
    <div className="form-grid"><Field label="监控指标"><select value={spec.metric} onChange={event => change({ metric: event.target.value as Spec['metric'] })}>{Object.entries(labels).map(([key, label]) => <option key={key} value={key}>{label}</option>)}</select></Field><Field label={`阈值（${unit}）`}><input required type="number" min={0.01} max={unit === '%' ? 100 : 1000000} step={0.01} value={spec.threshold} onChange={event => change({ threshold: Number(event.target.value) })} /></Field><Field label="时间窗口（分钟）"><input required type="number" min={1} max={1440} step={1} value={spec.duration_minutes} onChange={event => change({ duration_minutes: Number(event.target.value) })} /></Field><Field label="判断方式"><select value={spec.aggregation} onChange={event => change({ aggregation: event.target.value as Spec['aggregation'] })}><option value="average">窗口平均值达到阈值</option><option value="continuous">窗口内样本均达到阈值</option></select></Field></div>
    <p className="helper">按完整分钟内实际样本聚合判断：平均值按样本数加权，持续超限检查全部样本的最小值。未写完整、缺失、无效或离线时保持原告警；网速遵循服务器的网卡筛选，更改范围后等待新窗口。</p>
    <label className="server-setup-toggle"><span><strong>启用规则</strong><small>关闭或修改后结束旧事件，按新规则重新判断。</small></span><input role="switch" type="checkbox" checked={spec.enabled} onChange={event => change({ enabled: event.target.checked })} /><span className="server-setup-switch" aria-hidden="true" /></label>
    <label className="server-setup-toggle"><span><strong>全部服务器</strong><small>包括之后新增的服务器；关闭后可指定范围。</small></span><input role="switch" type="checkbox" checked={spec.all_servers} onChange={event => change({ all_servers: event.target.checked })} /><span className="server-setup-switch" aria-hidden="true" /></label>
    {!spec.all_servers && <ServerPicker servers={servers} selected={spec.server_ids} onChange={server_ids => change({ server_ids })} />}
  </fieldset></div><footer><button type="button" className="button button-secondary" disabled={action.busy} onClick={onClose}>取消</button><button className="button button-primary" disabled={action.busy || (spec.enabled && !spec.all_servers && !spec.server_ids.length)}>{action.busy ? '正在保存…' : '保存规则'}</button></footer></form></Modal>
}

export default function AlertRules() {
  const rules = useResource<Rule[]>('/api/alert-rules', 0), servers = useResource<Server[]>('/api/servers', 0), action = useAction()
  const [editing, setEditing] = useState<Rule | null>(null), [removing, setRemoving] = useState<Rule | null>(null)
  return <section className="panel"><div className="panel-heading"><div><h2>资源告警规则</h2><small className="subtle">最多 20 条，触发与恢复均写入告警事件。</small></div><button className="button button-secondary button-small" disabled={!servers.data || Boolean(servers.error) || (rules.data?.length ?? 20) >= 20} onClick={() => setEditing(empty)}>新增规则</button></div><div className="panel-body"><ErrorNotice message={rules.error || servers.error || action.error} retry={() => { rules.reload(); servers.reload() }} />{!rules.data && rules.loading ? <Loading /> : !rules.data?.length ? <p className="subtle">尚未配置资源规则。可监控处理器、内存、磁盘使用率和网卡速率。</p> : <div className="table-wrap"><table><thead><tr><th>规则</th><th>条件</th><th>服务器</th><th>状态</th><th>操作</th></tr></thead><tbody>{rules.data.map(rule => <tr key={rule.id}><td>{rule.spec.name}</td><td>{labels[rule.spec.metric]} ≥ {rule.spec.threshold} {rule.spec.metric.startsWith('net_') ? 'MiB/s' : '%'}<small>{rule.spec.duration_minutes} 分钟 · {rule.spec.aggregation === 'continuous' ? '样本均超限' : '窗口平均'}</small></td><td>{rule.spec.all_servers ? '全部服务器' : `${rule.spec.server_ids.length} 台`}</td><td><Badge tone={rule.spec.enabled ? 'good' : 'neutral'}>{rule.spec.enabled ? '已启用' : '已暂停'}</Badge></td><td><div className="monitoring-actions"><button className="text-button" disabled={!servers.data || Boolean(servers.error)} onClick={() => setEditing(rule)}>编辑</button><button className="text-button" disabled={action.busy} onClick={() => void action.run(() => api(`/api/alert-rules/${rule.id}`, 'PATCH', { spec: { ...rule.spec, enabled: !rule.spec.enabled }, revision: rule.revision }), rules.reload)}>{rule.spec.enabled ? '暂停' : '启用'}</button><button className="text-button danger-text" onClick={() => setRemoving(rule)}>删除</button></div></td></tr>)}</tbody></table></div>}</div>
    {editing && servers.data && <Editor rule={editing} servers={servers.data} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); rules.reload() }} />}
    {removing && <Confirm title="删除资源告警规则" busy={action.busy} error={action.error} onClose={() => setRemoving(null)} onConfirm={() => void action.run(() => api(`/api/alert-rules/${removing.id}`, 'DELETE'), () => { setRemoving(null); rules.reload() })}>确认删除“{removing.spec.name}”？已有事件保留，待发送通知会取消。</Confirm>}
  </section>
}
