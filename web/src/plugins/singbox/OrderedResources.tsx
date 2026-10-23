import { useEffect, useRef, useState } from 'react'
import { Badge, Confirm, Empty, ErrorNotice, Field, Loading, Modal, Refresh } from '../../components'
import { bytes, totalBytes } from '../../format'
import { useAction, useResource } from '../../hooks'
import type { ResourceState } from '../../hooks'
import type { Node, PluginServer, Usage } from '../../types'
import { protocolNames } from './ProtocolFields'
import Chains, { deleteProxyResource, proxyDeleteError, proxyWriteError, validNodeList, validServerList } from './Chains'
import type { ProxyWriteSnapshot } from './Chains'
import { dateText, filterProxyResources, proxyResourceKey, validatedSnapshot, validProxyResource, validProxyResources } from './groupTypes'
import type { ProxyResource, ProxyResourceServerRole, ResourceEndpoint } from './groupTypes'
import { nodeHash } from './nodeRoute'
import NodeDeployment from './NodeDeployment'
import { installationView } from './Settings'
import ChainLifecycle, { publicPathText } from './ChainLifecycle'
import ChainResourceEditor from './ChainResourceEditor'
import ChainVersionEditor from './ChainVersionEditor'
import { Button, buttonVariants } from '@/components/ui/button'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { cn } from '@/lib/utils'
import { WaypointsIcon } from 'lucide-react'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'

