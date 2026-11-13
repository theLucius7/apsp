import { useState } from 'react'
import { api } from '../../api'
import { Badge, Confirm, Empty, ErrorNotice, Field, Loading, Modal, PageHeader, Refresh, SettingRow, DialogPresence, InfoTip } from '../../components'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import { latencyDraftError, saveLatencyTask, deleteLatencyTask, bindProbeAuthorization, changeProbe, authorizationState, monitoringOf, withMonitoring, networkLabel } from '../probes'
import type { Probe, TaskWriteSnapshot } from '../probes'
import { ago } from '../../format'
import ProbeMonitoringFields from '../servers/ProbeMonitoringFields'
import type { Server } from '../../types'
import ServerPicker from './ServerPicker'
import { intervalError, runErrorText, runLine } from './reachability'
import type { ReachabilityOverview, ReachabilitySettings } from './reachability'

import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Checkbox } from '@/components/ui/checkbox'
import { Switch } from '@/components/ui/switch'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ActivityIcon, PlayIcon, PlusIcon } from 'lucide-react'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

const reachabilityRoot = '/api/observability/reachability'

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

function ReachabilityForm({ settings, writeError, onSaved }: { settings: ReachabilitySettings; writeError: () => string; onSaved: () => void }) {
  const [enabled, setEnabled] = useState(settings.enabled)
  const [interval, setIntervalMinutes] = useState(String(settings.interval_minutes))
  const [token, setToken] = useState('')
  const [clearToken, setClearToken] = useState(false)
  const action = useAction()
  const invalid = intervalError(interval, settings)
  const blocked = writeError()
  return <form className="p-4" onSubmit={event => {
      event.preventDefault()
      if (invalid || writeError()) return
      void action.run(() => api(`${reachabilityRoot}/settings`, 'PUT', { enabled, interval_minutes: Number(interval), revision: settings.revision, ...(token ? { api_token: token } : {}), ...(clearToken ? { clear_token: true } : {}) }), onSaved)
    }}>
      <ErrorNotice message={action.error || blocked} />
      <fieldset disabled={action.busy}>
        <SettingRow title="启用中国可达性检测" description="开启后，面板会把节点的 IP 和端口发给第三方 Globalping（api.globalping.io），由其国内外探测点发起 TCP 连接；不发送节点名称、密钥或用户信息。"><Switch checked={enabled} onCheckedChange={setEnabled} /></SettingRow>
        <div className="mt-4 grid gap-4 sm:grid-cols-2">
          <Field className="m-0" label="检测间隔（分钟）" hint={`最少 ${settings.minimum_interval_minutes} 分钟。额度不足时剩余节点顺延，不算失败。`}><Input required type="number" min={settings.minimum_interval_minutes} max={settings.maximum_interval_minutes} step={1} value={interval} onChange={event => setIntervalMinutes(event.target.value)} /></Field>
          <Field className="m-0" label="Globalping API 令牌（可选）" hint={settings.token_configured ? '已保存，只写不回显；留空保留。填写新令牌后旧令牌立即停用。' : '匿名额度为每小时 250 次测试，每个探测点算一次；令牌可提高额度。加密保存在凭据中心。'}><Input type="password" autoComplete="off" spellCheck={false} maxLength={512} disabled={clearToken} value={token} onChange={event => setToken(event.target.value.trim())} /></Field>
        </div>
        {settings.token_configured && <label className="mt-3 flex items-center gap-2 text-sm"><Checkbox checked={clearToken} onCheckedChange={checked => { setClearToken(checked === true); if (checked === true) setToken('') }} />清除已保存的令牌</label>}
        <p className="text-destructive mt-3 text-xs leading-relaxed">{invalid}</p>
      </fieldset>
      <div className="mt-3 flex justify-end"><Button disabled={action.busy || Boolean(invalid) || Boolean(blocked)}>{action.busy ? '正在保存…' : '保存设置'}</Button></div>
    </form>
}

function ReachabilityPanel({ overview, loading, onChanged, writeError }: { overview?: ReachabilityOverview; loading: boolean; onChanged: () => void; writeError: () => string }) {
  const action = useAction()
  const settings = overview?.settings
  const last = settings ? runLine(settings.last_run) : undefined
  const active = settings?.running || settings?.run_requested
  return <Panel aria-label="中国可达性">
    <PanelHeader>
      <span className="flex min-w-0 items-center gap-1.5"><PanelTitle>中国可达性</PanelTitle><InfoTip label="判定说明"><p>由第三方探测点从国内与海外向节点端口发起 TCP 连接。连续两轮失败判定为封锁，一轮恢复即解除；只用海外确认开放的对照端口判断 IP 封锁。风险提示只提醒，不阻止操作。</p></InfoTip></span>
      <Button size="sm" variant="outline" disabled={!settings?.enabled || action.busy || Boolean(active) || Boolean(writeError())} title={settings?.enabled ? undefined : '启用检测后才能运行'} onClick={() => { if (!writeError()) void action.run(() => api(`${reachabilityRoot}/run`, 'POST'), onChanged) }}><PlayIcon aria-hidden="true" />{active ? '检测进行中…' : '立即检测'}</Button>
    </PanelHeader>
    <ErrorNotice message={action.error} />
    {settings?.last_run.error && <p className="text-destructive px-4 pt-3 text-xs">{runErrorText(settings.last_run.error)}</p>}
    {last && <p className="text-muted-foreground px-4 pt-3 text-xs">最近一轮{settings?.last_run.finished_at ? `（${ago(settings.last_run.finished_at)}）` : ''}：{last}</p>}
    {settings ? <ReachabilityForm key={settings.revision} settings={settings} writeError={writeError} onSaved={onChanged} /> : loading ? <Loading variant="panel" /> : null}
  </Panel>
}

