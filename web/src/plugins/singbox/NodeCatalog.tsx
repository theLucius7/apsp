import { useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { api } from '../../api'
import { Badge, Confirm, Empty, ErrorNotice, Field, FormDialog, Loading, Modal, RefreshNotice, DialogPresence } from '../../components'
import { bytes, totalBytes } from '../../format'
import { resourceWriteError, useAction, useRefreshNotice } from '../../hooks'
import type { ResourceState } from '../../hooks'
import type { Node, PluginServer, Usage } from '../../types'
import type { ProxyResource, ResourceKey } from './resourceTypes'
import { endpoint, resourceLink, roleNames, stageName } from './resourceTypes'
import { protocolNames } from './ProtocolFields'
import { sourceReason } from './sourceTypes'
import { catalogKey, catalogRef, catalogMutationError, filterCatalog, parsedTags, renamed, tagsError, validCatalog } from './catalog'
import type { CatalogNode } from './catalog'
import { validatedSnapshot } from './groupTypes'

import { Button, buttonVariants } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { ArrowDownIcon, ArrowUpIcon, SlidersHorizontalIcon, WaypointsIcon } from 'lucide-react'
import { ActionMenu } from '@/components/action-menu'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { SortableList, sortableItemClass } from '@/components/sortable'
import type { SortableItemProps } from '@/components/sortable'

const root = '/api/plugins/sing-box'
const kinds = { direct: '直连节点', chain: '链路', external: '外部节点' }
type Props = { query: ResourceState<unknown>; nodes: Node[]; servers: PluginServer[]; getServers: () => PluginServer[] | undefined; usage?: Usage; server: string; getServer: () => string; onServer: (id: string) => void; excludedKeys: string[]; metadataTarget: ResourceKey | null; onMetadataOpened: () => void; kind: string; onKind: (kind: string) => void; initialServerRole?: 'any' | 'entry' | 'middle' | 'exit'; onChanged: () => void; managedWriteError: () => string; onEdit: (node: Node) => void; onDelete: (node: ProxyResource) => void; onDeployment: (id: number) => void }

// The catalog reads its data from the node page's shared snapshot, so it refreshes
// together with the chain and resource lists it is filtered against.
export default function NodeCatalog(props: Props) {
  const query = props.query
  const history = useRef<CatalogNode[] | undefined>(undefined)
  const view = validatedSnapshot(query, validCatalog, history.current)
  if (view.fresh) history.current = view.data
  const catalog = { ...query, ...view, data: view.data, getCurrent: () => view.getCurrent?.(), isCurrent: () => view.isCurrent?.() === true }
  const [filter, setFilter] = useState({ search: '', kind: props.kind, protocol: '', tag: '', status: '', source: '', role: '' })
  const filterRef = useRef(filter)
  filterRef.current = filter
  useEffect(() => { filterRef.current = { ...filterRef.current, kind: props.kind }; setFilter(filterRef.current); setPage(1) }, [props.kind])
  const [page, setPage] = useState(1), [size, setSize] = useState(20), [sort, setSort] = useState('custom')
  // Search, type, server and status stay in view; the rest sit behind a toggle that counts what it hides.
  const [moreOpen, setMoreOpen] = useState(false)
  const hiddenFilters = [filter.protocol, filter.tag, filter.source, filter.role].filter(Boolean).length + (sort === 'custom' ? 0 : 1)
  const [selection, setSelection] = useState<string[]>([])
  const [batch, setBatch] = useState<{ mode: string; nodes: CatalogNode[]; scope: string } | null>(null)
  const [details, setDetails] = useState<CatalogNode | null>(null)
  const [clone, setClone] = useState<{ node: CatalogNode; scope: string } | null>(null)
  const action = useAction()
  const all = catalog.data ?? []
  const visible = filterCatalog(all.filter(node => !props.excludedKeys.includes(catalogKey(node))), { ...filter, server: props.server, serverRole: props.initialServerRole }).sort((a, b) => sort === 'name' ? a.name.localeCompare(b.name, 'zh-CN') || catalogKey(a).localeCompare(catalogKey(b)) : sort === 'protocol' ? (a.protocol ?? '').localeCompare(b.protocol ?? '') || a.name.localeCompare(b.name) : a.sort_order - b.sort_order || a.kind.localeCompare(b.kind) || a.id - b.id)
  const pages = Math.max(1, Math.ceil(visible.length / size)), currentPage = Math.min(page, pages)
  const rows = visible.slice((currentPage - 1) * size, currentPage * size)
  const selected = all.filter(node => selection.includes(catalogKey(node)))
  const scope = () => JSON.stringify({ ...filterRef.current, server: props.getServer(), serverRole: props.initialServerRole ?? 'any' })
  const stale = () => resourceWriteError(catalog) || props.managedWriteError()
  const mutationError = (nodes: CatalogNode[]) => stale() || catalogMutationError(catalog.getCurrent?.(), nodes, props.getServer(), props.initialServerRole)
  // Row and batch actions clicked before a refresh visibly disables them are reported.
  const refreshNotice = useRefreshNotice()
  const unavailable = Boolean(stale())
  const refresh = props.onChanged
  const change = (key: keyof typeof filter, value: string) => { filterRef.current = { ...filterRef.current, [key]: value }; setFilter(filterRef.current); if (key === 'kind') props.onKind(value); setPage(1) }
  const toggle = (node: CatalogNode) => setSelection(value => value.includes(catalogKey(node)) ? value.filter(key => key !== catalogKey(node)) : value.length < 200 ? [...value, catalogKey(node)] : value)
  const openBatch = (mode: string, nodes = selected) => { if (!refreshNotice.allows(mutationError(nodes))) return; action.clearError(); setBatch({ mode, nodes, scope: scope() }) }
  const openClone = (node: CatalogNode) => { if (refreshNotice.allows(mutationError([node]))) setClone({ node, scope: scope() }) }
  const remove = (node: CatalogNode) => { if (!refreshNotice.allows(mutationError([node]))) return; if (node.kind === 'external') openBatch('delete', [node]); else props.onDelete(node as ProxyResource) }
  useEffect(() => { if (!props.metadataTarget) return; const node = (catalog.getCurrent?.() ?? catalog.data)?.find(node => catalogKey(node) === catalogKey(props.metadataTarget!)); if (node) openBatch('metadata', [node]); props.onMetadataOpened() }, [props.metadataTarget])
  const apply = (items: object[]) => api(`${root}/node-catalog/batch`, 'POST', { items })
  const move = (node: CatalogNode, offset: number) => {
    if (mutationError([node])) return
    const current = catalog.getCurrent?.(); if (!current) return
    const ordered = [...current].sort((a, b) => a.sort_order - b.sort_order || a.kind.localeCompare(b.kind) || a.id - b.id)
    const currentVisible = filterCatalog(current, { ...filterRef.current, server: props.getServer(), serverRole: props.initialServerRole }).sort((a, b) => a.sort_order - b.sort_order || a.kind.localeCompare(b.kind) || a.id - b.id)
    const neighbor = currentVisible[currentVisible.findIndex(value => catalogKey(value) === catalogKey(node)) + offset]
    if (!neighbor || mutationError([node, neighbor])) return
    const index = ordered.findIndex(value => catalogKey(value) === catalogKey(node)), other = ordered.findIndex(value => catalogKey(value) === catalogKey(neighbor))
    // Normalize a small catalog to distinct ranks so tied defaults cannot jump other rows.
    if (ordered.length > 200) { openBatch('order', [node]); return }
    ;[ordered[index], ordered[other]] = [ordered[other], ordered[index]]
    void action.run(() => apply(ordered.map((value, position) => ({ ...catalogRef(value), sort_order: position }))), refresh)
  }
  const reorder = (node: CatalogNode, target: CatalogNode) => {
    if (mutationError([node, target])) return
    const current = catalog.getCurrent?.(); if (!current) return
    const ordered = [...current].sort((a, b) => a.sort_order - b.sort_order || a.kind.localeCompare(b.kind) || a.id - b.id)
    if (ordered.length > 200) { openBatch('order', [node]); return }
    const index = ordered.findIndex(value => catalogKey(value) === catalogKey(node)), other = ordered.findIndex(value => catalogKey(value) === catalogKey(target))
    if (index < 0 || other < 0) return
    const [moved] = ordered.splice(index, 1)
    ordered.splice(other, 0, moved)
    void action.run(() => apply(ordered.map((value, position) => ({ ...catalogRef(value), sort_order: position }))), refresh)
  }
  const row = (node: CatalogNode, itemProps?: SortableItemProps, handle?: ReactNode) => <TableRow key={catalogKey(node)} {...itemProps} className={itemProps && sortableItemClass} data-resource-key={catalogKey(node)}><TableCell className="max-w-[310px] wrap-anywhere"><span className="flex items-center gap-1">{handle}<Checkbox aria-label={`选择 ${node.name}`} checked={selection.includes(catalogKey(node))} onCheckedChange={() => toggle(node)} /></span></TableCell><TableCell className="max-w-[310px] whitespace-normal wrap-anywhere">{title(node)}</TableCell><TableCell className="max-w-[310px] whitespace-normal wrap-anywhere">{node.kind === 'external' ? node.source_name : <a className="text-primary underline-offset-4 hover:underline" href={`#/servers/${node.server_id}`}>{node.server_name}</a>}</TableCell><TableCell className="max-w-[310px] whitespace-normal wrap-anywhere"><code className="block min-w-36 whitespace-normal">{publicEndpoint(node)}</code><small className="text-muted-foreground mt-1 block">{node.udp ? 'TCP / UDP' : '仅 TCP'}</small></TableCell><TableCell className="max-w-[310px] whitespace-normal wrap-anywhere">{status(node)}</TableCell><TableCell className="max-w-[310px] whitespace-normal wrap-anywhere">{traffic(node)}</TableCell><TableCell>{actions(node)}</TableCell></TableRow>
  const actions = (node: CatalogNode) => {
    const managed = node.kind !== 'external' ? props.nodes.find(value => value.id === (node.entry_node_id ?? node.id)) : undefined
    // Frequent actions stay visible; deployment, metadata and cloning sit behind “更多操作”.
    return <div className="flex items-center justify-end gap-1 @max-4xl/catalog:flex-wrap">{node.kind === 'external' ? <Button variant="ghost" size="sm" onClick={() => setDetails(node)}>详情</Button> : <a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={resourceLink(node as ProxyResource)}>详情</a>}
      {node.kind === 'direct' && <Button variant="ghost" size="sm" disabled={!managed} onClick={() => managed && props.onEdit(managed)}>编辑</Button>}
      <Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={unavailable} onClick={() => remove(node)}>删除</Button>
      {sort === 'custom' && <><Button variant="ghost" size="icon-sm" aria-label={`上移 ${node.name}`} title="上移" disabled={action.busy || Boolean(stale()) || catalogKey(node) === catalogKey(visible[0])} onClick={() => move(node, -1)}><ArrowUpIcon aria-hidden="true" /></Button><Button variant="ghost" size="icon-sm" aria-label={`下移 ${node.name}`} title="下移" disabled={action.busy || Boolean(stale()) || catalogKey(node) === catalogKey(visible.at(-1)!)} onClick={() => move(node, 1)}><ArrowDownIcon aria-hidden="true" /></Button></>}
      <ActionMenu groups={[[
        node.kind !== 'external' && { label: '部署', onSelect: () => props.onDeployment(node.server_id!) },
        { label: '整理', disabled: unavailable, onSelect: () => openBatch('metadata', [node]) },
        node.kind === 'direct' && { label: '复制', disabled: unavailable, onSelect: () => openClone(node) },
      ]]} />
    </div>
  }
  const publicEndpoint = (node: CatalogNode) => node.public_host && node.port !== null ? endpoint(node.public_host, props.nodes.find(value => node.kind !== 'external' && value.id === (node.entry_node_id ?? node.id))?.settings?.public_port ?? node.port) : '未知端点'
  const title = (node: CatalogNode) => <div className="min-w-32"><strong>{node.name}</strong><small className="text-muted-foreground mt-1 block text-xs">{kinds[node.kind]} · {(node.protocol && protocolNames[node.protocol]) ?? node.protocol ?? '未知协议'}</small><div className="mt-1.5 flex flex-wrap gap-1">{node.tags.map(tag => <button key={tag} onClick={() => { change('tag', tag); setMoreOpen(true) }} className="bg-card hover:border-primary hover:bg-accent hover:text-accent-foreground focus-visible:ring-ring/50 cursor-pointer rounded-md border px-1.5 py-0.5 text-xs transition-colors outline-none focus-visible:ring-3">{tag}</button>)}</div>{node.note && <small className="text-muted-foreground mt-1 block max-w-60 truncate text-xs" title={node.note}>{node.note}</small>}</div>
  const status = (node: CatalogNode) => <><Badge tone={stale() ? 'neutral' : !node.enabled ? 'neutral' : node.available ? 'good' : 'warm'}>{stale() ? '资源状态待确认' : !node.enabled ? '已停用' : node.kind === 'external' ? node.available ? '可分配' : '不可分配' : node.available ? '可用' : '待就绪'}</Badge><small className="text-muted-foreground mt-1 block text-xs">{node.kind === 'external' ? node.last_error ? sourceReason(node.last_error) : '由来源提供服务' : `${roleNames[node.role as ProxyResource['role']]} · ${stageName(node.stage)}`}</small></>
  const traffic = (node: CatalogNode) => { const record = props.usage?.by_node.find(value => value.node_id === (node.entry_node_id ?? node.id)); return node.kind === 'external' ? '提供方计量' : record ? bytes(totalBytes(record.uplink, record.downlink)) : props.usage ? '0 B' : '暂无数据' }
  return <Panel aria-label="节点库">
    <PanelHeader><PanelTitle>节点库 <span className="bg-muted text-muted-foreground min-w-5 rounded px-1.5 text-center text-xs font-medium tabular-nums">{all.length}</span></PanelTitle><span className="text-muted-foreground text-xs">受管 {all.filter(node => node.kind !== 'external').length} · 外部 {all.filter(node => node.kind === 'external').length}</span></PanelHeader>
    <div className="flex flex-wrap gap-2 border-b px-5 py-4 max-md:p-3"><Input className="min-w-40 flex-[1_1_250px] max-md:w-full" aria-label="搜索代理资源" placeholder="搜索名称、地址、标签或备注" value={filter.search} onChange={event => change('search', event.target.value)} />
      <NativeSelect className="w-auto max-w-[200px] max-md:max-w-none max-md:min-w-0 max-md:flex-[1_1_40%]" aria-label="按类型筛选" value={filter.kind} onChange={event => change('kind', event.target.value)}><option value="">全部类型</option>{Object.entries(kinds).map(([value, name]) => <option key={value} value={value}>{name}</option>)}</NativeSelect>
      <NativeSelect className="w-auto max-w-[200px] max-md:max-w-none max-md:min-w-0 max-md:flex-[1_1_40%]" aria-label="按服务器筛选" value={props.server} onChange={event => { props.onServer(event.target.value); setPage(1) }}><option value="">全部服务器</option>{props.server && !props.servers.some(server => String(server.id) === props.server) && <option value={props.server}>指定服务器尚未启用或不存在</option>}{props.servers.map(server => <option key={server.id} value={server.id}>{server.name}</option>)}</NativeSelect>
      <NativeSelect className="w-auto max-w-[200px] max-md:max-w-none max-md:min-w-0 max-md:flex-[1_1_40%]" aria-label="按状态筛选" value={filter.status} onChange={event => change('status', event.target.value)}><option value="">全部状态</option><option value="available">可用 / 可分配</option><option value="unavailable">待就绪 / 不可分配</option><option value="disabled">已停用</option></NativeSelect>
      <Button type="button" variant="outline" className="max-md:flex-[1_1_40%]" aria-expanded={moreOpen} aria-controls="catalog-more-filters" onClick={() => setMoreOpen(open => !open)}><SlidersHorizontalIcon aria-hidden="true" />更多筛选{hiddenFilters > 0 && <span className="bg-primary text-primary-foreground min-w-5 rounded-full px-1.5 text-center text-xs font-medium tabular-nums">{hiddenFilters}</span>}</Button>
    </div>
    <div id="catalog-more-filters" className={moreOpen ? 'flex flex-wrap gap-2 border-b px-5 py-3 max-md:p-3' : 'hidden'}>
      <NativeSelect className="w-auto max-w-[200px] max-md:max-w-none max-md:min-w-0 max-md:flex-[1_1_40%]" aria-label="按协议筛选" value={filter.protocol} onChange={event => change('protocol', event.target.value)}><option value="">全部协议</option>{[...new Set(all.flatMap(node => node.protocol ? [node.protocol] : []))].sort().map(value => <option key={value}>{value}</option>)}</NativeSelect>
      <NativeSelect className="w-auto max-w-[200px] max-md:max-w-none max-md:min-w-0 max-md:flex-[1_1_40%]" aria-label="按标签筛选" value={filter.tag} onChange={event => change('tag', event.target.value)}><option value="">全部标签</option>{[...new Set(all.flatMap(node => node.tags))].sort().map(value => <option key={value}>{value}</option>)}</NativeSelect>
      <NativeSelect className="w-auto max-w-[200px] max-md:max-w-none max-md:min-w-0 max-md:flex-[1_1_40%]" aria-label="按来源筛选" value={filter.source} onChange={event => change('source', event.target.value)}><option value="">全部来源</option>{[...new Map(all.filter(node => node.source_id).map(node => [node.source_id, node.source_name])).entries()].map(([id, name]) => <option key={id} value={id}>{name}</option>)}</NativeSelect>
      <NativeSelect className="w-auto max-w-[200px] max-md:max-w-none max-md:min-w-0 max-md:flex-[1_1_40%]" aria-label="按角色筛选" value={filter.role} onChange={event => change('role', event.target.value)}><option value="">全部角色</option>{Object.entries(roleNames).map(([key, value]) => <option key={key} value={key}>{value}</option>)}<option value="external">外部节点</option></NativeSelect>
      <NativeSelect className="w-auto max-w-[200px] max-md:max-w-none max-md:min-w-0 max-md:flex-[1_1_40%]" aria-label="节点排序" value={sort} onChange={event => { setSort(event.target.value); setPage(1) }}><option value="custom">自定义顺序</option><option value="name">名称</option><option value="protocol">协议</option></NativeSelect>
    </div>
    <div className="flex flex-wrap items-center gap-3.5 border-b px-5 py-3 text-sm max-md:gap-2.5 max-md:p-3"><label className="flex items-center gap-1.5"><Checkbox aria-label="选择本页节点" checked={!!rows.length && rows.every(node => selection.includes(catalogKey(node)))} onCheckedChange={checked => setSelection(checked === true ? [...new Set([...selection, ...rows.map(catalogKey)])].slice(0, 200) : selection.filter(key => !rows.some(node => catalogKey(node) === key)))} /> 本页</label><span>已选 {selected.length} 项</span><Button variant="ghost" size="sm" disabled={!visible.length || visible.length > 200} onClick={() => setSelection(visible.map(catalogKey))}>选择筛选结果（{visible.length}）</Button><Button variant="ghost" size="sm" onClick={() => setSelection([])} disabled={!selection.length}>清空</Button>
      {selected.length > 0 && <div className="flex flex-wrap items-center justify-end gap-2">
        <Button variant="ghost" size="sm" disabled={unavailable} onClick={() => openBatch('rename')}>批量改名</Button>
        <Button variant="ghost" size="sm" disabled={unavailable} onClick={() => openBatch('tags')}>批量标签</Button>
        <Button variant="ghost" size="sm" disabled={unavailable} onClick={() => openBatch('enable')}>启用</Button>
        <Button variant="ghost" size="sm" disabled={unavailable} onClick={() => openBatch('disable')}>停用</Button>
        <Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={unavailable} onClick={() => openBatch('delete')}>批量删除</Button>
      </div>}
    </div>
    <ErrorNotice message={catalog.error || (!batch ? action.error : '')} retry={catalog.reload} />
    {refreshNotice.visible && <RefreshNotice />}
    {catalog.loading && !catalog.data ? <Loading variant="table" rows={6} /> : !rows.length ? <Empty icon={WaypointsIcon} title="没有符合条件的节点" description="创建受管节点，或在“订阅来源”中导入外部节点。" /> : <>
      {/* The table needs about 56rem; a narrower panel (phones, tablets, a wide sidebar) shows cards. */}
      <div className="@container/catalog">
      <div data-slot="catalog-table" className="max-w-full min-w-0 @max-4xl/catalog:hidden"><Table><TableHeader><TableRow><TableHead>选择</TableHead><TableHead>节点 / 链路</TableHead><TableHead>服务器 / 来源</TableHead><TableHead>公开地址</TableHead><TableHead>状态</TableHead><TableHead>累计流量</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{sort === 'custom' ? <SortableList items={rows} getId={node => catalogKey(node)} getLabel={node => node.name} disabled={action.busy || Boolean(stale())} onMove={(from, to) => reorder(rows[from], rows[to])}>{(node, _, itemProps, handle) => row(node, itemProps, handle)}</SortableList> : rows.map(node => row(node))}</TableBody></Table></div>
      <div className="@4xl/catalog:hidden">{rows.map(node => <article data-slot="catalog-card" key={catalogKey(node)} className="border-b p-4" data-resource-key={catalogKey(node)}><div className="flex items-start gap-2.5"><Checkbox aria-label={`选择 ${node.name}`} checked={selection.includes(catalogKey(node))} onCheckedChange={() => toggle(node)} />{title(node)}</div><div className="text-muted-foreground my-3 grid gap-1.5 text-sm"><span>{node.server_name ?? node.source_name}</span><code className="whitespace-normal wrap-anywhere">{publicEndpoint(node)}</code>{status(node)}<span>{traffic(node)}</span></div>{actions(node)}</article>)}</div>
      </div>
    </>}
    <div className="flex flex-wrap items-center justify-between gap-3.5 border-t px-5 py-3 text-sm max-md:gap-2.5 max-md:p-3"><span>共 {visible.length} 项 · 第 {currentPage} / {pages} 页</span><div className="flex flex-wrap items-center justify-end gap-2"><NativeSelect className="w-auto" aria-label="每页节点数" value={size} onChange={event => { setSize(Number(event.target.value)); setPage(1) }}>{[20, 50, 100, 200].map(value => <option key={value} value={value}>{value} 项 / 页</option>)}</NativeSelect><Button variant="outline" size="sm" disabled={currentPage === 1} onClick={() => setPage(currentPage - 1)}>上一页</Button><Button variant="outline" size="sm" disabled={currentPage === pages} onClick={() => setPage(currentPage + 1)}>下一页</Button></div></div>
    <DialogPresence>{batch && <CatalogBatch mode={batch.mode} nodes={batch.nodes} current={() => catalog.getCurrent?.()} stale={() => stale() || (scope() !== batch.scope ? '筛选范围已改变；请重新确认，当前草稿已保留。' : '')} server={() => props.getServer()} serverRole={props.initialServerRole} onClose={() => setBatch(null)} onSaved={() => { setBatch(null); setSelection([]); refresh() }} />}</DialogPresence>
    <DialogPresence>{details && <Modal title={details.name} onClose={() => setDetails(null)}><div data-slot="modal-body" className="p-4"><dl className="grid grid-cols-[auto_1fr] gap-3.5"><dt className="text-muted-foreground">来源</dt><dd className="m-0 wrap-anywhere">{details.source_name}</dd><dt className="text-muted-foreground">原始名称</dt><dd className="m-0 wrap-anywhere">{details.original_name}</dd><dt className="text-muted-foreground">节点端点</dt><dd className="m-0 wrap-anywhere">{details.protocol} · {details.public_host && details.port !== null ? endpoint(details.public_host, details.port) : '未知端点'}</dd><dt className="text-muted-foreground">当前版本</dt><dd className="m-0 wrap-anywhere">{details.version_id ? `#${details.version_id}` : '未知版本'} · 来源代次 {details.identity_epoch}</dd><dt className="text-muted-foreground">备注</dt><dd className="m-0 wrap-anywhere">{details.note || '—'}</dd></dl><p className="text-muted-foreground mt-3 text-xs leading-relaxed">从代理用户页面分配此节点。连接凭据仅在授权订阅中提供，运行状态与用量由来源管理。</p><a className={buttonVariants()} href="#/plugins/sing-box/users" onClick={() => setDetails(null)}>分配给代理用户</a></div></Modal>}</DialogPresence>
    <DialogPresence>{clone && <CloneNode node={clone.node} servers={props.servers} getServers={props.getServers} stale={() => mutationError([clone.node]) || (scope() !== clone.scope ? '筛选范围已改变；请重新确认，当前草稿已保留。' : '')} onClose={() => setClone(null)} onSaved={() => { setClone(null); refresh() }} />}</DialogPresence>
  </Panel>
}

function CatalogBatch({ mode, nodes, current, stale, server, serverRole, onClose, onSaved }: { mode: string; nodes: CatalogNode[]; current: () => CatalogNode[] | undefined; stale: () => string; server: () => string; serverRole?: 'any' | 'entry' | 'middle' | 'exit'; onClose: () => void; onSaved: () => void }) {
  const action = useAction()
  const [renameMode, setRenameMode] = useState('prefix'), [value, setValue] = useState(''), [replacement, setReplacement] = useState('')
  const [tagMode, setTagMode] = useState('append'), [tags, setTags] = useState(mode === 'metadata' ? nodes[0].tags.join(', ') : '')
  const [name, setName] = useState(nodes[0].name), [note, setNote] = useState(nodes[0].note), [order, setOrder] = useState(String(nodes[0].sort_order))
  const writeError = () => stale() || catalogMutationError(current(), nodes, server(), serverRole)
  const error = writeError()
  const submit = () => void action.run(async () => {
    const currentError = writeError(); if (currentError) throw new Error(currentError)
    const parsed = parsedTags(tags)
    if (['tags', 'metadata'].includes(mode) && tagsError(parsed)) throw new Error(tagsError(parsed))
    const items = nodes.map(node => {
      const fields: Record<string, unknown> = { ...catalogRef(node) }
      if (mode === 'rename') fields.name = renamed(node.name, renameMode, value, replacement)
      if (mode === 'tags') fields.tags = tagMode === 'replace' ? parsed : tagMode === 'remove' ? node.tags.filter(tag => !parsed.includes(tag)) : [...new Set([...node.tags, ...parsed])]
      if (mode === 'enable' || mode === 'disable') fields.enabled = mode === 'enable'
      if (mode === 'metadata') { if (name.trim() !== nodes[0].name) fields.name = name.trim(); fields.tags = parsed; fields.note = note; fields.sort_order = Number(order) }
      if (mode === 'order') fields.sort_order = Number(order)
      if (fields.name !== undefined && (!fields.name || Array.from(String(fields.name)).length > 128)) throw new Error('节点名称不能为空，且不能超过 128 个字符。')
      if (fields.tags && tagsError(fields.tags as string[])) throw new Error(tagsError(fields.tags as string[]))
      if (fields.sort_order !== undefined && !Number.isSafeInteger(fields.sort_order)) throw new Error('排序值须为安全范围内的整数。')
      return fields
    })
    return api(`${root}/node-catalog/batch`, mode === 'delete' ? 'DELETE' : 'POST', { items })
  }, onSaved)
  const titles: Record<string, string> = { rename: '批量改名', tags: '批量标签', enable: '启用节点', disable: '停用节点', delete: '删除节点', metadata: '整理节点', order: '调整顺序' }
  if (['delete', 'enable', 'disable'].includes(mode)) return <Confirm title={`${titles[mode]}（${nodes.length} 项）`} busy={action.busy} error={error || action.error} confirmDisabled={Boolean(error)} confirmLabel={`确认${titles[mode]}`} busyLabel="正在提交…" onClose={onClose} onConfirm={submit}><span>{nodes.map(node => node.name).join('、')}</span><br />{mode === 'delete' ? '操作会先核对全部引用，任一节点无法删除则整批不变。历史流量与版本保留。' : '受管节点等待设备应用后生效；外部节点只改变后续订阅分发，已下载凭据及现有链路不因此失效。'}</Confirm>
  return <FormDialog title={`${titles[mode]}（${nodes.length} 项）`} wide onClose={onClose} onSubmit={submit} busy={action.busy} error={error || action.error} submitDisabled={Boolean(error)} submitLabel="确认保存">
    {mode === 'rename' && <><Field label="改名方式"><NativeSelect className="w-full" value={renameMode} onChange={event => setRenameMode(event.target.value)}><option value="prefix">添加前缀</option><option value="suffix">添加后缀</option><option value="replace">替换文本</option></NativeSelect></Field><Field label={renameMode === 'replace' ? '查找文本' : '添加文本'}><Input required value={value} onChange={event => setValue(event.target.value)} maxLength={128} /></Field>{renameMode === 'replace' && <Field label="替换为"><Input value={replacement} onChange={event => setReplacement(event.target.value)} maxLength={128} /></Field>}<div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>原名称</TableHead><TableHead>预览</TableHead></TableRow></TableHeader><TableBody>{nodes.map(node => <TableRow key={catalogKey(node)}><TableCell>{node.name}</TableCell><TableCell>{renamed(node.name, renameMode, value, replacement)}</TableCell></TableRow>)}</TableBody></Table></div></>}
    {mode === 'tags' && <Field label="标签操作"><NativeSelect className="w-full" value={tagMode} onChange={event => setTagMode(event.target.value)}><option value="append">追加标签</option><option value="replace">替换全部标签</option><option value="remove">移除指定标签</option></NativeSelect></Field>}
    {mode === 'metadata' && <Field label="显示名称"><Input required maxLength={128} value={name} onChange={event => setName(event.target.value)} /></Field>}
    {['tags', 'metadata'].includes(mode) && <Field label="标签" hint="逗号分隔，最多 16 个。"><Input value={tags} onChange={event => setTags(event.target.value)} /></Field>}
    {mode === 'metadata' && <Field label="备注"><Textarea rows={3} value={note} maxLength={1024} onChange={event => setNote(event.target.value)} /></Field>}
    {['metadata', 'order'].includes(mode) && <Field label="排序值" hint="数值越小越靠前。"><Input type="number" step={1} required value={order} onChange={event => setOrder(event.target.value)} /></Field>}
  </FormDialog>
}

function CloneNode({ node, servers, getServers, stale, onClose, onSaved }: { node: CatalogNode; servers: PluginServer[]; getServers: () => PluginServer[] | undefined; stale: () => string; onClose: () => void; onSaved: () => void }) {
  const action = useAction()
  const [serverId, setServerId] = useState(String(node.server_id ?? ''))
  const selectedServer = useRef(serverId); selectedServer.current = serverId
  const targetError = () => getServers()?.some(server => server.enabled === true && Number.isSafeInteger(server.id) && server.id > 0 && String(server.id) === selectedServer.current) ? '' : '目标服务器已不存在或未启用，请明确重新选择；当前草稿已保留。'
  const writeError = () => stale() || targetError()
  return <FormDialog title={`复制「${node.name}」`} onClose={onClose} busy={action.busy} error={writeError() || action.error} submitDisabled={Boolean(writeError())} submitLabel="创建副本" onSubmit={form => void action.run(() => { if (writeError()) throw new Error(writeError()); return api(`${root}/nodes/${node.id}/clone`, 'POST', { revision: node.revision, name: String(form.get('name')).trim(), server_id: Number(selectedServer.current), public_host: String(form.get('public_host')).trim(), ...(form.get('port') ? { port: Number(form.get('port')) } : {}) }) }, onSaved)}>
    <Field label="副本名称"><Input name="name" required maxLength={128} defaultValue={`${node.name} 副本`} /></Field><Field label="目标服务器"><NativeSelect className="w-full" name="server_id" required value={serverId} onChange={event => { selectedServer.current = event.target.value; setServerId(event.target.value) }}>{serverId && !servers.some(server => server.enabled && String(server.id) === serverId) && <option value={serverId}>已选服务器已不存在或未启用（原选择保留）</option>}{servers.filter(server => server.enabled).map(server => <option key={server.id} value={server.id}>{server.name}</option>)}</NativeSelect></Field><Field label="公开地址"><Input name="public_host" required defaultValue={node.public_host ?? ''} /></Field><Field label="监听端口" hint="留空自动分配。"><Input name="port" type="number" min={1} max={65535} /></Field><p className="text-muted-foreground mt-3 text-xs leading-relaxed">复制协议设置并重新生成连接身份，不复制代理用户授权。使用证书的节点请确认目标服务器和域名的证书设置。</p>
  </FormDialog>
}
