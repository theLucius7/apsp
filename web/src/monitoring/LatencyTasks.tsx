import { useState } from 'react'
import { api } from '../api'
import { Badge, Confirm, Empty, ErrorNotice, Field, Loading, Modal, PageHeader, Refresh, SettingRow } from '../components'
import { resourceWriteError, useAction, useResource } from '../hooks'
import { latencyDraftError, saveLatencyTask, deleteLatencyTask, bindProbeAuthorization, changeProbe, authorizationState, monitoringOf, withMonitoring, familyLabel, latency, loss, lossLabel, networkLabel, networks, probeState, probeValue } from '../probes'
import type { Probe, ProbeOverview, TaskWriteSnapshot } from '../probes'
import { time } from '../format'
import ProbeMonitoringFields from '../servers/ProbeMonitoringFields'
import type { Server } from '../types'
import ServerPicker from '../servers/ServerPicker'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ActivityIcon, PlusIcon } from 'lucide-react'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

type Task = { id: string; spec: Probe; default_enabled: boolean; server_ids: number[]; revision: number }
const empty: Task = { id: '', spec: { id: '00000000-0000-0000-0000-000000000000', name: '', kind: 'tcp', target: '', port: 443, carrier: '', interval_secs: 30, enabled: true, monitor: null }, default_enabled: false, server_ids: [], revision: 0 }