export default function LineQualitySettings() {
  const tasks = useResource<Task[]>('/api/latency-tasks'), servers = useResource<Server[]>('/api/servers')
  const actor = useResource<{ role: string }>('/api/control-center/me', 0)
  const owner = actor.data?.role === 'owner'
  const reachability = useResource<ReachabilityOverview>(owner ? reachabilityRoot : null, 15_000)
  const [editing, setEditing] = useState<Task | null>(null), [removing, setRemoving] = useState<Task | null>(null)
  const action = useAction()
  const refresh = () => { tasks.reload(); servers.reload(); if (owner) reachability.reload() }
  const writeError = (task?: Task, serverIds?: number[]) => {
    const stale = resourceWriteError(tasks, servers)
    if (stale) return stale
    if (!task?.id && (tasks.getCurrent()?.length ?? 32) >= 32) return '统一延迟任务已达上限。'
    return latencyDraftError({ tasks, servers }, task?.id ? task : null, serverIds ?? [])
  }
  const toggle = (task: Task) => { if (writeError(task)) return; void action.run(() => saveLatencyTask({ tasks, servers }, task, { ...task, spec: { ...task.spec, enabled: !task.spec.enabled } }, body => api(`/api/latency-tasks/${task.id}`, 'PATCH', body)), tasks.reload) }
  return <><PageHeader title="检测设置"><Refresh onClick={refresh} /><Button disabled={Boolean(writeError()) || (tasks.data?.length ?? 32) >= 32} onClick={() => { if (!writeError()) setEditing(empty) }}><PlusIcon aria-hidden="true" />添加任务</Button></PageHeader><ErrorNotice message={tasks.error || servers.error || reachability.error || action.error} retry={refresh} />
    <Panel aria-label="延迟任务"><PanelHeader><span className="flex min-w-0 items-center gap-1.5"><PanelTitle>延迟任务</PanelTitle><InfoTip label="任务说明"><p>一个任务把同一个检测目标分配给多台服务器，结果显示在质量概况。单台服务器自己的拨测在服务器详情 › Agent 中设置，与任务合计每台最多 32 个。</p></InfoTip></span><span className="text-muted-foreground text-xs">{tasks.data?.length ?? '—'} / 32 个任务</span></PanelHeader>{!tasks.data && tasks.loading ? <Loading variant="table" rows={3} /> : !tasks.data?.length ? <Empty icon={ActivityIcon} title="尚未配置统一延迟任务" description="添加检测目标，再选择需要持续观测线路的服务器。缺少目标授权的旧任务保持暂停，历史仍可查看。" /> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>任务 / 线路</TableHead><TableHead>检测目标</TableHead><TableHead>间隔</TableHead><TableHead>服务器</TableHead><TableHead>状态</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{tasks.data.map(task => <TableRow key={task.id}><TableCell>{task.spec.name}<small>{`${networkLabel(task.spec)} · ${task.spec.monitor?.region || '地区未配置'}${task.spec.carrier ? ` · ${task.spec.carrier}` : ''}`}</small></TableCell><TableCell>{task.spec.kind.toUpperCase()}<small className="font-mono text-xs">{task.spec.target}{task.spec.port && ` · ${task.spec.port}`}</small></TableCell><TableCell>{task.spec.interval_secs} 秒</TableCell><TableCell>{task.server_ids.length} 台<small>{task.default_enabled ? '默认分配新服务器' : '手动分配'}</small></TableCell><TableCell><Badge tone={task.spec.enabled && !authorizationState(task.spec) ? 'good' : 'neutral'}>{authorizationState(task.spec) ?? (task.spec.enabled ? '已启用' : '已暂停')}</Badge></TableCell><TableCell><div className="flex flex-wrap items-center gap-2.5"><Button variant="ghost" size="sm" disabled={Boolean(writeError(task))} onClick={() => { if (!writeError(task)) setEditing(task) }}>编辑</Button><Button variant="ghost" size="sm" disabled={action.busy || Boolean(writeError(task))} onClick={() => toggle(task)}>{task.spec.enabled ? '暂停' : '启用'}</Button><Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={Boolean(writeError(task))} onClick={() => { if (!writeError(task)) setRemoving(task) }}>删除</Button></div></TableCell></TableRow>)}</TableBody></Table></div>}</Panel>
    {owner && <ReachabilityPanel overview={reachability.data} loading={reachability.loading} onChanged={reachability.reload} writeError={() => resourceWriteError(reachability)} />}
    <DialogPresence>{editing && servers.data && <Editor key={editing.id || 'new'} task={editing} servers={servers.data} writeError={ids => writeError(editing, ids)} getSnapshot={() => ({ tasks, servers })} onClose={() => setEditing(null)} onSaved={() => { setEditing(null); refresh() }} />}</DialogPresence>
    <DialogPresence>{removing && <Confirm title="删除延迟任务" busy={action.busy} confirmDisabled={Boolean(writeError(removing))} error={writeError(removing) || action.error} onClose={() => setRemoving(null)} onConfirm={() => { if (writeError(removing)) return; void action.run(() => deleteLatencyTask({ tasks, servers }, removing, (id, body) => api(`/api/latency-tasks/${id}`, 'DELETE', body)), () => { setRemoving(null); tasks.reload() }) }}>确认删除“{removing.spec.name}”？所有已分配服务器将在同步后停止检测，该任务将从看板移除。</Confirm>}</DialogPresence>
  </>
}
