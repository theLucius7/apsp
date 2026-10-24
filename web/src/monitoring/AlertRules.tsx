import { useState } from 'react'
import { api } from '../api'
import { Badge, Confirm, ErrorNotice, Field, Loading, Modal, SettingRow, DialogPresence } from '../components'
import { useAction, useResource } from '../hooks'
import type { Server } from '../types'
import ServerPicker from '../servers/ServerPicker'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

const labels = { cpu: '处理器使用率', memory: '内存使用率', disk: '磁盘使用率', net_in: '下行速度', net_out: '上行速度' }
type Spec = { name: string; metric: keyof typeof labels; threshold: number; duration_minutes: number; aggregation: 'average' | 'continuous'; all_servers: boolean; enabled: boolean; server_ids: number[] }
type Rule = { id: string; spec: Spec; revision: number }
const empty: Rule = { id: '', revision: 0, spec: { name: '', metric: 'cpu', threshold: 90, duration_minutes: 5, aggregation: 'average', all_servers: true, enabled: true, server_ids: [] } }

function Editor({ rule, servers, onClose, onSaved }: { rule: Rule; servers: Server[]; onClose: () => void; onSaved: () => void }) {
  const [spec, setSpec] = useState(rule.spec), action = useAction()
  const change = (part: Partial<Spec>) => setSpec(current => ({ ...current, ...part }))
  const unit = spec.metric.startsWith('net_') ? 'MiB/s' : '%'
  return <Modal className="sm:max-w-[51rem]" title={rule.id ? '编辑资源告警规则' : '新增资源告警规则'} wide busy={action.busy} onClose={onClose}><form onSubmit={event => { event.preventDefault(); void action.run(() => api(rule.id ? `/api/alert-rules/${rule.id}` : '/api/alert-rules', rule.id ? 'PATCH' : 'POST', { spec: { ...spec, server_ids: spec.all_servers ? [] : spec.server_ids }, ...(rule.id ? { revision: rule.revision } : {}) }), onSaved) }}><div data-slot="modal-body" className="p-4"><ErrorNotice message={action.error} /><fieldset disabled={action.busy}>
    <Field className="m-0" label="规则名称"><Input required pattern=".*\S.*" maxLength={80} value={spec.name} onChange={event => change({ name: event.target.value })} placeholder="例如：持续高负载" /></Field>
    <div className="mb-4 grid gap-4 sm:grid-cols-2"><Field className="m-0" label="监控指标"><NativeSelect className="w-full" value={spec.metric} onChange={event => change({ metric: event.target.value as Spec['metric'] })}>{Object.entries(labels).map(([key, label]) => <option key={key} value={key}>{label}</option>)}</NativeSelect></Field><Field className="m-0" label={`阈值（${unit}）`}><Input required type="number" min={0.01} max={unit === '%' ? 100 : 1000000} step={0.01} value={spec.threshold} onChange={event => change({ threshold: Number(event.target.value) })} /></Field><Field className="m-0" label="时间窗口（分钟）"><Input required type="number" min={1} max={1440} step={1} value={spec.duration_minutes} onChange={event => change({ duration_minutes: Number(event.target.value) })} /></Field><Field className="m-0" label="判断方式"><NativeSelect className="w-full" value={spec.aggregation} onChange={event => change({ aggregation: event.target.value as Spec['aggregation'] })}><option value="average">窗口平均值达到阈值</option><option value="continuous">窗口内样本均达到阈值</option></NativeSelect></Field></div>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">按完整分钟内实际样本聚合判断：平均值按样本数加权，持续超限检查全部样本的最小值。未写完整、缺失、无效或离线时保持原告警；网速遵循服务器的网卡筛选，更改范围后等待新窗口。</p>
    <SettingRow title="启用规则" description="关闭或修改后结束旧事件，按新规则重新判断。"><Switch checked={spec.enabled} onCheckedChange={checked => change({ enabled: checked })} /></SettingRow>
    <SettingRow title="全部服务器" description="包括之后新增的服务器；关闭后可指定范围。"><Switch checked={spec.all_servers} onCheckedChange={checked => change({ all_servers: checked })} /></SettingRow>
    {!spec.all_servers && <ServerPicker servers={servers} selected={spec.server_ids} onChange={server_ids => change({ server_ids })} />}
  </fieldset></div><footer><Button type="button" variant="outline" disabled={action.busy} onClick={onClose}>取消</Button><Button disabled={action.busy || (spec.enabled && !spec.all_servers && !spec.server_ids.length)}>{action.busy ? '正在保存…' : '保存规则'}</Button></footer></form></Modal>
}