const root = '/api/plugins/sing-box'
function address(endpoint: ResourceEndpoint) {
  const host = endpoint.public_host.includes(':') ? `[${endpoint.public_host.replace(/^\[|\]$/g, '')}]` : endpoint.public_host
  return `${host}:${endpoint.public_port}`
}
function applicationView(endpoint: ResourceEndpoint, server?: PluginServer, fresh = false) {
  if (!fresh || !server?.installation) return { label: '应用状态待确认', tone: 'neutral' as const, reason: '尚未取得最新设备应用状态，请查看服务器详情。' }
  const view = installationView(server)
  if (server.installation.state === 'ready') {
    if (!endpoint.server_deleted && endpoint.plugin_enabled && endpoint.online && endpoint.desired_revision !== null && endpoint.desired_revision > 0
      && endpoint.applied_revision === endpoint.desired_revision && endpoint.applied_observed_at !== null && server.enabled && server.online
      && server.installation.target_rev === endpoint.desired_revision && server.installation.applied_rev === endpoint.applied_revision) return { ...view, label: '目标配置已应用' }
    return { label: '应用状态待确认', tone: 'warm' as const, reason: '设备状态与目标版本尚未确认一致，请查看服务器详情。' }
  }
  return { ...view, label: server.installation.state === 'queued' ? '等待生成配置' : view.label }
}
function EndpointView({ endpoint, server, node, fresh }: { endpoint: ResourceEndpoint; server?: PluginServer; node?: Node; fresh: boolean }) {
  const application = applicationView(endpoint, server, fresh)
  return <div className="grid min-w-44 justify-items-start gap-1.5"><strong className="max-w-72 wrap-anywhere">{endpoint.name}</strong><a className={cn(buttonVariants({ variant: 'ghost', size: 'sm' }), 'max-w-72 wrap-anywhere')} href={`#/servers/${endpoint.server_id}`}>{fresh ? endpoint.server_name : `服务器 #${endpoint.server_id}`}</a><code className="max-w-72 wrap-anywhere">{address(endpoint)}</code><small className="max-w-72 wrap-anywhere">{protocolNames[endpoint.protocol] ?? endpoint.protocol} · {endpoint.sni || '无需证书'}</small>
    {node && <small className="text-muted-foreground mt-1.5 block max-w-72 wrap-anywhere">监听 {node.settings?.listen ?? '::'} / {endpoint.port}</small>}
    {!endpoint.enabled && <Badge tone="warm">已设为停用</Badge>}
    <small className="max-w-72 wrap-anywhere"><Badge tone={fresh && endpoint.online ? 'good' : 'neutral'}>{fresh ? endpoint.online ? '设备在线' : '设备离线' : '设备状态待确认'}</Badge></small>
    <small className="max-w-72 wrap-anywhere"><Badge tone={application.tone}>{application.label}</Badge></small>
    {fresh && endpoint.desired_revision !== null && endpoint.applied_revision !== null && <small className="max-w-72 wrap-anywhere">目标版本 {endpoint.desired_revision} · 已应用版本 {endpoint.applied_revision}</small>}
    <small className="max-w-72 wrap-anywhere">{application.reason}</small>
    {endpoint.node_deleted && <small className="max-w-72 wrap-anywhere">节点已删除，保留链路记录供清理。</small>}{endpoint.server_deleted && <small className="max-w-72 wrap-anywhere">服务器已删除，保留链路记录供清理。</small>}
  </div>
}
function ResourceDetail({ selected, snapshot, onClose }: { selected: ProxyResource; snapshot: ProxyWriteSnapshot; onClose: () => void }) {
  const query = useResource<unknown>(`${root}/ordered-proxy-resources/${selected.kind}/${selected.id}`)
  const history = useRef(selected)
  const current = validatedSnapshot(query, validProxyResource, history.current)
  const sameIdentity = current.data && proxyResourceKey(current.data) === proxyResourceKey(selected)
  if (sameIdentity && current.fresh) history.current = current.data!
  const resource = sameIdentity ? current.data! : history.current
  const error = current.error || (!sameIdentity ? '资源详情与所选记录不一致，请刷新确认。' : '')
  const fresh = current.fresh && sameIdentity === true && !proxyDeleteError(snapshot.resources, selected)
  const deviceFresh = fresh && snapshot.servers.fresh && !snapshot.servers.error
  return <Modal wide title={`资源详情：${resource.name}`} onClose={onClose}><div data-slot="modal-body" className="p-4"><ErrorNotice message={error} retry={query.reload} />
    {!fresh && <p className="text-muted-foreground mt-3 text-xs leading-relaxed" role="status">当前显示已取得的历史信息，资源与设备状态等待最新确认。</p>}
    <dl className="my-6 grid gap-4 sm:grid-cols-2 md:grid-cols-3"><div><dt className="text-muted-foreground text-xs">资源类型</dt><dd className="m-0 mt-2 text-sm wrap-anywhere">{resource.kind === 'direct' ? '直连节点' : resource.path_kind === 'legacy' ? '受管两跳链路' : '有序混合链路'}</dd></div><div><dt className="text-muted-foreground text-xs">资源标识</dt><dd className="m-0 mt-2 text-sm wrap-anywhere"><code>{proxyResourceKey(resource)}</code> · 设置 {resource.settings_revision}</dd></div><div><dt className="text-muted-foreground text-xs">授权范围</dt><dd className="m-0 mt-2 text-sm wrap-anywhere">{resource.policy_group_ids.length} 个策略组 · {resource.user_count} 位代理用户</dd></div></dl>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">授权人数表示当前授权关系，不代表套餐仍然有效。配置可用、设备在线和目标配置应用分别记录，尚未验证公网可达或链路连通。</p>
    <div className="grid items-start gap-5 py-4 md:grid-cols-[repeat(auto-fit,minmax(min(220px,100%),1fr))]"><section className="min-w-0"><h3 className="mt-3 mb-4 text-sm font-semibold">{resource.kind === 'chain' ? '受管公开入口' : '直连监听'}</h3><EndpointView endpoint={resource.entry} server={snapshot.servers.data?.find(server => server.id === resource.entry.server_id)} node={snapshot.nodes.fresh ? snapshot.nodes.data?.find(node => node.id === resource.entry.id) : undefined} fresh={deviceFresh} /></section>{resource.hops.map(hop => <section className="min-w-0" key={hop.position}><h3 className="mt-3 mb-4 text-sm font-semibold">→ 第 {hop.position} 跳 · {hop.position === resource.hops.length ? '最终出口' : '中间段'}</h3>{hop.kind === 'managed' ? <EndpointView endpoint={hop.endpoint} server={snapshot.servers.data?.find(server => server.id === hop.endpoint.server_id)} node={snapshot.nodes.fresh ? snapshot.nodes.data?.find(node => node.id === hop.node_id) : undefined} fresh={deviceFresh} /> : <div className="grid justify-items-start gap-1.5 min-w-0"><strong className="max-w-72 wrap-anywhere">{hop.name}</strong><small className="max-w-72 wrap-anywhere">{hop.source_name} · 来源 #{hop.source_id} / 代次 {hop.identity_epoch}</small><code className="max-w-72 wrap-anywhere">{hop.server ?? '未知端点'}:{hop.server_port ?? '未知端口'}</code><small className="max-w-72 wrap-anywhere">{hop.protocol ?? '未知协议'} · {hop.transport ?? '默认传输'} · SNI {hop.sni ?? '—'}</small><Badge tone="neutral">订阅节点，无 Agent 状态</Badge><small className="max-w-72 wrap-anywhere">{hop.update_mode === 'pinned' ? '固定版本' : '跟随所选节点'} · 目标版本 {hop.node_version_id}</small><small className="max-w-72 wrap-anywhere">{hop.source_archived ? '来源已归档，保留已有快照' : !hop.node_present ? '当前来源缺失此节点，保留已应用快照' : '当前来源包含此节点'}</small>{hop.update_error && <small className="max-w-72 wrap-anywhere">{hop.update_error}</small>}</div>}</section>)}</div>
    {resource.kind === 'chain' && <><p className="text-muted-foreground mt-3 text-xs leading-relaxed">以上拓扑为目标代输入，不能据此推断当前运行版本。下面单独列出已应用、候选与恢复代。</p><ChainLifecycle resource={resource} fresh={fresh} /></>}
    <p><Badge tone={!fresh ? 'neutral' : resource.available ? 'good' : 'bad'}>{!fresh ? '资源状态待确认' : resource.available ? '资源存在' : '资源已不可用'}</Badge></p>
    {!!resource.unavailable_reasons.length && <ul className="list-disc pl-5 leading-relaxed">{resource.unavailable_reasons.map((reason, index) => <li key={index}>{reason}</li>)}</ul>}
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">策略组：{resource.policy_group_ids.map(id => `#${id}`).join('、') || '尚未加入'}。{resource.chain_refs.length ? `被 ${resource.chain_refs.map(ref => `「${ref.name}」#${ref.id} · ${ref.hop_position === null ? '入口' : `第 ${ref.hop_position} 跳`} · 代 ${ref.generation} / ${{ desired: '目标', applied: '已应用', candidate: '候选', recovery: '恢复', retained: '历史依赖待清理', unresolved: '版本损坏，依赖待确认' }[ref.state]}`).join('、')} 引用。` : '没有其他链路引用。'}</p>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">入口最后应用观测：{dateText(resource.entry.applied_observed_at)}{resource.exit && `；出口：${dateText(resource.exit.applied_observed_at)}`}。</p>
  </div><footer><Refresh onClick={query.reload} /><Button variant="outline" onClick={onClose}>关闭</Button></footer></Modal>
}

type Props = {
  resourcesQuery: ResourceState<unknown>
  nodesQuery: ResourceState<Node[]>
  serversQuery: ResourceState<PluginServer[]>
  usage: ResourceState<Usage>
  flatKeys: string[]
  hideFlatRows?: boolean
  onOrganize?: (resource: ProxyResource) => void
  selected?: ProxyResource
  onCloseSelected: () => void
  filter: string
  initialServerRole?: ProxyResourceServerRole
  getServerId: () => number | undefined
  onEdit: (node: Node) => void
  onChanged: () => void
  onAddSource: () => void
  /** Each increment opens the ordered creation form (the page header's “创建链路”). */
  createRequest?: number
}

// The chain section of the node page; direct nodes are managed in the catalog.
const kind = 'chains'

export default function OrderedResources({ resourcesQuery, nodesQuery, serversQuery, usage, flatKeys, hideFlatRows = false, onOrganize, selected, onCloseSelected, filter, initialServerRole = 'any', getServerId, onEdit, onChanged, onAddSource, createRequest }: Props) {
  const history = useRef<{ resources?: ProxyResource[]; nodes?: Node[]; servers?: PluginServer[] }>({})
  const resources = validatedSnapshot(resourcesQuery, validProxyResources, history.current.resources)
  const nodes = validatedSnapshot(nodesQuery, validNodeList, history.current.nodes)
  const servers = validatedSnapshot(serversQuery, validServerList, history.current.servers)
  if (resources.fresh) history.current.resources = resources.data
  if (nodes.fresh) history.current.nodes = nodes.data
  if (servers.fresh) history.current.servers = servers.data
  const snapshot: ProxyWriteSnapshot = { resources, nodes, servers }
  const action = useAction()
  const [deleting, setDeleting] = useState<ProxyResource | null>(null)
  const [detail, setDetail] = useState<ProxyResource | null>(null)
  const [chainEditor, setChainEditor] = useState<ProxyResource | null>(null)
  const [versionEditor, setVersionEditor] = useState<ProxyResource | null>(null)
  const [chainEditorOpen, setChainEditorOpen] = useState(false), [versionEditorOpen, setVersionEditorOpen] = useState(false)
  const [editPending, setEditPending] = useState(false), [versionPending, setVersionPending] = useState(false)
  const [replacement, setReplacement] = useState<{ generation: number; resource: ProxyResource } | undefined>(undefined)
  const [deployment, setDeployment] = useState<number | null>(null)
  const [batchSaved, setBatchSaved] = useState(0)
  const [serverRole, setServerRole] = useState(initialServerRole)
  useEffect(() => { setServerRole(initialServerRole) }, [initialServerRole])
  const all = resources.data ?? []
  const visible = filterProxyResources(all, kind, filter ? Number(filter) : undefined, serverRole).filter(resource => !hideFlatRows || resource.kind !== 'direct' || !flatKeys.includes(proxyResourceKey(resource)))
  const enabledServers = servers.data?.filter(server => server.enabled) ?? []
  const filterServer = servers.data?.find(server => server.id === Number(filter))
  const writeError = proxyWriteError(snapshot)
  const deleteError = deleting ? proxyDeleteError(resources, deleting) : ''
  const refresh = onChanged
  const remove = (resource: ProxyResource) => { if (action.busy || proxyDeleteError(resources, resource)) return; action.clearError(); setDeleting(resource) }
  const editChain = (resource: ProxyResource) => { if (proxyDeleteError(resources, resource) || editPending && chainEditor?.id !== resource.id) return; setChainEditor(current => current?.id === resource.id ? current : resource); setChainEditorOpen(true) }
  const editVersions = (resource: ProxyResource) => { if (proxyDeleteError(resources, resource) || versionPending && versionEditor?.id !== resource.id) return; setVersionEditor(current => current?.id === resource.id ? current : resource); setVersionEditorOpen(true) }
  return <section aria-label="有序链路与资源引用">
    <div className="flex flex-wrap items-center justify-end gap-2"><Chains snapshot={snapshot} serverId={filter ? Number(filter) : undefined} getServerId={getServerId} refresh={refresh} onCreated={result => setBatchSaved(result.chain_ids.length)} replacement={replacement} onAddSource={onAddSource} openRequest={createRequest} /></div>
    <ErrorNotice message={resources.error} retry={refresh} />
    {(editPending || versionPending) && <Notice role="status">有未确认的链路操作，原草稿与精确请求保留在当前页面内存。{editPending && <Button variant="ghost" size="sm" onClick={() => setChainEditorOpen(true)}>继续确认公开信息修改</Button>}{versionPending && <Button variant="ghost" size="sm" onClick={() => setVersionEditorOpen(true)}>继续确认节点版本更新</Button>}</Notice>}
    {filter && <Field label="链路中的服务器角色" hint="直连按所在服务器匹配；链路按完整有序受管段匹配，订阅段无需 Agent。"><NativeSelect className="w-full" value={serverRole} onChange={event => { const value = event.target.value as ProxyResourceServerRole; setServerRole(value); window.location.hash = nodeHash({ server: filter, role: value, view: 'chains' }).slice(1) }}><option value="any">任一段</option><option value="entry">作为入口</option><option value="middle">作为中间段</option><option value="exit">作为最终出口</option></NativeSelect></Field>}
    {batchSaved > 0 && <Notice role="status"><span>{batchSaved} 条链路已原子创建，尚未授权给代理用户；受管依赖与入口仍需应用。</span><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href="#/plugins/sing-box/groups">管理策略组与套餐</a></Notice>}
    {all.some(resource => resource.chain_refs.length && flatKeys.includes(proxyResourceKey(resource))) && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">资源引用：{all.filter(resource => resource.chain_refs.length && flatKeys.includes(proxyResourceKey(resource))).map(resource => <Button variant="ghost" size="sm" key={proxyResourceKey(resource)} onClick={() => setDetail(resource)}>{resource.name} · {resource.chain_refs.length} 个引用</Button>)}</p>}
    <Panel><PanelHeader><PanelTitle>有序链路与资源引用 <span className="bg-muted text-muted-foreground min-w-5 rounded px-1.5 text-center text-xs font-medium tabular-nums">{visible.length}</span></PanelTitle></PanelHeader>
      <div className="p-4">
        <p className="text-muted-foreground mt-3 text-xs leading-relaxed">
          {filter
            ? `筛选范围：${{ any: '任一受管段', entry: '入口', middle: '中间受管段', exit: '最终受管出口' }[serverRole]}属于${filterServer ? `「${filterServer.name}」` : `服务器 #${filter}`}的链路。`
            : '筛选范围：全部服务器的链路。'}
          {filter && <a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={nodeHash({ kind })}>查看全部链路</a>}
        </p>
        {writeError && <p className="text-muted-foreground mt-3 text-xs leading-relaxed" role="status">{writeError} 已取得的列表保留供查看，资源状态等待确认。</p>}
      </div>
      {resourcesQuery.loading && !resources.data ? <Loading /> : !visible.length ? <Empty icon={WaypointsIcon} title={filter ? '此服务器暂无已确认关联的链路' : '尚未创建链路'} description={enabledServers.length ? '填写端口或使用自动分配。为代理用户授权后，等待设备成功应用配置，再连接节点。' : '先在系统的插件设置中为服务器启用 sing-box，再创建代理节点。'}>{!enabledServers.length && <a className={buttonVariants()} href="#/system/plugins">插件设置</a>}</Empty> : <div className="max-w-full min-w-0"><Table data-slot="proxy-resource-table" className="min-w-[1080px]"><TableHeader><TableRow><TableHead>资源</TableHead><TableHead>入口或直连监听</TableHead><TableHead>出口</TableHead><TableHead>资源与授权</TableHead><TableHead>累计流量</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{visible.map(resource => {
        const node = nodes.data?.find(node => node.id === resource.entry.id)
        const record = usage.data?.by_node.find(record => record.node_id === resource.entry.id)
        return <TableRow key={proxyResourceKey(resource)} data-resource-key={proxyResourceKey(resource)}><TableCell><strong>{resource.name}</strong><small className="text-muted-foreground mt-1.5 block">{resource.kind === 'direct' ? '直连节点' : resource.path_kind === 'legacy' ? '受管两跳链路' : '有序混合链路'}</small><small className="text-muted-foreground mt-1.5 block"><code>{proxyResourceKey(resource)}</code></small>{filter && resource.kind === 'chain' && resource.entry.server_id !== Number(filter) && resource.exit?.server_id === Number(filter) && <small className="text-muted-foreground mt-1.5 block">本服务器作为出口</small>}{resource.chain_refs.length > 0 && <small className="text-muted-foreground mt-1.5 block">共享端点 · {new Set(resource.chain_refs.map(ref => ref.id)).size} 条链路引用</small>}</TableCell>
          <TableCell><EndpointView endpoint={resource.entry} node={nodes.fresh ? node : undefined} server={servers.data?.find(server => server.id === resource.entry.server_id)} fresh={resources.fresh && servers.fresh} /></TableCell><TableCell>{resource.path_kind === 'ordered' ? <div className="max-w-[340px] text-sm leading-relaxed wrap-anywhere"><p>{publicPathText(resource.hops)}</p><small className="text-muted-foreground mt-1.5 block">{resource.hops.length} 个代理跳 · 已应用代 {resource.path_state?.applied_generation ?? '—'} / 目标代 {resource.path_state?.desired_generation ?? '—'}</small><small className="text-muted-foreground mt-1.5 block">状态与指定路径验证见详情</small></div> : resource.exit ? <EndpointView endpoint={resource.exit} node={nodes.fresh ? nodes.data?.find(node => node.id === resource.exit!.id) : undefined} server={servers.data?.find(server => server.id === resource.exit!.server_id)} fresh={resources.fresh && servers.fresh} /> : '—'}</TableCell>
          <TableCell><Badge tone={!resources.fresh ? 'neutral' : resource.available ? 'good' : 'bad'}>{!resources.fresh ? '资源状态待确认' : resource.available ? '资源存在' : '资源已不可用'}</Badge><small className="text-muted-foreground mt-1.5 block">{resource.policy_group_ids.length} 个策略组 · {resource.user_count} 位代理用户</small>{resource.unavailable_reasons.map((reason, index) => <small className="text-muted-foreground mt-1.5 block" key={index}>{reason}</small>)}</TableCell><TableCell>{record ? bytes(totalBytes(record.uplink, record.downlink)) : usage.data ? '0 B' : '暂无数据'}{resource.kind === 'chain' && <small className="text-muted-foreground mt-1.5 block">按入口计量，不重复累计出口</small>}</TableCell>
          <TableCell><div className="flex flex-wrap items-center justify-end gap-2"><Button variant="ghost" size="sm" onClick={() => { if (resource.path_kind === 'ordered') setDetail(resource); else window.location.hash = `/plugins/sing-box/nodes/${resource.kind}/${resource.id}` }}>详情</Button><Button variant="ghost" size="sm" onClick={() => setDetail(resource)}>路径与引用</Button>{resource.path_kind === 'legacy' && onOrganize && <Button variant="ghost" size="sm" disabled={Boolean(proxyDeleteError(resources, resource))} onClick={() => { if (!proxyDeleteError(resources, resource)) onOrganize(resource) }}>整理</Button>}<Button variant="ghost" size="sm" onClick={() => setDeployment(resource.entry.server_id)}>部署</Button>{resource.kind === 'direct' ? <Button variant="ghost" size="sm" disabled={action.busy || Boolean(writeError) || !node} onClick={() => { if (node) onEdit(node) }}>编辑</Button> : <><Button variant="ghost" size="sm" disabled={Boolean(proxyDeleteError(resources, resource)) || editPending && chainEditor?.id !== resource.id} onClick={() => editChain(resource)}>编辑公开信息</Button>{resource.hops.some(hop => hop.kind === 'subscription') && <Button variant="ghost" size="sm" disabled={Boolean(proxyDeleteError(resources, resource)) || versionPending && versionEditor?.id !== resource.id} onClick={() => editVersions(resource)}>应用节点新版本</Button>}<Button variant="ghost" size="sm" disabled={Boolean(writeError)} onClick={() => setReplacement(current => ({ generation: (current?.generation ?? 0) + 1, resource }))}>创建替代链路</Button></>}<Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={action.busy || Boolean(proxyDeleteError(resources, resource))} onClick={() => remove(resource)}>删除</Button></div></TableCell></TableRow>
      })}</TableBody></Table></div>}
    </Panel>
    {deployment !== null && <NodeDeployment serverId={deployment} server={servers.fresh ? servers.data?.find(server => server.id === deployment) : undefined} onClose={() => setDeployment(null)} />}
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">普通节点需为代理用户授权并等待设备成功应用配置；出口可使用内部连接凭据监听，无需为出口单独授权用户。两端状态仅表示设备应用与健康信息，尚未验证公网可达或链路连通。</p>
    {(selected || detail) && <ResourceDetail key={proxyResourceKey((selected || detail)!)} selected={(selected || detail)!} snapshot={snapshot} onClose={() => { setDetail(null); if (selected) onCloseSelected() }} />}
    {chainEditor && <ChainResourceEditor key={chainEditor.id} resource={chainEditor} snapshot={resources} open={chainEditorOpen} onClose={() => { setChainEditorOpen(false); if (!editPending) setChainEditor(null) }} onSaved={() => { setChainEditorOpen(false); setChainEditor(null) }} onPending={setEditPending} refresh={refresh} />}
    {versionEditor && <ChainVersionEditor key={versionEditor.id} resource={versionEditor} snapshot={resources} open={versionEditorOpen} onClose={() => { setVersionEditorOpen(false); if (!versionPending) setVersionEditor(null) }} onSaved={() => { setVersionEditorOpen(false); setVersionEditor(null) }} onPending={setVersionPending} refresh={refresh} />}
    {deleting && <Confirm title={`删除「${deleting.name}」？`} busy={action.busy} disabled={Boolean(deleteError)} error={deleteError || action.error} retry={deleteError ? refresh : undefined} onClose={() => setDeleting(null)} onConfirm={() => { if (action.busy || proxyDeleteError(resources, deleting)) return; void action.run(() => deleteProxyResource(deleting, snapshot), () => { setDeleting(null); refresh() }) }}>存在策略组或当前、候选、恢复路径引用时，面板会拒绝删除并列出引用；请先解除对应关系，再重试。链路删除会退役专用入口及内部身份，共享受管节点、来源、其他链路与历史流量保留。设备应用新配置后才完成监听与内部连接撤销。</Confirm>}
  </section>
}
