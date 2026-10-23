import { useRef, useState } from 'react'
import { api } from '../../api'
import { ErrorNotice, Field } from '../../components'
import { useAction } from '../../hooks'
import type { Node, PluginServer } from '../../types'
import { assignmentRequestId } from './groupTypes'
import SubscriptionSources from './SubscriptionSources'
import type { ExternalNodePreview, SubscriptionHopReference } from './sourceTypes'
import type { ChainInput, ChainReceipt, HopInput, ProxyResource } from './resourceTypes'
import { endpoint } from './resourceTypes'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { SortableList, sortableItemClass } from '@/components/sortable'
import { cn } from '@/lib/utils'

type ChosenHop = { input: HopInput; name: string; address: string }
type Draft = { key: string; name: string; mode: 'new' | 'existing'; serverId: number; nodeId: number; host: string; sni: string; port: string; hops: ChosenHop[] }
type CurrentResources = { nodes: Node[]; resources: ProxyResource[]; servers: PluginServer[]; entryServerIds: number[] }
const empty = (serverId: number): Draft => ({ key: assignmentRequestId(), name: '', mode: 'new', serverId, nodeId: 0, host: '', sni: '', port: '', hops: [] })
const managedNode = (current: CurrentResources, id: number) => current.nodes.find(node => node.id === id && node.enabled !== false && node.protocol === 'vless-reality' && current.servers.some(server => server.id === node.server_id && server.enabled) && current.resources.some(resource => resource.kind === 'direct' && resource.id === node.id && resource.server_id === node.server_id && resource.enabled && resource.available && resource.protocol === node.protocol))
const existingEntry = (current: CurrentResources, id: number) => { const node = managedNode(current, id); return node && current.entryServerIds.includes(node.server_id) && current.resources.some(resource => resource.kind === 'direct' && resource.id === node.id && resource.reference_count === 0 && resource.entry_eligible === true) ? node : undefined }