function Editor({ task, servers, writeError, getSnapshot, onClose, onSaved }: { task: Task; servers: Server[]; writeError: (serverIds?: number[]) => string; getSnapshot: () => TaskWriteSnapshot; onClose: () => void; onSaved: () => void }) {
  const [value, setValue] = useState(task)
  const action = useAction(), editing = Boolean(task.id)
  const change = (part: Partial<Probe>) => setValue(current => ({ ...current, spec: changeProbe(current.spec, part) }))
  return <Modal className="sm:max-w-[51rem]" title={editing ? `编辑延迟任务 · ${task.spec.name}` : '添加延迟任务'} onClose={onClose} busy={action.busy} wide><form onSubmit={event => { event.preventDefault(); if (writeError(value.server_ids)) return; void action.run(() => saveLatencyTask(getSnapshot(), editing ? task : null, editing ? { ...value, spec: bindProbeAuthorization(value.spec) } : value, body => api(editing ? `/api/latency-tasks/${task.id}` : '/api/latency-tasks', editing ? 'PATCH' : 'POST', body)), onSaved) }}><div data-slot="modal-body" className="p-4"><ErrorNotice message={writeError(value.server_ids) || action.error} /><fieldset disabled={action.busy}>
    <div className="mb-4 grid gap-4 sm:grid-cols-2"><Field className="m-0" label="任务名称"><Input required pattern=".*\S.*" maxLength={128} value={value.spec.name} onChange={event => change({ name: event.target.value })} placeholder="例如：主站连通性" /></Field><Field className="m-0" label="检测方式"><NativeSelect className="w-full" disabled={editing} value={value.spec.kind} onChange={event => change({ kind: event.target.value as Probe['kind'], port: event.target.value === 'tcp' ? 443 : null })}><option value="tcp">TCP 连接</option><option value="icmp">ICMP 回显</option></NativeSelect></Field><Field className="m-0" label="目标地址" hint="主机名、IPv4 或 IPv6，不含协议和路径。"><Input required disabled={editing} maxLength={253} autoCapitalize="none" spellCheck={false} value={value.spec.target} onChange={event => change({ target: event.target.value })} placeholder="probe.example.com" /></Field>{value.spec.kind === 'tcp' && <Field className="m-0" label="目标端口"><Input required disabled={editing} type="number" min={1} max={65535} step={1} value={value.spec.port ?? ''} onChange={event => change({ port: Number(event.target.value) })} /></Field>}<Field className="m-0" label="检测间隔（秒）"><Input required type="number" min={10} max={3600} step={1} value={value.spec.interval_secs} onChange={event => change({ interval_secs: Number(event.target.value) })} /></Field><Field className="m-0" label="线路备注"><Input disabled={editing} maxLength={64} value={value.spec.carrier} onChange={event => change({ carrier: event.target.value })} placeholder="例如：电信 / 联通 / 移动" /></Field></div>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{value.spec.kind === 'icmp' ? 'ICMP 测量延迟与丢包；节点需具备检测工具及权限。' : 'TCP 测量连接延迟与连接失败率，每轮尝试四次。'}{editing && '检测方式、目标和端口固定，修改名称与间隔保留已有历史。'}</p>
    <ProbeMonitoringFields value={monitoringOf(value.spec)} onChange={monitoring => setValue(current => ({ ...current, spec: withMonitoring(current.spec, monitoring) }))} editing={editing} />
    <ServerPicker servers={servers} selected={value.server_ids} onChange={server_ids => setValue(current => ({ ...current, server_ids }))} />
    <SettingRow title="启用任务" description="暂停后保留配置与已有历史，所有已分配服务器同步暂停。"><Switch checked={value.spec.enabled} onCheckedChange={checked => change({ enabled: checked })} /></SettingRow>
    <SettingRow title="默认分配给新服务器" description="仅影响之后新增的服务器；已有服务器按上方选择分配。"><Switch checked={value.default_enabled} onCheckedChange={checked => setValue(current => ({ ...current, default_enabled: checked }))} /></SettingRow>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">每台服务器的独立拨测与统一任务合计最多 32 个。取消分配后再次分配会开始新的采样历史。</p>
  </fieldset></div><footer><Button type="button" variant="outline" disabled={action.busy} onClick={onClose}>取消</Button><Button disabled={action.busy || Boolean(writeError(value.server_ids))}>{action.busy ? '正在保存…' : '保存任务'}</Button></footer></form></Modal>
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
  return <><PageHeader eyebrow="监控告警" title="延迟检测" description="让服务器周期性地用 TCP / ICMP 测量到电信、联通、移动等已授权目标的延迟与丢包，结果显示在看板中。"><Refresh onClick={refresh} /><Button disabled={Boolean(writeError()) || (tasks.data?.length ?? 32) >= 32} onClick={() => { if (!writeError()) setEditing(empty) }}><PlusIcon aria-hidden="true" />添加任务</Button></PageHeader><ErrorNotice message={tasks.error || servers.error || overview.error || action.error} retry={refresh} />
    <Panel><div className="p-4"><label className="grid min-w-0 gap-1.5 text-sm">观测服务器<NativeSelect className="w-full" aria-label="观测服务器" value={serverId} onChange={event => setServerId(Number(event.target.value))}><option value={0}>全部服务器</option>{servers.data?.map(server => <option key={server.id} value={server.id}>{server.name}</option>)}</NativeSelect></label><p className="text-muted-foreground mt-3 text-xs leading-relaxed">按目标所属运营商与地区查看每台服务器的观测。时间为实际采样时间；历史值不代表当前连通性，也不用于完整三网排名。</p></div></Panel>
    {networks.slice(0, 3).map(([network, label]) => {
      const rows = overview.data?.filter(row => row.probe.monitor?.network === network && (!serverId || row.server_id === serverId)) ?? []
      return <Panel key={network} aria-label={`${label}周期观测`}><PanelHeader><PanelTitle>{label}</PanelTitle><span className="text-muted-foreground text-xs">{rows.length} 个目标</span></PanelHeader>{!rows.length ? <div className="p-4">{overview.loading && !overview.data ? '正在读取…' : overview.error ? '状态未知' : '未配置'}</div> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>服务器 / 地区</TableHead><TableHead>目标 / 轻量方法</TableHead><TableHead>状态</TableHead><TableHead>延迟</TableHead><TableHead>丢包 / 连接失败率</TableHead><TableHead>采样时间</TableHead></TableRow></TableHeader><TableBody>{rows.map(({ server_id, probe, results }) => {
        const latest = results.filter(point => point.probe_id === probe.id && point.sampled_at <= Date.now()).reduce<typeof results[number] | undefined>((last, point) => !last || point.sampled_at > last.sampled_at ? point : last, undefined)
        const state = probeState(probe, latest, Date.now(), Boolean(overview.error))
        const current = state === '最近采样' ? latest : undefined
        return <TableRow key={`${server_id}/${probe.id}`}><TableCell><a className="text-primary underline-offset-4 hover:underline" href={`#/servers/${server_id}`}>{servers.data?.find(server => server.id === server_id)?.name ?? `服务器 #${server_id}`}</a><small>{probe.monitor?.region || '地区未配置'}</small></TableCell><TableCell>{probe.name}<small>{probe.target}{probe.port ? `:${probe.port}` : ''} · {probe.kind === 'tcp' ? 'TCP 连接' : 'ICMP 回显'} · {familyLabel(probe, latest)}</small></TableCell><TableCell>{state}{latest?.error && <small>{latest.error}</small>}</TableCell><TableCell>{latency(probeValue(current, 'latency_ms'))}</TableCell><TableCell>{loss(probeValue(current, 'loss_percent'))}<small>{lossLabel(probe)}</small></TableCell><TableCell>{latest ? time(latest.sampled_at / 1000) : '尚无采样'}</TableCell></TableRow>
      })}</TableBody></Table></div>}</Panel>
    })}
    <Panel><PanelHeader><PanelTitle>延迟任务</PanelTitle><span className="text-muted-foreground text-xs">{tasks.data?.length ?? '—'} / 32 个任务</span></PanelHeader>{!tasks.data && tasks.loading ? <Loading variant="table" rows={3} /> : !tasks.data?.length ? <Empty icon={ActivityIcon} title="尚未配置统一延迟任务" description="添加检测目标，再选择需要持续观测线路的服务器。缺少目标授权的旧任务保持暂停，历史仍可查看。" /> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>任务 / 线路</TableHead><TableHead>检测目标</TableHead><TableHead>间隔</TableHead><TableHead>服务器</TableHead><TableHead>状态</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{tasks.data.map(task => <TableRow key={task.id}><TableCell>{task.spec.name}<small>{`${networkLabel(task.spec)} · ${task.spec.monitor?.region || '地区未配置'}${task.spec.carrier ? ` · ${task.spec.carrier}` : ''}`}</small></TableCell><TableCell>{task.spec.kind.toUpperCase()}<small className="font-mono text-xs">{task.spec.target}{task.spec.port && ` · ${task.spec.port}`}</small></TableCell><TableCell>{task.spec.interval_secs} 秒</TableCell><TableCell>{task.server_ids.length} 台<small>{task.default_enabled ? '默认分配新服务器' : '手动分配'}</small></TableCell><TableCell><Badge tone={task.spec.enabled && !authorizationState(task.spec) ? 'good' : 'neutral'}>{authorizationState(task.spec) ?? (task.spec.enabled ? '已启用' : '已暂停')}</Badge></TableCell><TableCell><div className="flex flex-wrap items-center gap-2.5"><Button variant="ghost" size="sm" disabled={Boolean(writeError(task))} onClick={() => { if (!writeError(task)) setEditing(task) }}>编辑</Button><Button variant="ghost" size="sm" disabled={action.busy || Boolean(writeError(task))} onClick={() => toggle(task)}>{task.spec.enabled ? '暂停' : '启用'}</Button><Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={Boolean(writeError(task))} onClick={() => { if (!writeError(task)) setRemoving(task) }}>删除</Button></div></TableCell></TableRow>)}</TableBody></Table></div>}</Panel>
    {editing && servers.data && <Editor key={editing.id || 'new'} task={editing} servers={servers.data} writeError={ids => writeError(editing, ids)} getSnapshot={() => ({ tasks, servers })} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); refresh() }} />}
    {removing && <Confirm title="删除延迟任务" busy={action.busy} confirmDisabled={Boolean(writeError(removing))} error={writeError(removing) || action.error} onClose={() => setRemoving(null)} onConfirm={() => { if (writeError(removing)) return; void action.run(() => deleteLatencyTask({ tasks, servers }, removing, (id, body) => api(`/api/latency-tasks/${id}`, 'DELETE', body)), () => { setRemoving(null); tasks.reload() }) }}>确认删除“{removing.spec.name}”？所有已分配服务器将在同步后停止检测，该任务将从看板移除。</Confirm>}
  </>
}
