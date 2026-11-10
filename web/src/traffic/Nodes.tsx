import { useEffect, useRef, useState } from 'react'
import { api } from '../api'
import { Confirm, ErrorNotice, Field, FormDialog, InfoTip, PageHeader, Refresh, RefreshNotice, DialogPresence } from '../components'
import { Stat, StatStrip } from '../observability/display/ui'
import { bytes, navigate } from '../format'
import { resourceWriteError, useAction, useRefreshNotice, useResource, useResourceGroup } from '../hooks'
import type { Node, PluginServer, Usage } from '../types'
import ProtocolFields, { protocolRequest } from './ProtocolFields'
import { ConnectionFields, nodeSettingsRequest } from './NodeSettingsFields'
import Sources from './Sources'
import NodeCatalog from './NodeCatalog'
import Runtimes from './Runtimes'
import NexusNodes from './NexusNodes'
import { validNexusNodes } from './nexusNodeTypes'
import type { ResourceKey } from './resourceTypes'
import { resourceLink } from './resourceTypes'
import OrderedResources from './OrderedResources'
import type { ProxyResourceFilter, ProxyResourceServerRole, ProxyResource as OrderedResource } from '../access/groupTypes'
import { validatedSnapshot, validProxyResources } from '../access/groupTypes'
import { validCatalog } from './catalog'
import { nodeHash } from './nodeRoute'
import type { NodeView } from './nodeRoute'

import { Button, buttonVariants } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { ActivityIcon, PlusIcon, RadioTowerIcon, ServerIcon, WaypointsIcon } from 'lucide-react'
import { SegmentedLink, SegmentedNav } from '@/components/segmented'
import { Notice } from '@/components/panel'

const root = '/api/plugins/sing-box'
const views: readonly { key: NodeView; label: string }[] = [
  { key: 'catalog', label: '节点库' },
  { key: 'chains', label: '链路' },
  { key: 'sources', label: '外部来源' },
  { key: 'runtimes', label: '运行时' },
]

type Props = {
  serverId?: number
  chainsOnly?: boolean
  view?: NodeView
  selected?: ResourceKey
  initialKind?: ProxyResourceFilter
  initialServerRole?: ProxyResourceServerRole
}
type Reads = { nodes: Node[]; ordered: unknown; servers: PluginServer[]; usage: Usage; catalog: unknown }
/** The managed node a delete confirmation names; chains are deleted from the chain list. */
type Deleting = { kind: 'direct'; id: number; name: string }
// The node a save just reached; `node` is null when the response did not name it.
type SavedNode = { server: number; node: number | null; created: boolean }