export default function AlertRules() {
  const rules = useResource<Rule[]>('/api/alert-rules', 0), servers = useResource<Server[]>('/api/servers', 0), action = useAction()
  const [editing, setEditing] = useState<Rule | null>(null), [removing, setRemoving] = useState<Rule | null>(null)
  return <Panel><PanelHeader><div><PanelTitle>资源告警规则</PanelTitle><small className="text-muted-foreground text-xs">最多 20 条，触发与恢复均写入告警事件。</small></div><Button variant="outline" size="sm" disabled={!servers.data || Boolean(servers.error) || (rules.data?.length ?? 20) >= 20} onClick={() => setEditing(empty)}>新增规则</Button></PanelHeader><div className="p-4"><ErrorNotice message={rules.error || servers.error || action.error} retry={() => { rules.reload(); servers.reload() }} />{!rules.data && rules.loading ? <Loading /> : !rules.data?.length ? <p className="text-muted-foreground text-xs">尚未配置资源规则。可监控处理器、内存、磁盘使用率和网卡速率。</p> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>规则</TableHead><TableHead>条件</TableHead><TableHead>服务器</TableHead><TableHead>状态</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{rules.data.map(rule => <TableRow key={rule.id}><TableCell>{rule.spec.name}</TableCell><TableCell>{labels[rule.spec.metric]} ≥ {rule.spec.threshold} {rule.spec.metric.startsWith('net_') ? 'MiB/s' : '%'}<small>{rule.spec.duration_minutes} 分钟 · {rule.spec.aggregation === 'continuous' ? '样本均超限' : '窗口平均'}</small></TableCell><TableCell>{rule.spec.all_servers ? '全部服务器' : `${rule.spec.server_ids.length} 台`}</TableCell><TableCell><Badge tone={rule.spec.enabled ? 'good' : 'neutral'}>{rule.spec.enabled ? '已启用' : '已暂停'}</Badge></TableCell><TableCell><div className="flex flex-wrap items-center gap-2.5"><Button variant="ghost" size="sm" disabled={!servers.data || Boolean(servers.error)} onClick={() => setEditing(rule)}>编辑</Button><Button variant="ghost" size="sm" disabled={action.busy} onClick={() => void action.run(() => api(`/api/alert-rules/${rule.id}`, 'PATCH', { spec: { ...rule.spec, enabled: !rule.spec.enabled }, revision: rule.revision }), rules.reload)}>{rule.spec.enabled ? '暂停' : '启用'}</Button><Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" onClick={() => setRemoving(rule)}>删除</Button></div></TableCell></TableRow>)}</TableBody></Table></div>}</div>
    <DialogPresence>{editing && servers.data && <Editor rule={editing} servers={servers.data} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); rules.reload() }} />}</DialogPresence>
    <DialogPresence>{removing && <Confirm title="删除资源告警规则" busy={action.busy} error={action.error} onClose={() => setRemoving(null)} onConfirm={() => void action.run(() => api(`/api/alert-rules/${removing.id}`, 'DELETE'), () => { setRemoving(null); rules.reload() })}>确认删除“{removing.spec.name}”？已有事件保留，待发送通知会取消。</Confirm>}</DialogPresence>
  </Panel>
}
