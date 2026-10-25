import { useRef, useState } from 'react'
import { api, errorMessage } from '../api'
import { ErrorNotice, Field, PageHeader, Refresh, TierBadge, TierNotice } from '../components'
import { useFeatureStore } from '@/stores/features'
import { resourceWriteError, useResource } from '../hooks'
import Certificates from './Certificates'
import Editor from './Editor'
import Relationships from './Relationships'
import ServerMigrations from './ServerMigrations'
import { frozenKinds, labels, template, time } from './types'
import type { Document, Draft, Kind, Server } from './types'

import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { SegmentedButton, SegmentedLink, SegmentedNav } from '@/components/segmented'
import { Panel } from '@/components/panel'
import { ListDetailLayout } from '@/components/list-detail'

export default function NetworkConfiguration({ serverId }: { serverId?: number }) {
  const documents = useResource<Document[]>('/api/network-configuration/documents')
  const servers = useResource<Server[]>('/api/servers')
  const [kind, setKind] = useState<Kind>('domain'), [selectedId, setSelectedId] = useState<string | null>(null)
  const showFrozen = useFeatureStore(state => state.showFrozen)
  const kinds = (Object.keys(labels) as Kind[]).filter(value => showFrozen || !frozenKinds.has(value))
  const [draft, setDraft] = useState<Draft>(template('domain', serverId)), [dirty, setDirty] = useState(false)
  const [busy, setBusy] = useState(false), [error, setError] = useState(''), [preview, setPreview] = useState<unknown>(null)
  const [evidence, setEvidence] = useState<unknown>(null), [confirmed, setConfirmed] = useState(false), [snapshot, setSnapshot] = useState(''), [operation, setOperation] = useState('')
  const [baseRevision, setBaseRevision] = useState<number | null>(null)
  const locked = useRef(false)
  const selected = documents.data?.find(item => item.id === selectedId)
  const refresh = () => { documents.reload(); servers.reload() }
  const run = async (work: () => Promise<unknown>) => { if (locked.current) return; locked.current = true; setBusy(true); setError(''); try { const value = await work(); if (value !== undefined) setEvidence(value) } catch (cause) { setError(errorMessage(cause)) } finally { locked.current = false; setBusy(false) } }
  const change = (part: Record<string, unknown>) => { setDraft(value => ({ ...value, ...part })); setDirty(true); setPreview(null) }
  const pick = (next: Document | null, nextKind = kind) => { if (dirty && !window.confirm('当前表单有未保存草稿，确认离开？')) return; setSelectedId(next?.id ?? null); setBaseRevision(next?.revision ?? null); setKind(next?.kind ?? nextKind); setDraft(next?.config ?? template(nextKind, serverId)); setDirty(false); setPreview(null); setEvidence(null); setConfirmed(false); setSnapshot(''); setOperation('') }
  const loadEvidence = () => selected && run(async () => { setEvidence(await api(`/api/network-configuration/documents/${selected.id}/observations`)) })
  const execute = (action: string) => selected && run(async () => { const error = resourceWriteError(documents, servers); if (error) throw new Error(error); const result = await api<{ operation_id: string; request: { snapshot_id?: string } }>(`/api/network-configuration/documents/${selected.id}/execute`, 'POST', { action, revision: baseRevision, snapshot_id: snapshot || null, confirmed }); setOperation(result.operation_id); if (result.request.snapshot_id) setSnapshot(result.request.snapshot_id); return result })
  const readOnly = (action: string) => ['status', 'tunnel_status', 'mesh_status', 'firewall_status'].includes(action)
  const actions: Partial<Record<Kind, string[][]>> = { forwarding: [['start', '启动临时转发'], ['stop', '停止转发'], ['status', '核对服务状态']], tuning: [['temporary', '临时应用并安排恢复'], ['confirm', '确认结果并解除恢复'], ['persist', '保存长期配置'], ['restore', '恢复原始参数']], tunnel: [['tunnel_key', '生成或读取独立隧道公钥'], ['tunnel_start', '启动反向隧道'], ['tunnel_stop', '停止反向隧道'], ['tunnel_status', '查看隧道服务状态']], mesh: [['mesh_apply', '应用私有组网'], ['mesh_status', '读取握手及传输观测'], ['mesh_stop', '停止接口'], ['mesh_restore', '回退前一配置'], ['mesh_persist', '启用开机组网']], firewall: [['firewall_temporary', '临时应用并安排恢复'], ['firewall_confirm', '确认结果并解除恢复'], ['firewall_persist', '持久化受管防火墙'], ['firewall_restore', '恢复前一受管表'], ['firewall_status', '核对实际规则']] }
  const scope = servers.data?.filter(server => !serverId || server.id === serverId) ?? []
  return <div className="grid gap-4"><PageHeader eyebrow="实验功能" title="网络配置" description="维护域名、证书、端点与端口转发的台账，并按需下发到服务器。保存配置不会改动服务器，执行变更需要单独确认。节点证书在「代理节点」中管理。"><Refresh onClick={refresh} /></PageHeader><TierNotice tier="experimental">系统网络参数、反向隧道、私有组网与受管防火墙已冻结，默认隐藏。</TierNotice>
    <ErrorNotice message={error || documents.error || servers.error} retry={refresh} />
    <ServerMigrations servers={servers.data ?? []} initialServerId={serverId} busy={busy} run={run} changed={refresh} />
    <SegmentedNav aria-label="网络配置分类">{kinds.map(value => <SegmentedButton type="button" key={value} aria-pressed={kind === value} onClick={() => pick(null, value)}>{labels[value]}{frozenKinds.has(value) && <TierBadge tier="frozen" />}</SegmentedButton>)}<SegmentedLink href="#/plugins/ddns">DNS 与 DDNS 插件</SegmentedLink></SegmentedNav>
    <ListDetailLayout id="apsp-network-configuration" label="调整台账列表宽度" listSize={260} minListSize={200} maxListSize={400}
      list={<aside data-slot="panel" aria-label={`${labels[kind]}台账`} className="bg-card text-card-foreground ring-foreground/10 min-w-0 overflow-hidden rounded-xl p-4 ring-1 max-md:flex max-md:flex-wrap max-md:gap-2"><Button variant="outline" disabled={busy} onClick={() => pick(null)}>新增{labels[kind]}</Button>{documents.data?.filter(item => item.kind === kind).map(item => <button className="aria-[current=true]:border-primary hover:bg-muted/50 focus-visible:ring-ring/50 mt-2.5 flex w-full flex-col gap-1.5 rounded-xl border bg-transparent p-3.5 text-left outline-none focus-visible:ring-3 max-md:m-0 max-md:w-auto max-md:flex-[1_1_180px] [&_span]:text-muted-foreground [&_span]:text-xs" aria-current={selectedId === item.id ? 'true' : undefined} key={item.id} onClick={() => pick(item)}><strong>{String(item.config.name)}</strong><span>版本 {item.revision} · {time(item.updated_at)}</span></button>)}</aside>}
      detail={<Panel className="mb-0 p-5">{selected && baseRevision !== selected.revision && <p className="text-destructive text-sm" role="alert">台账已被并发修改，当前草稿保留；请重新选择并核对最新配置。</p>}<form onSubmit={event => { event.preventDefault(); void run(async () => { const error = resourceWriteError(documents, servers); if (error) throw new Error(error); const saved = await api<Document>(selected ? `/api/network-configuration/documents/${selected.id}` : '/api/network-configuration/documents', selected ? 'PUT' : 'POST', { config: draft, revision: baseRevision }); setSelectedId(saved.id); setBaseRevision(saved.revision); setDraft(saved.config); setDirty(false); setPreview(null); documents.reload(); return { status: '已保存配置台账，执行需单独确认', revision: saved.revision } }) }}><fieldset disabled={busy}><Editor key={`${selectedId ?? 'new'}:${kind}`} draft={draft} change={change} servers={scope} documents={documents.data ?? []} /><div className="my-3.5 flex flex-wrap items-center gap-2.5"><Button type="button" variant="outline" disabled={!documents.fresh || !servers.fresh} onClick={() => void run(async () => { const error = resourceWriteError(documents, servers); if (error) throw new Error(error); setPreview(await api('/api/network-configuration/documents/preview', 'POST', { config: draft, revision: baseRevision })) })}>预览关联与影响</Button><Button disabled={!preview || !documents.fresh || !servers.fresh || (selected !== undefined && baseRevision !== selected.revision)}>确认预览并保存台账</Button></div></fieldset></form>
        {preview !== null && <details open><summary className="cursor-pointer text-sm font-medium">待保存配置与影响</summary><pre className="bg-muted/50 max-h-[500px] overflow-y-auto rounded-lg border p-3.5 text-xs whitespace-pre-wrap wrap-anywhere">{JSON.stringify(preview, null, 2)}</pre></details>}
      </Panel>} />
    {selected && <><Relationships document={selected} documents={documents.data ?? []} select={pick} /><Panel className="grid gap-3 p-5"><h3 className="text-sm font-semibold">独立执行与实际观测</h3><p className="text-muted-foreground mt-3 text-xs leading-relaxed">保存配置不会执行网络变更。任务排队、进程运行、端到端可达性和握手验证是不同状态；缺失结果保留为未知。</p><div className="my-3.5 flex flex-wrap items-center gap-2.5"><Button variant="outline" disabled={busy} onClick={() => void loadEvidence()}>读取结果与观测</Button><Button variant="outline" disabled={busy} onClick={() => void run(async () => setEvidence(await api(`/api/network-configuration/documents/${selected.id}/history`)))}>查看台账历史</Button></div>
      {actions[kind] && <><label className="flex items-center gap-2 text-sm"><Checkbox checked={confirmed} onCheckedChange={checked => setConfirmed(checked === true)} />确认目标服务器、资源影响与本机授权</label>
        <div className="my-3.5 flex flex-wrap items-center gap-2.5">{actions[kind]?.map(([action, label]) => <Button variant="outline" key={action} disabled={busy || dirty || !documents.fresh || !servers.fresh || (!readOnly(action) && !confirmed) || (['confirm', 'persist', 'restore', 'firewall_confirm', 'firewall_persist', 'firewall_restore', 'firewall_status'].includes(action) && !snapshot)} onClick={() => void execute(action)}>{label}</Button>)}</div>
        {(kind === 'tuning' || kind === 'firewall') && <Field label="当前临时快照标识"><Input value={snapshot} onChange={event => setSnapshot(event.target.value)} placeholder="临时应用后自动填入；也可选择历史快照" /></Field>}
        {operation && <div className="flex flex-wrap items-center justify-between gap-2.5 border-b py-2.5"><span>任务 {operation}</span><Button variant="outline" disabled={busy} onClick={() => void run(async () => setEvidence(await api(`/api/fleet/operations/${operation}`)))}>查询此任务实际结果</Button></div>}
      </>}
    </Panel>{kind === 'certificate' && <Certificates key={selected.id} document={selected} run={run} changed={() => { documents.reload(); void loadEvidence() }} />}</>}
    {kind === 'tuning' && <Panel className="grid gap-3 p-5"><h3 className="text-sm font-semibold">只读网络参数盘点</h3>{scope.map(server => <Button variant="outline" disabled={busy} key={server.id} onClick={() => void run(async () => { const result = await api<{ operation_id: string }>(`/api/network-configuration/servers/${server.id}/inventory`, 'POST'); setOperation(result.operation_id); return result })}>读取 {server.name} 参数与能力</Button>)}{!selected && operation && <Button variant="outline" disabled={busy} onClick={() => void run(async () => setEvidence(await api(`/api/fleet/operations/${operation}`)))}>查询盘点任务结果</Button>}</Panel>}
    {evidence !== null && <Panel className="grid gap-3 p-5"><details open><summary className="cursor-pointer text-sm font-medium">结构化执行证据</summary><pre className="bg-muted/50 max-h-[500px] overflow-y-auto rounded-lg border p-3.5 text-xs whitespace-pre-wrap wrap-anywhere">{JSON.stringify(evidence, null, 2)}</pre></details></Panel>}
  </div>
}