export default function Nodes({ serverId, chainsOnly = false, view, selected, initialKind, initialServerRole }: Props) {
  // All sections share one read round, so a refresh never shows one section's new
  // data beside another's old data, such as a deleted chain lingering in the catalog.
  const reads = useResourceGroup<Reads>({
    nodes: `${root}/nodes`,
    ordered: `${root}/ordered-proxy-resources`,
    servers: `${root}/servers`,
    usage: `${root}/usage`,
    catalog: `${root}/node-catalog`,
  })
  const { nodes, servers, usage, catalog } = reads
  // The imported Nexus inventory is owner-only; other administrators never request it.
  const actor = useResource<{ role: string }>('/api/control-center/me', 0)
  const owner = actor.data?.role === 'owner'
  const nexus = useResource<unknown>(owner ? `${root}/nexus-nodes` : null)
  const orderedQuery = reads.ordered
  const orderedHistory = useRef<OrderedResource[] | undefined>(undefined)
  const ordered = validatedSnapshot(orderedQuery, validProxyResources, orderedHistory.current)
  if (ordered.fresh) orderedHistory.current = ordered.data
  const activeView: NodeView = view ?? (chainsOnly || selected?.kind === 'chain' ? 'chains' : 'catalog')
  // `#/nodes/{kind}/{id}` opens that node's or chain's detail over the current view.
  const selectedResource = selected
    ? ordered.data?.find(resource => resource.kind === selected.kind && resource.id === selected.id) : undefined
  const action = useAction()
  const [editor, setEditor] = useState<Node | 'new' | null>(null)
  const [draftServer, setDraftServer] = useState('')
  const [deleting, setDeleting] = useState<Deleting | null>(null)
  const [filter, setFilter] = useState(serverId === undefined ? '' : String(serverId))
  const currentFilter = useRef(filter)
  useEffect(() => {
    const value = serverId === undefined ? '' : String(serverId)
    currentFilter.current = value
    setFilter(value)
  }, [serverId])
  // Chains have their own view, so only the direct filter reaches the catalog.
  const [kind, setKind] = useState(initialKind === 'direct' ? 'direct' : '')
  useEffect(() => { setKind(initialKind === 'direct' ? 'direct' : '') }, [initialKind])
  // A server's deployment lives in its network services tab.
  const showDeployment = (server: number) => navigate(`/servers/${server}/services`)
  const [saved, setSaved] = useState<SavedNode | null>(null)
  const [sourceCreate, setSourceCreate] = useState(0)
  const [orderedCreate, setOrderedCreate] = useState(0)

  const enabledServers = servers.data?.filter(server => server.enabled) ?? []
  // Why creation is unavailable is only stated once the server list is confirmed.
  const serversKnown = servers.data !== undefined && !servers.error
  const noEnabledServer = serversKnown && enabledServers.length === 0
  const filteredServerDisabled = serversKnown && !noEnabledServer && Boolean(filter) && !enabledServers.some(server => server.id === Number(filter))
  const createBlocked = noEnabledServer || filteredServerDisabled
  const pageGuard = resourceWriteError(nodes, orderedQuery, servers)
  // Buttons disable during a refresh only after a short delay that keeps brief polls
  // from flashing; a click in that window is reported rather than silently ignored.
  const refreshNotice = useRefreshNotice()
  const canCreate = !pageGuard && enabledServers.length > 0
    && (!filter || enabledServers.some(server => server.id === Number(filter)))
  const all = ordered.data ?? []
  const inventoryCount = all.length
  const chainCount = ordered.data?.filter(resource => resource.kind === 'chain').length
  const importedNodes = validNexusNodes(nexus.data) ? nexus.data : undefined
  // Non-owners count the catalog alone; an owner's count waits for the inventory.
  const importedKnown = actor.data !== undefined && (!owner || importedNodes !== undefined)
  const catalogCount = validCatalog(catalog.data) && importedKnown ? catalog.data.length + (importedNodes?.length ?? 0) : undefined
  // The chain form writes against the ordered resource snapshot; the button follows the form's own guard, so a
  // refreshing, failed or malformed read keeps it closed.
  const canCreateChain = canCreate && !resourceWriteError(nodes, orderedQuery, servers) && validProxyResources(orderedQuery.data)
  // A node can only need a preflight once users are authorised to it, so a saved node
  // that nobody can use yet points to authorisation instead.
  const savedNodeId = saved?.node ?? null
  const savedAuthorised = savedNodeId !== null
    && (ordered.data?.find(resource => resource.kind === 'direct' && resource.id === savedNodeId)?.user_count ?? 0) > 0
  const refresh = () => { reads.nodes.reload(); if (owner) nexus.reload() }
  // Section links keep the current server scope; the catalog type filter stays local.
  const routeHash = (target: NodeView) => nodeHash({ server: filter || undefined, role: initialServerRole, view: target })

  const creationError = () => resourceWriteError(nodes, orderedQuery, servers)
    || (!servers.getCurrent()?.some(server => server.enabled && (!currentFilter.current || server.id === Number(currentFilter.current)))
      ? '当前筛选中没有可用的入口服务器，请明确选择后再创建。' : '')
  // A managed node is written only while it is still a direct node in the confirmed resource list.
  const writeError = (id?: number) => {
    const stale = resourceWriteError(nodes, orderedQuery, servers)
    if (stale) return stale
    return id !== undefined && !ordered.getCurrent?.()?.some(item => item.kind === 'direct' && item.id === id)
      ? '此代理资源已不存在，请重新选择；当前草稿已保留。' : ''
  }
  const nodeDraftError = () => editor && editor !== 'new'
    && JSON.stringify(nodes.getCurrent()?.find(value => value.id === editor.id)) !== JSON.stringify(editor)
    ? '节点配置已变化，请重新打开编辑；当前草稿已保留。' : ''
  const nodeServerError = () => editor
    && !servers.getCurrent()?.some(server => server.id === (editor === 'new' ? Number(draftServer) : editor.server_id) && server.enabled)
    ? '已选节点所属服务器已不存在或未安装 sing-box；当前草稿已保留。' : ''
  const newServerMissing = () => editor === 'new' && !servers.getCurrent()?.some(server => String(server.id) === draftServer && server.enabled)
  const editorError = editor
    ? writeError(editor === 'new' ? undefined : editor.id) || nodeDraftError() || nodeServerError()
      || (newServerMissing() ? '已选服务器已不存在或未安装 sing-box，请明确重新选择；当前草稿已保留。' : '')
    : ''
  const currentEditor = editor && editor !== 'new' ? nodes.data?.find(node => node.id === editor.id) : null
  const connectionLocked = Boolean(currentEditor?.configuration_locked || (editor && editor !== 'new' && editor.configuration_locked))
  const deletingError = deleting ? writeError(deleting.id) : ''

  const edit = (node: Node | 'new') => {
    const error = writeError(node === 'new' ? undefined : node.id) || (node === 'new' ? creationError() : '')
    if (!refreshNotice.allows(error)) return
    action.clearError()
    setDraftServer(currentFilter.current || String(servers.getCurrent()?.find(server => server.enabled)?.id ?? ''))
    setEditor(node)
  }
  // The one chain entry: it opens the chain form in the chain view.
  const openChainCreate = () => {
    if (!refreshNotice.allows(creationError())) return
    action.clearError()
    setOrderedCreate(value => value + 1)
    if (activeView !== 'chains') window.location.hash = routeHash('chains').slice(1)
  }
  const confirmDelete = (resource: Deleting) => {
    if (!refreshNotice.allows(writeError(resource.id))) return
    action.clearError()
    setDeleting(resource)
  }
  const closeDetail = () => { window.location.hash = '/nodes' }
  const submit = (form: FormData) => {
    if (!editor || writeError(editor === 'new' ? undefined : editor.id) || nodeDraftError() || nodeServerError() || newServerMissing()) return
    const fields = {
      name: String(form.get('name')).trim(),
      public_host: String(form.get('public_host')).trim(),
      sni: String(form.get('sni') ?? '').trim(),
      protocol_config: protocolRequest(form),
      enabled: form.get('enabled') === 'on',
      settings: nodeSettingsRequest(form),
    }
    const port = String(form.get('port') ?? '').trim()
    const selectedPort = port ? { port: Number(port) } : {}
    const locked = connectionLocked || (editor !== 'new' && nodes.getCurrent()?.find(node => node.id === editor.id)?.configuration_locked)
    const request = editor === 'new'
      ? { ...fields, ...selectedPort, server_id: Number(draftServer) }
      : locked ? { name: fields.name, enabled: fields.enabled } : { ...fields, ...selectedPort }
    const targetServer = editor === 'new' ? Number(draftServer) : editor.server_id
    const created = editor === 'new'
    const editedId = editor === 'new' ? null : editor.id
    const path = editor === 'new' ? `${root}/nodes` : `${root}/nodes/${editor.id}`
    void action.run(() => api<{ id?: unknown } | null>(path, editor === 'new' ? 'POST' : 'PATCH', request), receipt => {
      // A new node is named by the response; an edited one keeps its own id.
      const createdId = receipt && typeof receipt.id === 'number' && Number.isSafeInteger(receipt.id) ? receipt.id : null
      setEditor(null); setSaved({ server: targetServer, node: created ? createdId : editedId, created }); refresh()
    })
  }
  const remove = () => {
    if (!deleting || writeError(deleting.id)) return
    void action.run(() => api(`${root}/nodes/${deleting.id}`, 'DELETE'), () => { setDeleting(null); refresh() })
  }

  return <div data-slot="nodes-page" className="min-w-0" onInvalidCapture={event => {
    if (!(event.target instanceof HTMLElement)) return
    const details = event.target.closest('details')
    if (details) details.open = true
  }}>
    <PageHeader title="代理节点">
      <Refresh onClick={refresh} />
      <Button disabled={!canCreate} aria-describedby={createBlocked ? 'create-blocked-reason' : undefined} onClick={() => edit('new')}><PlusIcon aria-hidden="true" />创建节点</Button>
      <Button variant="outline" disabled={!canCreateChain} aria-describedby={createBlocked ? 'create-blocked-reason' : undefined} onClick={openChainCreate}>创建链路</Button>
      <InfoTip label="发布规则"><p>保存后等待 5 秒合并处理。新增有效监听，或协议、TLS、服务设置变化，须先在部署进度内查看差异、采集并确认完整预检；原监听内的凭据更新、撤销和停用自动处理。</p><p>未授权给任何用户的节点不会监听端口；订阅只包含设备已成功应用的配置。</p></InfoTip>
    </PageHeader>
    {createBlocked && <Notice id="create-blocked-reason" role="status"><span>{noEnabledServer ? '还没有服务器安装 sing-box，所以暂时不能创建节点或链路。' : '当前筛选的服务器尚未安装 sing-box，所以暂时不能创建节点或链路。'}</span>
      <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={noEnabledServer ? '#/servers' : `#/servers/${filter}/services`}>为服务器安装 sing-box</a></Notice>}
    <div className="mb-4"><StatStrip label="代理资源概况" columns={4}>
      <Stat icon={WaypointsIcon} label="代理资源" loading={!(ordered.data && importedNodes) && !ordered.error} value={ordered.data && importedNodes ? inventoryCount + importedNodes.length : '—'}>{importedNodes?.length ? `受管 ${inventoryCount} · 待接管 ${importedNodes.length}` : '直连节点与独立链路入口'}</Stat>
      <Stat icon={ServerIcon} label="所在服务器" loading={!(ordered.data && importedNodes) && !ordered.error} value={ordered.data && importedNodes ? new Set([...all.map(resource => resource.entry.server_id), ...importedNodes.map(node => node.server_id)]).size : '—'} />
      <Stat icon={RadioTowerIcon} label="物理监听数" loading={!nodes.data && !nodes.error} value={nodes.data ? nodes.data.length : '—'} />
      <Stat icon={ActivityIcon} label="累计代理流量" loading={!usage.data && !usage.error} value={usage.data ? bytes(usage.data.total) : '—'} />
    </StatStrip></div>
    <ErrorNotice message={ordered.error || nodes.error || servers.error || usage.error} retry={refresh} />
    {refreshNotice.visible && <RefreshNotice />}
    {saved && <Notice role="status"><span>{savedAuthorised
      ? '目标已保存。该节点已有授权用户：新增监听或协议、TLS 等设置变化须先完整预检确认才会发布，实际应用等待设备回执。'
      : saved.created
        ? '节点已保存为目标配置，尚未授权给任何代理用户，所以暂不会监听端口。先把它加入策略组，或直接授权给代理用户；设备应用配置后才开始监听。'
        : '目标已保存；该节点尚未授权给任何代理用户，暂不会监听端口，实际应用等待设备回执。'}</span>
      <div className="flex flex-wrap items-center justify-end gap-1">
        {!savedAuthorised && <>
          <a className={buttonVariants({ variant: 'outline', size: 'sm' })} href="#/groups">加入策略组</a>
          <a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href="#/users">授权给代理用户</a>
        </>}
        <Button variant="ghost" size="sm" onClick={() => showDeployment(saved.server)}>{savedAuthorised ? '查看差异、完整预检与发布状态' : '查看部署状态'}</Button>
      </div></Notice>}
    <SegmentedNav className="mt-1 mb-5" aria-label="节点视图">
      {views.map(item => <SegmentedLink key={item.key} href={routeHash(item.key)}
        aria-current={activeView === item.key ? 'page' : undefined}>
        {item.label}
        {item.key === 'catalog' && catalogCount !== undefined && <span className="bg-muted text-muted-foreground min-w-5 rounded px-1.5 text-center text-xs font-medium tabular-nums" aria-hidden="true">{catalogCount}</span>}
        {item.key === 'chains' && chainCount !== undefined && <span className="bg-muted text-muted-foreground min-w-5 rounded px-1.5 text-center text-xs font-medium tabular-nums" aria-hidden="true">{chainCount}</span>}
      </SegmentedLink>)}
    </SegmentedNav>
    <div hidden={activeView !== 'catalog'}>
      {owner && <NexusNodes query={nexus} server={filter} onReload={refresh} />}
      <NodeCatalog query={catalog} nodes={nodes.data ?? []} servers={enabledServers} getServers={() => servers.getCurrent()} usage={usage.data ?? undefined}
        server={filter} getServer={() => currentFilter.current} onServer={value => { currentFilter.current = value; setFilter(value) }}
        kind={kind} onKind={setKind}
        onChanged={refresh} managedWriteError={() => writeError() || resourceWriteError(ordered)}
        onEdit={edit} onDelete={confirmDelete} onDeployment={showDeployment} />
    </div>
    <div hidden={activeView !== 'chains'}>
      <OrderedResources resourcesQuery={orderedQuery} nodesQuery={nodes} serversQuery={servers} usage={usage}
        selected={selectedResource} onCloseSelected={closeDetail}
        filter={filter} initialServerRole={initialServerRole} getServerId={() => currentFilter.current ? Number(currentFilter.current) : undefined}
        onEdit={edit} onChanged={refresh} onAddSource={() => { setSourceCreate(value => value + 1); window.location.hash = routeHash('sources').slice(1) }} createRequest={orderedCreate} />
    </div>
    <div hidden={activeView !== 'sources'}>
      <Sources createRequest={sourceCreate} onCatalogChange={refresh} active={activeView === 'sources'} />
    </div>
    <div hidden={activeView !== 'runtimes'}>{activeView === 'runtimes' && <Runtimes />}</div>
    <DialogPresence>{editor && <FormDialog wide slot="node-editor" title={editor === 'new' ? '创建节点' : '编辑节点'} onClose={() => setEditor(null)} onSubmit={submit}
      busy={action.busy} submitDisabled={Boolean(editorError)} error={editorError || action.error} submitLabel={editor === 'new' ? '创建目标节点' : '保存目标配置'}>
      <h3 className="mt-1.5 mb-4 text-sm font-semibold">基本信息</h3>
      <div className="grid gap-x-5 sm:grid-cols-2">
        <Field label="节点名称"><Input name="name" required maxLength={128} defaultValue={editor === 'new' ? '' : editor.name} placeholder="例如：香港 · 直连" autoComplete="off" /></Field>
        {editor === 'new' && <Field label="所属服务器"><NativeSelect className="w-full" name="server_id" required value={draftServer} onChange={event => setDraftServer(event.target.value)}>
          {draftServer && !enabledServers.some(server => String(server.id) === draftServer) && <option value={draftServer}>已选服务器已不存在或未安装 sing-box（原选择保留）</option>}
          {enabledServers.map(server => <option key={server.id} value={server.id}>{server.name}{server.online ? ' · 在线' : ' · 离线'}</option>)}
        </NativeSelect></Field>}
        <label className="flex items-start gap-2.5 pt-3 pb-5 text-sm"><Checkbox name="enabled" defaultChecked={editor === 'new' || editor.enabled !== false} className="mt-0.5" /><span>启用节点<small className="text-muted-foreground mt-1.5 block text-xs leading-relaxed">停用保留授权与历史流量。</small></span></label>
      </div>
      {connectionLocked && <div className="bg-muted mb-5 rounded-lg px-4 py-3 text-sm leading-relaxed" role="status">此节点已被链路引用，当前可修改名称和启用状态。调整连接参数请先替换链路中的节点。<br />
        {(currentEditor?.referenced_chains ?? (editor === 'new' ? [] : editor.referenced_chains) ?? []).map(chain =>
          <a className="mr-3" key={chain.id} href={resourceLink({ kind: 'chain', id: chain.id })} onClick={() => setEditor(null)}>{chain.name}</a>)}
      </div>}
      <fieldset className="m-0 min-w-0 border-0 p-0 disabled:opacity-65" disabled={connectionLocked}>
        <h3 className="mt-1.5 mb-4 text-sm font-semibold">连接地址</h3>
        <div className="grid gap-x-5 sm:grid-cols-2">
          <Field label="监听端口" hint={editor === 'new' ? '可填写 443 等端口；留空时从 20000–29999 自动分配。18085 为保留端口。' : '修改后客户端需更新订阅；服务器上已有其他服务占用的端口不可使用。'}>
            <Input name="port" type="number" min={1} max={65535} step={1} required={editor !== 'new'} defaultValue={editor === 'new' ? '' : editor.port} placeholder="自动分配" />
          </Field>
          <Field label="公开地址" hint="填写客户端连接使用的域名或 IP，不含协议、端口和路径。">
            <Input name="public_host" required defaultValue={editor === 'new' ? '' : editor.public_host} placeholder="node.example.com" autoComplete="off" spellCheck={false} />
          </Field>
          <ConnectionFields node={editor} />
        </div>
        <h3 className="mt-1.5 mb-4 text-sm font-semibold">协议与安全</h3>
        <ProtocolFields key={editor === 'new' ? 'new' : editor.id} node={editor} />
      </fieldset>
    </FormDialog>}</DialogPresence>
    <DialogPresence>{deleting && <Confirm title={`删除「${deleting.name}」？`} busy={action.busy} confirmDisabled={Boolean(deletingError)} error={deletingError || action.error}
      onClose={() => setDeleting(null)} onConfirm={remove}>
      此节点及其授权将从订阅中移除，历史流量会保留。被链路引用的节点不能直接删除。设备应用新配置后，代理入口停止监听。
    </Confirm>}</DialogPresence>
  </div>
}
