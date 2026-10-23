import { useState } from 'react'
import { api } from '../api'
import { Badge, Confirm, Empty, ErrorNotice, Field, Icon, Loading, Modal, PageHeader, Refresh } from '../components'
import { resourceWriteError, useAction, useResource } from '../hooks'
import { latencyDraftError, saveLatencyTask, deleteLatencyTask, bindProbeAuthorization, changeProbe, authorizationState, monitoringOf, withMonitoring, familyLabel, latency, loss, lossLabel, networkLabel, networks, probeState, probeValue } from '../probes'
import type { Probe, ProbeOverview, TaskWriteSnapshot } from '../probes'
import { time } from '../format'
import ProbeMonitoringFields from '../servers/ProbeMonitoringFields'
import type { Server } from '../types'
import ServerPicker from '../servers/ServerPicker'
import '../servers/server-setup.css'

type Task = { id: string; spec: Probe; default_enabled: boolean; server_ids: number[]; revision: number }
const empty: Task = { id: '', spec: { id: '00000000-0000-0000-0000-000000000000', name: '', kind: 'tcp', target: '', port: 443, carrier: '', interval_secs: 30, enabled: true, monitor: null }, default_enabled: false, server_ids: [], revision: 0 }

function Editor({ task, servers, writeError, getSnapshot, onClose, onSaved }: { task: Task; servers: Server[]; writeError: (serverIds?: number[]) => string; getSnapshot: () => TaskWriteSnapshot; onClose: () => void; onSaved: () => void }) {
  const [value, setValue] = useState(task)
  const action = useAction(), editing = Boolean(task.id)
  const change = (part: Partial<Probe>) => setValue(current => ({ ...current, spec: changeProbe(current.spec, part) }))
  return <Modal className="monitoring-editor" title={editing ? `编辑延迟任务 · ${task.spec.name}` : '添加延迟任务'} onClose={onClose} busy={action.busy} wide><form onSubmit={event => { event.preventDefault(); if (writeError(value.server_ids)) return; void action.run(() => saveLatencyTask(getSnapshot(), editing ? task : null, editing ? { ...value, spec: bindProbeAuthorization(value.spec) } : value, body => api(editing ? `/api/latency-tasks/${task.id}` : '/api/latency-tasks', editing ? 'PATCH' : 'POST', body)), onSaved) }}><div className="modal-body"><ErrorNotice message={writeError(value.server_ids) || action.error} /><fieldset disabled={action.busy}>
    <div className="form-grid"><Field label="任务名称"><input required pattern=".*\S.*" maxLength={128} value={value.spec.name} onChange={event => change({ name: event.target.value })} placeholder="例如：主站连通性" /></Field><Field label="检测方式"><select disabled={editing} value={value.spec.kind} onChange={event => change({ kind: event.target.value as Probe['kind'], port: event.target.value === 'tcp' ? 443 : null })}><option value="tcp">TCP 连接</option><option value="icmp">ICMP 回显</option></select></Field><Field label="目标地址" hint="主机名、IPv4 或 IPv6，不含协议和路径。"><input required disabled={editing} maxLength={253} autoCapitalize="none" spellCheck={false} value={value.spec.target} onChange={event => change({ target: event.target.value })} placeholder="probe.example.com" /></Field>{value.spec.kind === 'tcp' && <Field label="目标端口"><input required disabled={editing} type="number" min={1} max={65535} step={1} value={value.spec.port ?? ''} onChange={event => change({ port: Number(event.target.value) })} /></Field>}<Field label="检测间隔（秒）"><input required type="number" min={10} max={3600} step={1} value={value.spec.interval_secs} onChange={event => change({ interval_secs: Number(event.target.value) })} /></Field><Field label="线路备注"><input disabled={editing} maxLength={64} value={value.spec.carrier} onChange={event => change({ carrier: event.target.value })} placeholder="例如：电信 / 联通 / 移动" /></Field></div>
    <p className="helper">{value.spec.kind === 'icmp' ? 'ICMP 测量延迟与丢包；节点需具备检测工具及权限。' : 'TCP 测量连接延迟与连接失败率，每轮尝试四次。'}{editing && '检测方式、目标和端口固定，修改名称与间隔保留已有历史。'}</p>
    <ProbeMonitoringFields value={monitoringOf(value.spec)} onChange={monitoring => setValue(current => ({ ...current, spec: withMonitoring(current.spec, monitoring) }))} editing={editing} />
    <ServerPicker servers={servers} selected={value.server_ids} onChange={server_ids => setValue(current => ({ ...current, server_ids }))} />
    <label className="server-setup-toggle"><span><strong>启用任务</strong><small>暂停后保留配置与已有历史，所有已分配服务器同步暂停。</small></span><input role="switch" type="checkbox" checked={value.spec.enabled} onChange={event => change({ enabled: event.target.checked })} /><span className="server-setup-switch" aria-hidden="true" /></label>
    <label className="server-setup-toggle"><span><strong>默认分配给新服务器</strong><small>仅影响之后新增的服务器；已有服务器按上方选择分配。</small></span><input role="switch" type="checkbox" checked={value.default_enabled} onChange={event => setValue(current => ({ ...current, default_enabled: event.target.checked }))} /><span className="server-setup-switch" aria-hidden="true" /></label>
    <p className="helper">每台服务器的独立拨测与统一任务合计最多 32 个。取消分配后再次分配会开始新的采样历史。</p>
  </fieldset></div><footer><button type="button" className="button button-secondary" disabled={action.busy} onClick={onClose}>取消</button><button className="button button-primary" disabled={action.busy || Boolean(writeError(value.server_ids))}>{action.busy ? '正在保存…' : '保存任务'}</button></footer></form></Modal>
}