// Mixed chains, created only before the source migration; afterwards the page
// header opens the ordered form and the panel refuses new mixed chains. The
// migrated checks below cover a migration that lands while this editor is open.
export default function ChainEditor({ nodes, resources, servers, availableServers, writeError, getCurrent, onClose, onSaved, sourcesMigrated }: { nodes: Node[]; resources: ProxyResource[]; servers: PluginServer[]; availableServers: PluginServer[]; writeError: () => string; getCurrent: () => CurrentResources; onClose: () => void; onSaved: (receipt: ChainReceipt) => void; sourcesMigrated?: boolean }) {
  const [drafts, setDrafts] = useState<Draft[]>([empty(servers[0]?.id ?? 0)])
  const [selectingSource, setSelectingSource] = useState<string | null>(null)
  const [error, setError] = useState('')
  const receipt = useRef<{ signature: string; id: string } | null>(null)
  const action = useAction()
  const rendered = { nodes, resources, servers: availableServers, entryServerIds: servers.map(server => server.id) }
  const entryNodes = nodes.filter(node => existingEntry(rendered, node.id))
  const selectionError = (current: CurrentResources) => {
    for (const [index, draft] of drafts.entries()) {
      if (draft.mode === 'new' && draft.serverId && !current.entryServerIds.includes(draft.serverId)) return `第 ${index + 1} 条链路的已选入口服务器已不存在、未启用或不在当前筛选中，原选择和草稿已保留，请明确重新选择。`
      if (draft.mode === 'existing' && draft.nodeId && !existingEntry(current, draft.nodeId)) return `第 ${index + 1} 条链路的已选入口已不存在、不可用或不在当前筛选中，原选择和草稿已保留，请明确重新选择。`
      if (draft.hops.some(hop => hop.input.kind === 'managed' && !managedNode(current, hop.input.node_id))) return `第 ${index + 1} 条链路的已选受管段已不可用，原选择仍保留，请明确调整后再保存。`
      if (sourcesMigrated && draft.hops.some(hop => hop.input.kind === 'subscription')) return `订阅来源已迁移，第 ${index + 1} 条链路的订阅段不能再用于混合链路；请移除订阅段，或在“链路”中创建有序链路。`
    }
    return ''
  }
  const draftError = () => writeError() || selectionError(getCurrent())
  const update = (key: string, change: Partial<Draft>) => { receipt.current = null; setError(''); setDrafts(previous => previous.map(draft => draft.key === key ? { ...draft, ...change } : draft)) }
  const addSource = (reference: SubscriptionHopReference, node: ExternalNodePreview) => {
    const draft = drafts.find(draft => draft.key === selectingSource)
    if (!draft || draft.hops.length >= 8) return
    if (draft.hops.some(hop => hop.input.kind === 'subscription' && hop.input.source_id === reference.source_id && hop.input.external_node_id === reference.external_node_id)) { setError('同一条链路不能重复使用同一个订阅节点。'); return }
    update(draft.key, { hops: [...draft.hops, { input: reference, name: node.name, address: endpoint(node.server ?? '', node.port ?? 0) }] })
    setSelectingSource(null)
  }
  const submit = () => void action.run(async () => {
    const stale = draftError(); if (stale) throw new Error(stale)
    const current = getCurrent()
    const items: ChainInput[] = drafts.map((draft, index) => {
      if (!draft.name.trim()) throw new Error(`请填写第 ${index + 1} 条链路的名称。`)
      if (!draft.hops.length || draft.hops.length > 8) throw new Error(`第 ${index + 1} 条链路需要选择 1 至 8 个有序代理段。`)
      const entryServer = draft.mode === 'new' ? current.servers.find(server => server.id === draft.serverId && current.entryServerIds.includes(server.id))?.id : existingEntry(current, draft.nodeId)?.server_id
      if (!entryServer) throw new Error(`请为第 ${index + 1} 条链路选择入口。`)
      const hosts = [entryServer, ...draft.hops.flatMap(hop => hop.input.kind === 'managed' ? [managedNode(current, hop.input.node_id)!.server_id] : [])]
      if (new Set(hosts).size !== hosts.length) throw new Error(`第 ${index + 1} 条链路重复使用了同一台受管服务器，请调整代理段。`)
      return { name: draft.name.trim(), entry: draft.mode === 'new' ? { mode: 'new', server_id: draft.serverId, public_host: draft.host.trim(), sni: draft.sni.trim(), port: draft.port ? Number(draft.port) : null } : { mode: 'existing', node_id: draft.nodeId }, hops: draft.hops.map(hop => hop.input) }
    })
    const signature = JSON.stringify(items)
    if (receipt.current?.signature !== signature) receipt.current = { signature, id: assignmentRequestId() }
    return api<ChainReceipt>('/api/plugins/sing-box/chains/batch', 'POST', { request_id: receipt.current.id, items })
  }, onSaved)
  return <Panel aria-label="创建链路">
    <PanelHeader><PanelTitle>创建链路</PanelTitle><Button type="button" variant="ghost" size="sm" disabled={action.busy} onClick={onClose}>收起编辑器</Button></PanelHeader>
    <form onSubmit={event => { event.preventDefault(); submit() }}><div className="p-4">
      <ErrorNotice message={draftError() || error || action.error} />
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">每条链路拥有独立公开入口；按入口之后的实际顺序添加受管节点或订阅节点，最后一段作为出口。一次可创建 1 至 32 条链路，全部校验通过后一起保存。</p>
      <fieldset disabled={action.busy}>{drafts.map((draft, index) => {
        const entryServer = draft.mode === 'new' ? draft.serverId : nodes.find(node => node.id === draft.nodeId)?.server_id
        const eligible = nodes.filter(node => managedNode(rendered, node.id) && node.server_id !== entryServer && !draft.hops.some(hop => hop.input.kind === 'managed' && managedNode(rendered, hop.input.node_id)?.server_id === node.server_id))
        const move = (position: number, delta: number) => { const hops = [...draft.hops]; [hops[position], hops[position + delta]] = [hops[position + delta], hops[position]]; update(draft.key, { hops }) }
        return <fieldset data-slot="chain-draft" className="my-5 min-w-0 rounded-lg border p-5 max-sm:p-3" key={draft.key}><legend className="px-2 text-sm font-semibold">链路 {index + 1}</legend>
          <div className="grid gap-x-5 sm:grid-cols-2"><Field label="链路名称"><Input required maxLength={128} value={draft.name} onChange={event => update(draft.key, { name: event.target.value })} /></Field>
            <Field label="入口方式"><NativeSelect className="w-full" aria-label="入口方式" value={draft.mode} onChange={event => update(draft.key, { mode: event.target.value as Draft['mode'] })}><option value="new">新建独立入口</option><option value="existing">使用未授权的已有入口</option></NativeSelect></Field>
            {draft.mode === 'new' ? <><Field label="入口服务器"><NativeSelect className="w-full" aria-label="入口服务器" required value={draft.serverId || ''} onChange={event => update(draft.key, { serverId: Number(event.target.value) })}><option value="" disabled>选择服务器</option>{draft.serverId && !servers.some(server => server.id === draft.serverId) && <option value={draft.serverId}>已选服务器已不存在、未启用或不在当前筛选中（原选择保留）</option>}{servers.map(server => <option key={server.id} value={server.id}>{server.name}{server.online ? '' : ' · 离线'}</option>)}</NativeSelect></Field><Field label="入口端口" hint="留空自动分配；每条链路使用不同监听端口。"><Input type="number" min={1} max={65535} step={1} value={draft.port} placeholder="自动分配" onChange={event => update(draft.key, { port: event.target.value })} /></Field><Field label="入口公开地址"><Input required maxLength={253} value={draft.host} placeholder="entry.example.com" onChange={event => update(draft.key, { host: event.target.value })} /></Field><Field label="入口握手域名"><Input required maxLength={253} value={draft.sni} placeholder="www.example.com" onChange={event => update(draft.key, { sni: event.target.value })} /></Field></> : <Field label="已有入口" hint="须为当前服务器筛选中已启用、未授权且未被链路引用的 Reality 节点；保存时再次校验。"><NativeSelect className="w-full" aria-label="已有入口" required value={draft.nodeId || ''} onChange={event => update(draft.key, { nodeId: Number(event.target.value) })}><option value="" disabled>选择已有入口</option>{draft.nodeId && !entryNodes.some(node => node.id === draft.nodeId) && <option value={draft.nodeId}>已选入口已不存在、不可用或不在当前筛选中（原选择保留）</option>}{entryNodes.map(node => <option key={node.id} value={node.id}>{node.name} · {endpoint(node.public_host, node.port)}</option>)}</NativeSelect></Field>}
          </div>
          <ol className="my-6 grid list-none gap-3 p-0"><li className="border-l-primary flex min-w-0 items-start justify-between gap-4 border-l-[3px] px-4 py-2.5 wrap-anywhere max-sm:flex-col"><strong className="block">公开入口</strong><span>{draft.mode === 'new' ? draft.port ? endpoint(draft.host || '待填写地址', Number(draft.port)) : `${draft.host || '待填写地址'}（自动端口）` : nodes.find(node => node.id === draft.nodeId)?.name ?? '尚未选择'}</span></li><SortableList items={draft.hops} getId={hop => `${hop.input.kind}-${hop.input.kind === 'managed' ? hop.input.node_id : `${hop.input.source_id}-${hop.input.external_node_id}`}`} getLabel={(_, position) => `第 ${position + 1} 段`} onMove={(from, to) => { const hops = [...draft.hops]; const [hop] = hops.splice(from, 1); hops.splice(to, 0, hop); update(draft.key, { hops }) }}>{(hop, position, itemProps, handle) => <li {...itemProps} className={cn('flex min-w-0 items-start justify-between gap-4 border-l-[3px] px-4 py-2.5 wrap-anywhere max-sm:flex-col', sortableItemClass)}><div className="flex min-w-0 items-start gap-2">{handle}<div className="min-w-0"><strong className="block">第 {position + 1} 段{position === draft.hops.length - 1 ? ' · 最终出口' : ''}：{hop.name}</strong><small className="text-muted-foreground mt-1.5 block">{hop.input.kind === 'managed' ? '受管节点' : '订阅节点'} · {hop.address}</small>{hop.input.kind === 'subscription' && <label className="mt-3 flex flex-wrap items-center gap-2.5 text-sm">更新方式<NativeSelect className="w-auto" value={hop.input.update_mode} onChange={event => update(draft.key, { hops: draft.hops.map((item, at) => at === position && item.input.kind === 'subscription' ? { ...item, input: { ...item.input, update_mode: event.target.value as SubscriptionHopReference['update_mode'] } } : item) })}><option value="follow_node">跟随所选节点更新</option><option value="pinned">固定所选版本</option></NativeSelect></label>}</div></div><div className="flex flex-wrap items-center justify-end gap-2"><Button type="button" variant="ghost" size="sm" aria-label={`第 ${position + 1} 段上移`} disabled={position === 0} onClick={() => move(position, -1)}>上移</Button><Button type="button" variant="ghost" size="sm" aria-label={`第 ${position + 1} 段下移`} disabled={position === draft.hops.length - 1} onClick={() => move(position, 1)}>下移</Button><Button type="button" variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" onClick={() => update(draft.key, { hops: draft.hops.filter((_, at) => at !== position) })}>移除此段</Button></div></li>}</SortableList></ol>
          <div className="mb-5 flex flex-wrap items-end gap-4"><Field className="mb-0 min-w-52 flex-1 max-sm:min-w-0 max-sm:basis-full" label="添加受管代理段"><NativeSelect className="w-full" aria-label="添加受管代理段" value="" disabled={draft.hops.length >= 8} onChange={event => { const node = nodes.find(node => node.id === Number(event.target.value)); if (node) update(draft.key, { hops: [...draft.hops, { input: { kind: 'managed', node_id: node.id }, name: node.name, address: endpoint(node.public_host, node.port) }] }) }}><option value="">选择其他服务器的节点</option>{eligible.map(node => <option key={node.id} value={node.id}>{node.name} · {availableServers.find(server => server.id === node.server_id)?.name ?? `服务器 #${node.server_id}`}</option>)}</NativeSelect></Field>{sourcesMigrated ? <p className="text-muted-foreground mt-3 text-xs leading-relaxed">订阅来源已迁移：混合链路不再新增订阅段，请在“链路”中创建有序链路来使用订阅节点。</p> : <Button variant="outline" type="button" disabled={draft.hops.length >= 8 || sourcesMigrated === undefined} title={sourcesMigrated === undefined ? '正在确认订阅来源状态' : undefined} onClick={() => { setError(''); setSelectingSource(draft.key) }}>从订阅来源添加一段</Button>}</div>
          {drafts.length > 1 && <Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" type="button" onClick={() => { receipt.current = null; setDrafts(previous => previous.filter(item => item.key !== draft.key)); if (selectingSource === draft.key) setSelectingSource(null) }}>移除此链路</Button>}
        </fieldset>
      })}</fieldset>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">订阅的原始路由不会导入。节点顺序与传输承载会在保存时校验；验证路径和切换完成后，链路才可进入用户订阅。只有公开入口计算用户流量。</p>
      <div className="flex flex-wrap items-center justify-end gap-2"><Button type="button" variant="outline" disabled={action.busy || drafts.length >= 32} onClick={() => { receipt.current = null; setDrafts(previous => [...previous, empty(servers[0]?.id ?? 0)]) }}>添加一条独立链路</Button><Button disabled={action.busy || Boolean(draftError())}>{action.busy ? '正在保存…' : `保存 ${drafts.length} 条链路`}</Button></div>
    </div></form>
    {selectingSource && !action.busy && <div className="p-4 border-t"><PanelHeader><h3 className="mt-3 mb-4 text-sm font-semibold">选择要添加到链路 {drafts.findIndex(draft => draft.key === selectingSource) + 1} 的订阅节点</h3><Button type="button" variant="ghost" size="sm" onClick={() => setSelectingSource(null)}>关闭选点</Button></PanelHeader><ErrorNotice message={error} /><SubscriptionSources onSelect={addSource} migrated={sourcesMigrated} /></div>}
  </Panel>
}