export default function LatencyTasks() {
  const tasks = useResource<Task[]>('/api/latency-tasks'), servers = useResource<Server[]>('/api/servers'), overview = useResource<ProbeOverview[]>('/api/probes/overview', 15_000)
  const [serverId, setServerId] = useState(0)
  const [editing, setEditing] = useState<Task | null>(null), [removing, setRemoving] = useState<Task | null>(null)
  const action = useAction()
  const refresh = () => { tasks.reload(); servers.reload(); overview.reload() }
  const writeError = (task?: Task, serverIds?: number[]) => {
    const stale = resourceWriteError(tasks, servers)
    if (stale) return stale
    if (!task?.id && (tasks.getCurrent()?.length ?? 32) >= 32) return '统一延迟任务已达上限。'
    return latencyDraftError({ tasks, servers }, task?.id ? task : null, serverIds ?? [])
  }
  const toggle = (task: Task) => { if (writeError(task)) return; void action.run(() => saveLatencyTask({ tasks, servers }, task, { ...task, spec: { ...task.spec, enabled: !task.spec.enabled } }, body => api(`/api/latency-tasks/${task.id}`, 'PATCH', body)), tasks.reload) }
  return <><PageHeader eyebrow="监控告警" title="延迟检测" description="让服务器周期性地用 TCP / ICMP 测量到电信、联通、移动等已授权目标的延迟与丢包，结果显示在看板中。"><Refresh onClick={refresh} /><button className="button button-primary" disabled={Boolean(writeError()) || (tasks.data?.length ?? 32) >= 32} onClick={() => { if (!writeError()) setEditing(empty) }}><Icon name="plus" size={16} />添加任务</button></PageHeader><ErrorNotice message={tasks.error || servers.error || overview.error || action.error} retry={refresh} />
    <section className="panel"><div className="panel-body"><label>观测服务器<select aria-label="观测服务器" value={serverId} onChange={event => setServerId(Number(event.target.value))}><option value={0}>全部服务器</option>{servers.data?.map(server => <option key={server.id} value={server.id}>{server.name}</option>)}</select></label><p className="helper">按目标所属运营商与地区查看每台服务器的观测。时间为实际采样时间；历史值不代表当前连通性，也不用于完整三网排名。</p></div></section>
    {networks.slice(0, 3).map(([network, label]) => {
      const rows = overview.data?.filter(row => row.probe.monitor?.network === network && (!serverId || row.server_id === serverId)) ?? []
      return <section className="panel" key={network} aria-label={`${label}周期观测`}><div className="panel-heading"><h2>{label}</h2><span className="subtle">{rows.length} 个目标</span></div>{!rows.length ? <div className="panel-body">{overview.loading && !overview.data ? '正在读取…' : overview.error ? '状态未知' : '未配置'}</div> : <div className="table-wrap"><table><thead><tr><th>服务器 / 地区</th><th>目标 / 轻量方法</th><th>状态</th><th>延迟</th><th>丢包 / 连接失败率</th><th>采样时间</th></tr></thead><tbody>{rows.map(({ server_id, probe, results }) => {
        const latest = results.filter(point => point.probe_id === probe.id && point.sampled_at <= Date.now()).reduce<typeof results[number] | undefined>((last, point) => !last || point.sampled_at > last.sampled_at ? point : last, undefined)
        const state = probeState(probe, latest, Date.now(), Boolean(overview.error))
        const current = state === '最近采样' ? latest : undefined
        return <tr key={`${server_id}/${probe.id}`}><td><a href={`#/servers/${server_id}`}>{servers.data?.find(server => server.id === server_id)?.name ?? `服务器 #${server_id}`}</a><small>{probe.monitor?.region || '地区未配置'}</small></td><td>{probe.name}<small>{probe.target}{probe.port ? `:${probe.port}` : ''} · {probe.kind === 'tcp' ? 'TCP 连接' : 'ICMP 回显'} · {familyLabel(probe, latest)}</small></td><td>{state}{latest?.error && <small>{latest.error}</small>}</td><td>{latency(probeValue(current, 'latency_ms'))}</td><td>{loss(probeValue(current, 'loss_percent'))}<small>{lossLabel(probe)}</small></td><td>{latest ? time(latest.sampled_at / 1000) : '尚无采样'}</td></tr>
      })}</tbody></table></div>}</section>
    })}
    <section className="panel"><div className="panel-heading"><h2>延迟任务</h2><span className="subtle">{tasks.data?.length ?? '—'} / 32 个任务</span></div>{!tasks.data && tasks.loading ? <Loading /> : !tasks.data?.length ? <Empty icon="activity" title="尚未配置统一延迟任务" description="添加检测目标，再选择需要持续观测线路的服务器。缺少目标授权的旧任务保持暂停，历史仍可查看。" /> : <div className="table-wrap"><table><thead><tr><th>任务 / 线路</th><th>检测目标</th><th>间隔</th><th>服务器</th><th>状态</th><th>操作</th></tr></thead><tbody>{tasks.data.map(task => <tr key={task.id}><td>{task.spec.name}<small>{`${networkLabel(task.spec)} · ${task.spec.monitor?.region || '地区未配置'}${task.spec.carrier ? ` · ${task.spec.carrier}` : ''}`}</small></td><td>{task.spec.kind.toUpperCase()}<small className="mono">{task.spec.target}{task.spec.port && ` · ${task.spec.port}`}</small></td><td>{task.spec.interval_secs} 秒</td><td>{task.server_ids.length} 台<small>{task.default_enabled ? '默认分配新服务器' : '手动分配'}</small></td><td><Badge tone={task.spec.enabled && !authorizationState(task.spec) ? 'good' : 'neutral'}>{authorizationState(task.spec) ?? (task.spec.enabled ? '已启用' : '已暂停')}</Badge></td><td><div className="monitoring-actions"><button className="text-button" disabled={Boolean(writeError(task))} onClick={() => { if (!writeError(task)) setEditing(task) }}>编辑</button><button className="text-button" disabled={action.busy || Boolean(writeError(task))} onClick={() => toggle(task)}>{task.spec.enabled ? '暂停' : '启用'}</button><button className="text-button danger-text" disabled={Boolean(writeError(task))} onClick={() => { if (!writeError(task)) setRemoving(task) }}>删除</button></div></td></tr>)}</tbody></table></div>}</section>
    {editing && servers.data && <Editor key={editing.id || 'new'} task={editing} servers={servers.data} writeError={ids => writeError(editing, ids)} getSnapshot={() => ({ tasks, servers })} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); refresh() }} />}
    {removing && <Confirm title="删除延迟任务" busy={action.busy} confirmDisabled={Boolean(writeError(removing))} error={writeError(removing) || action.error} onClose={() => setRemoving(null)} onConfirm={() => { if (writeError(removing)) return; void action.run(() => deleteLatencyTask({ tasks, servers }, removing, (id, body) => api(`/api/latency-tasks/${id}`, 'DELETE', body)), () => { setRemoving(null); tasks.reload() }) }}>确认删除“{removing.spec.name}”？所有已分配服务器将在同步后停止检测，该任务将从看板移除。</Confirm>}
  </>
}
