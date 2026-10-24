import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from '../../api'
import { Badge, Confirm, Empty, ErrorNotice, Field, FormDialog, Loading, Refresh } from '../../components'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import { hopReference, sourceReason, sourceStatus, validSubscriptionSources, validExternalNodePreviews, knownSourceAmount, sourceReportedTotal, sourceAdoptionError } from './sourceTypes'
import type { ExternalNodePreview, SubscriptionHopReference, SubscriptionSource, SubscriptionSourceJob } from './sourceTypes'
import SourceImport from './SourceImport'
import { validatedSnapshot } from './groupTypes'
import { bytes } from '../../format'

import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Spinner } from '@/components/ui/spinner'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Textarea } from '@/components/ui/textarea'
import { PlusIcon, WaypointsIcon } from 'lucide-react'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

const root = '/api/plugins/sing-box'
const time = (timestamp: number | null) => timestamp ? new Date(timestamp * 1000).toLocaleString('zh-CN', { hour12: false }) : '尚无记录'

// Numbered sources. After the source migration they are a read-only archive;
// `migrated` is undefined until the panel confirms which state applies.
export default function SubscriptionSources({ onSelect, onChange, migrated }: { onSelect?: (reference: SubscriptionHopReference, node: ExternalNodePreview) => void; onChange?: () => void; migrated?: boolean }) {
  const sourcesQuery = useResource<unknown>(`${root}/subscription-sources`)
  const sourceHistory = useRef<SubscriptionSource[] | undefined>(undefined)
  const sourceView = validatedSnapshot(sourcesQuery, validSubscriptionSources, sourceHistory.current)
  if (sourceView.fresh) sourceHistory.current = sourceView.data
  const sources = { ...sourcesQuery, ...sourceView, data: sourceView.data, getCurrent: () => sourceView.getCurrent?.(), isCurrent: () => sourceView.isCurrent?.() === true }
  const [selectedId, setSelectedId] = useState<number | null>(null)
  const selectedIdRef = useRef(selectedId); selectedIdRef.current = selectedId
  const chooseSource = (id: number) => { selectedIdRef.current = id; setSelectedId(id) }
  const [editor, setEditor] = useState<{ source?: SubscriptionSource } | null>(null)
  const [deleting, setDeleting] = useState<SubscriptionSource | null>(null)
  const [mode, setMode] = useState<SubscriptionHopReference['update_mode']>('follow_node')
  const action = useAction()
  const selected = sources.data?.find(source => source.id === selectedId)
  const nodesQuery = useResource<unknown>(selectedId ? `${root}/subscription-sources/${selectedId}/nodes` : null)
  const nodeHistory = useRef<{ id: number | null; data?: ExternalNodePreview[] }>({ id: selectedId })
  if (nodeHistory.current.id !== selectedId) nodeHistory.current = { id: selectedId }
  const nodeView = validatedSnapshot(nodesQuery, validExternalNodePreviews, nodeHistory.current.data)
  if (nodeView.fresh) nodeHistory.current.data = nodeView.data
  const nodes = { ...nodesQuery, ...nodeView, data: nodeView.data, getCurrent: () => nodeView.getCurrent?.(), isCurrent: () => nodeView.isCurrent?.() === true }
  const job = useResource<SubscriptionSourceJob>(selected?.active_job_id ? `${root}/subscription-source-jobs/${selected.active_job_id}` : null, 1500)
  const previous = useRef<string | null>(null)
  const change = useRef(onChange); change.current = onChange
  useEffect(() => {
    if (!sources.data) return
    const revisions = sources.data.map(source => `${source.id}:${source.current_revision_id}:${source.settings_revision}`).join(',')
    // The first snapshot is the baseline; reloading the whole page for it only repeats the initial reads.
    if (previous.current === null) { previous.current = revisions; return }
    if (revisions !== previous.current) { previous.current = revisions; nodes.reload(); change.current?.() }
  }, [sources.data, nodes.reload])
  const reload = () => { sources.reload(); nodes.reload(); job.reload() }
  const writeError = (source?: SubscriptionSource) => {
    if (migrated === undefined) return '订阅来源迁移状态尚未确认，请刷新后再修改；当前草稿已保留。'
    if (migrated) return '数字编号来源已迁移到订阅来源，此处只读。'
    const stale = resourceWriteError(sources)
    if (stale) return stale
    const current = source && sources.getCurrent?.()?.find(value => value.id === source.id)
    return source && (!current || current.settings_revision !== source.settings_revision || current.identity_epoch !== source.identity_epoch || current.current_revision_id !== source.current_revision_id || current.archived !== source.archived || current.active_job_id !== source.active_job_id) ? '此来源已不存在或设置已改变，请重新确认；当前草稿已保留。' : ''
  }
  const selectionError = () => selectedIdRef.current !== selected?.id ? '来源选择已改变，请重新确认；当前选择已保留。' : writeError(selected) || resourceWriteError(nodes)
  const open = (source?: SubscriptionSource) => { if (writeError(source)) return; action.clearError(); setEditor({ source }) }
  const archive = (source: SubscriptionSource) => { if (writeError(source)) return; void action.run(() => api(`${root}/subscription-sources/${source.id}`, 'PATCH', { settings_revision: source.settings_revision, archived: !source.archived }), reload) }
  const refreshSource = (source: SubscriptionSource) => {
    if (writeError(source)) return
    const current = sources.getCurrent?.()?.find(value => value.id === source.id)
    if (!current || current.archived || current.active_job_id) return
    void action.run(() => api<SubscriptionSourceJob>(`${root}/subscription-sources/${source.id}/refresh`, 'POST', {}), () => { chooseSource(source.id); reload() })
  }
  const adopt = (node: ExternalNodePreview) => {
    if (!selected || selectionError() || !node.id) return
    const currentSource = sources.getCurrent?.()?.find(source => source.id === selected.id)
    if (!currentSource || currentSource.identity_epoch !== node.identity_epoch || currentSource.archived && !node.adopted) return
    const current = nodes.getCurrent?.()?.find(value => value.id === node.id && value.source_id === selected.id && value.node_version_id === node.node_version_id && value.identity_epoch === node.identity_epoch)
    if (sourceAdoptionError(node, current) || !current) return
    void action.run(() => api(`${root}/subscription-sources/${selected.id}/nodes/${node.id}`, 'PATCH', { adopted: !current.adopted, settings_revision: selected.settings_revision, identity_epoch: current.identity_epoch, node_version_id: current.node_version_id, metadata_revision: current.metadata_revision }), () => { reload(); onChange?.() })
  }
  const cancelJob = () => {
    if (selectedIdRef.current !== selected?.id || writeError(selected) || resourceWriteError(job)) return
    const current = job.getCurrent()
    const source = sources.getCurrent?.()?.find(value => value.id === selectedId)
    if (!current || !source || current.id !== source.active_job_id || current.source_id !== source.id || current.settings_revision !== source.settings_revision || current.identity_epoch !== source.identity_epoch || !['queued', 'running'].includes(current.state)) return
    void action.run(() => api(`${root}/subscription-source-jobs/${current.id}/cancel`, 'POST', {}), reload)
  }
  return <Panel data-slot="subscription-sources" className="mt-6" aria-labelledby="subscription-sources-title">
    <PanelHeader><div><PanelTitle id="subscription-sources-title">{migrated ? '数字编号来源（已迁移，只读）' : '订阅来源'}</PanelTitle><p className="text-muted-foreground mt-3 text-xs leading-relaxed">{migrated ? '这些来源已迁移到上方的订阅来源，节点编号、节点库设置和用户授权保持不变；此处仅保留迁移时的记录供查看，以后的版本会移除。' : '导入、更新来源，将选定节点加入节点库或用于链路。'}</p></div><div className="flex flex-wrap items-center justify-end gap-2"><Refresh onClick={reload} />{!migrated && <Button size="sm" disabled={Boolean(writeError())} onClick={() => open()}><PlusIcon aria-hidden="true" />添加来源</Button>}</div></PanelHeader>
    <div className="p-4"><ErrorNotice message={sources.error || (!editor && !deleting ? action.error : '')} retry={reload} /></div>
    {sources.loading && !sources.data ? <Loading variant="table" rows={3} /> : !sources.data?.length && migrated ? <div className="p-4"><p className="text-muted-foreground mt-3 text-xs leading-relaxed">迁移时没有数字编号来源。</p></div> : !sources.data?.length ? <Empty icon={WaypointsIcon} title="添加第一个订阅来源" description="支持订阅地址、分享链接、sing-box 配置和 Clash 节点配置。导入只保留代理节点。"><Button variant="outline" disabled={Boolean(writeError())} onClick={() => open()}>添加来源</Button></Empty> : <div className="m-0 grid list-none gap-3 p-0 px-5 pb-5 max-md:px-3.5 max-md:pb-4">{sources.data.map(source => <article key={source.id} className={`rounded-xl border p-4 transition-colors max-md:p-3.5 ${source.id === selectedId ? 'border-primary/50 bg-primary/[0.04] ring-primary/15 ring-1' : ''}`}>
      <div className="flex flex-wrap items-center justify-between gap-2.5 max-md:flex-col max-md:items-start"><Button variant="ghost" size="sm" className="-ml-2.5 h-auto text-left text-base font-semibold whitespace-normal wrap-anywhere aria-expanded:bg-transparent aria-expanded:text-primary" onClick={() => chooseSource(source.id)} aria-expanded={source.id === selectedId}>{source.name}</Button><Badge tone={source.archived ? 'neutral' : source.last_error ? 'warm' : source.supported_count ? 'good' : 'neutral'}>{sourceStatus(source)}</Badge></div>
      <div className="text-muted-foreground mt-2.5 mb-3 flex flex-wrap gap-x-4 gap-y-1.5 text-xs max-md:flex-col"><span>{source.kind === 'url' ? source.source_host : '粘贴或上传配置'}</span><span>最近解析：支持 {source.supported_count} · 不支持或身份待确认 {source.unsupported_count}</span><span>最后成功：{time(source.last_success_at)}</span></div>
      {source.changes && <div className="text-muted-foreground mt-2.5 mb-3 flex flex-wrap gap-x-4 gap-y-1.5 text-xs max-md:flex-col"><span>新增 {source.changes.added} · 更新 {source.changes.updated} · 缺失 {source.changes.missing}</span>{source.kind === 'url' && <span>{source.auto_refresh === false ? '自动更新已关闭' : `每 ${source.refresh_interval_seconds / 60} 分钟更新`}</span>}</div>}
      {source.traffic && (source.traffic.upload !== undefined || source.traffic.download !== undefined || source.traffic.total !== undefined || source.traffic.expire !== undefined) && <div className="text-muted-foreground my-2.5 flex flex-wrap gap-3 text-xs"><span>来源用量：{sourceReportedTotal(source.traffic.upload, source.traffic.download) !== undefined ? bytes(sourceReportedTotal(source.traffic.upload, source.traffic.download)!) : '未知'} / {knownSourceAmount(source.traffic.total) !== undefined ? bytes(source.traffic.total!) : '未知额度'}</span>{source.traffic.expire !== undefined && <span>到期：{knownSourceAmount(source.traffic.expire) !== undefined && source.traffic.expire > 0 ? time(source.traffic.expire) : '未知'}</span>}<small>提供方上报 · {time(source.traffic.updated_at ?? null)}{(source.stale || !sourceView.fresh) && ' · 上次成功数据'}</small></div>}
      {source.last_error && <p className="text-warning text-xs leading-relaxed wrap-anywhere">{sourceReason(source.last_error)}</p>}
      {!!source.dependency_ids.length && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">被 {source.dependency_ids.length} 条链路引用（编号 {source.dependency_ids.join('、')}）</p>}
      {migrated && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{typeof source.migrated_to === 'number' ? `已迁移为订阅来源 #${source.migrated_to}` : '迁移记录待确认'}</p>}
      {migrated ? <div className="flex flex-wrap items-center justify-end gap-2"><Button variant="ghost" size="sm" onClick={() => chooseSource(source.id)}>查看节点</Button></div> : <div className="flex flex-wrap items-center justify-end gap-2"><Button variant="ghost" size="sm" onClick={() => chooseSource(source.id)}>查看节点</Button><Button variant="ghost" size="sm" disabled={Boolean(writeError(source))} onClick={() => open(source)}>设置与更新</Button>{source.kind === 'url' && <Button variant="ghost" size="sm" disabled={source.archived || !!source.active_job_id || action.busy || Boolean(writeError(source))} onClick={() => refreshSource(source)}>立即更新</Button>}<Button variant="ghost" size="sm" disabled={action.busy || Boolean(writeError(source))} onClick={() => archive(source)}>{source.archived ? '恢复来源' : '归档'}</Button><Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={Boolean(writeError(source))} onClick={() => { if (writeError(source)) return; action.clearError(); setDeleting(source) }}>删除</Button></div>}
    </article>)}</div>}
    {selected && <div className="border-t p-5 max-md:px-3.5 max-md:py-4">
      <div className="mb-3 flex flex-wrap items-center justify-between gap-2"><h3 className="m-0 text-base font-semibold">{selected.name} · 节点预览</h3><span className="text-muted-foreground text-xs">来源代次 {selected.identity_epoch} · 成功批次 {selected.current_revision_id ?? '尚无'}</span></div>
      {job.data && ['queued', 'running'].includes(job.data.state) && <div className="mb-4 flex items-center gap-2.5 text-sm" role="status"><Spinner role="presentation" aria-hidden="true" aria-label={undefined} />{job.data.phase === 'downloading' ? '正在获取订阅…' : job.data.phase === 'parsing' ? '正在解析节点…' : '等待刷新…'}<Button variant="ghost" size="sm" disabled={action.busy || Boolean(writeError(selected) || resourceWriteError(job))} onClick={cancelJob}>取消刷新</Button></div>}
      <ErrorNotice message={nodes.error || job.error} retry={nodes.reload} />
      {onSelect && <Field label="选中节点后的更新方式"><NativeSelect className="w-full" value={mode} onChange={event => { if (['follow_node', 'pinned'].includes(event.target.value)) setMode(event.target.value as SubscriptionHopReference['update_mode']) }}><option value="follow_node">跟随所选节点更新</option><option value="pinned">固定当前版本</option></NativeSelect></Field>}
      {nodes.loading && !nodes.data ? <Loading /> : !nodes.data?.length ? <p className="text-muted-foreground mt-3 text-xs leading-relaxed">尚无解析结果。完成导入后会在此显示支持项与拒绝原因。</p> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead className="whitespace-nowrap">节点</TableHead><TableHead className="whitespace-nowrap">协议与端点</TableHead><TableHead className="whitespace-nowrap">版本与用途</TableHead><TableHead className="whitespace-nowrap">状态</TableHead><TableHead className="whitespace-nowrap">节点库</TableHead>{onSelect && <TableHead className="whitespace-nowrap">选择</TableHead>}</TableRow></TableHeader><TableBody>{nodes.data.map((node, index) => <TableRow key={`${node.id ?? 'rejected'}-${index}`}><TableCell className="min-w-32"><strong>{node.name}</strong>{node.id !== null && <small className="text-muted-foreground mt-1 block">节点 #{node.id}</small>}</TableCell><TableCell className="min-w-32">{node.protocol ? <>{node.protocol}<small className="max-w-56 wrap-anywhere text-muted-foreground mt-1 block">{node.server?.includes(':') ? `[${node.server}]` : node.server}:{node.port}</small><small className="text-muted-foreground mt-1 block">传输：{node.transport}</small></> : '无法转换'}</TableCell><TableCell className="min-w-32">{node.node_version_id ? <>版本 #{node.node_version_id}<small className="text-muted-foreground mt-1 block">{node.tcp ? '支持 TCP' : '不支持 TCP'} · {node.udp ? '支持 UDP' : '仅 TCP'}</small></> : '无可用版本'}</TableCell><TableCell className="min-w-32">{node.selectable ? <Badge tone="good">可选</Badge> : <span className="text-warning text-xs leading-relaxed wrap-anywhere">{sourceReason(node.reason)}</span>}</TableCell><TableCell className="min-w-32">{node.id !== null && (migrated ? node.adopted ? '已加入' : '未加入' : <Button variant="ghost" size="sm" disabled={action.busy || Boolean(selectionError() || sourceAdoptionError(node, node))} onClick={() => adopt(node)}>{node.adopted ? '移出节点库' : '加入节点库'}</Button>)}</TableCell>{onSelect && <TableCell className="min-w-32"><Button variant="outline" size="sm" disabled={!node.selectable || Boolean(selectionError())} onClick={() => { if (selectionError() || !['follow_node', 'pinned'].includes(mode)) return; const current = nodes.getCurrent?.()?.find(value => value.id === node.id && value.node_version_id === node.node_version_id && value.source_id === node.source_id && value.identity_epoch === node.identity_epoch); const reference = current?.selectable && hopReference(current, mode); if (reference && current) onSelect(reference, current) }}>加入此段</Button></TableCell>}</TableRow>)}</TableBody></Table></div>}
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">“可选”表示参数可用于链路配置。放置位置还需校验整条路径的承载能力；解析成功不代表机场账户有效或链路连通。节点缺失或更新失败时，已有链路保留已应用版本。</p>
    </div>}
    {editor && !editor.source && <SourceImport writeError={() => writeError()} onClose={() => setEditor(null)} onSaved={source => { setEditor(null); chooseSource(source.id); reload(); onChange?.() }} />}
    {editor?.source && <SourceEditor source={editor.source} writeError={() => writeError(editor.source)} onClose={() => setEditor(null)} onSaved={source => { setEditor(null); chooseSource(source.id); reload(); onChange?.() }} />}
    {deleting && <Confirm title={`删除「${deleting.name}」？`} busy={action.busy} confirmDisabled={Boolean(writeError(deleting))} error={writeError(deleting) || action.error} onClose={() => setDeleting(null)} onConfirm={() => { if (writeError(deleting)) return; void action.run(() => api(`${root}/subscription-sources/${deleting.id}`, 'DELETE'), () => { if (selectedIdRef.current === deleting.id) { selectedIdRef.current = null; setSelectedId(null) } setDeleting(null); reload(); onChange?.() }) }}>仍被链路或代理用户引用的来源不能删除。归档可以停止刷新和新引用，已有链路继续使用保存的版本；历史版本和发布记录会保留。</Confirm>}
  </Panel>
}

function SourceEditor({ source, writeError, onClose, onSaved }: { source?: SubscriptionSource; writeError: () => string; onClose: () => void; onSaved: (source: SubscriptionSource) => void }) {
  const action = useAction()
  const [kind, setKind] = useState(source?.kind ?? 'url')
  const [url, setUrl] = useState('')
  const [authorization, setAuthorization] = useState('')
  const [content, setContent] = useState('')
  const [clearAuthorization, setClearAuthorization] = useState(false)
  const [replaceSource, setReplaceSource] = useState(false)
  const [fileError, setFileError] = useState('')
  const [autoRefresh, setAutoRefresh] = useState(source?.auto_refresh !== false)
  const upload = async (file?: File) => {
    if (!file) return
    if (file.size > 2 * 1024 * 1024) { setFileError('配置文件不能超过 2 MiB。'); return }
    try { setContent(await file.text()); setFileError('') } catch (error) { setFileError(errorMessage(error)) }
  }
  const submit = (form: FormData) => void action.run(async () => {
    const stale = writeError(); if (stale) throw new Error(stale)
    if (new TextEncoder().encode(content).length > 2 * 1024 * 1024) throw new Error('配置内容不能超过 2 MiB。')
    const body: Record<string, unknown> = { name: String(form.get('name') ?? '').trim() }
    if (source) body.settings_revision = source.settings_revision
    else body.kind = kind
    if (kind === 'url') {
      body.refresh_interval_seconds = Number(form.get('refresh_minutes')) * 60; body.auto_refresh = autoRefresh; body.user_agent = String(form.get('user_agent') ?? 'Apsp-subscription-import/1')
      if (url.trim()) body.url = url.trim()
      if (authorization) body.authorization = authorization
      if (clearAuthorization) body.clear_authorization = true
    } else if (content) { body.content = content; if (source) body.replace_source = replaceSource }
    return api<SubscriptionSource>(`${root}/subscription-sources${source ? `/${source.id}` : ''}`, source ? 'PATCH' : 'POST', body)
  }, saved => { setUrl(''); setAuthorization(''); setContent(''); onSaved(saved) })
  return <FormDialog title={source ? `设置「${source.name}」` : '添加订阅来源'} wide onClose={onClose} onSubmit={submit} busy={action.busy} submitDisabled={Boolean(writeError())} error={writeError() || action.error || fileError} submitLabel={source ? '保存并解析更新' : '保存并解析'}>
    <Field label="来源名称"><Input name="name" maxLength={128} required defaultValue={source?.name ?? ''} autoComplete="off" /></Field>
    {!source && <Field label="来源类型"><NativeSelect className="w-full" value={kind} onChange={event => { setKind(event.target.value as 'url' | 'inline'); setUrl(''); setAuthorization(''); setContent('') }}><option value="url">HTTPS 订阅地址</option><option value="inline">粘贴或上传配置</option></NativeSelect></Field>}
    {kind === 'url' ? <>
      <Field label={source ? '替换订阅地址' : 'HTTPS 订阅地址'} hint={source ? '已配置的完整地址不会回显。留空保留；更换地址会建立新来源身份，原链路不会自动换机场。' : '仅接受公网 HTTPS 地址。地址中的路径和查询可能包含令牌。'}><Input type="password" name="subscription_url" autoComplete="new-password" spellCheck={false} required={!source} maxLength={8192} placeholder={source?.url_configured ? '已配置，留空保留' : 'https://…'} value={url} onChange={event => setUrl(event.target.value)} /></Field>
      <Field label="获取认证（可选）" hint="需要时填写完整 Authorization 值。留空保留已配置的认证。"><Input type="password" autoComplete="new-password" spellCheck={false} maxLength={8192} disabled={clearAuthorization} placeholder={source?.authorization_configured ? '已配置，留空保留' : '按来源要求填写'} value={authorization} onChange={event => setAuthorization(event.target.value)} /></Field>
      {source?.authorization_configured && <label className="mb-4 flex items-center gap-2.5 text-sm"><Checkbox checked={clearAuthorization} onCheckedChange={checked => { setClearAuthorization(checked === true); if (checked === true) setAuthorization('') }} />清除已配置的获取认证</label>}
      <Field label="请求标识"><Input name="user_agent" required maxLength={256} defaultValue={source?.user_agent ?? 'Apsp-subscription-import/1'} /></Field>
      <label className="mb-4 flex items-center gap-2.5 text-sm"><Checkbox checked={autoRefresh} onCheckedChange={checked => setAutoRefresh(checked === true)} />自动更新来源</label>
      <Field label="自动刷新间隔（分钟）" hint="默认每天一次；允许 5 分钟至 30 天。"><Input name="refresh_minutes" type="number" min={5} max={43200} step={1} required defaultValue={source ? source.refresh_interval_seconds / 60 : 1440} /></Field>
    </> : <>
      <Field label="上传配置文件" hint="支持文本、JSON、YAML，最大 2 MiB。"><Input type="file" accept=".txt,.json,.yaml,.yml,text/plain,application/json" onChange={event => void upload(event.target.files?.[0])} /></Field>
      <Field label={source ? '更新配置内容' : '配置内容'} hint={source ? '留空保留当前内容；原内容不会回显。' : '粘贴分享链接、Base64 订阅、sing-box JSON 或 Clash / Mihomo 节点配置。'}><Textarea name="source_content" rows={9} required={!source} autoComplete="off" spellCheck={false} value={content} onChange={event => setContent(event.target.value)} /></Field>
      {source && <Field label="内容更新方式"><NativeSelect className="w-full" value={replaceSource ? 'replace' : 'same'} onChange={event => setReplaceSource(event.target.value === 'replace')}><option value="same">同一来源更新内容，按明确节点身份匹配</option><option value="replace">更换来源，原链路保留旧版本并需要重新选点</option></NativeSelect></Field>}
    </>}
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">保存后只显示来源主机和节点公开预览。敏感内容仅在当前表单内存中保留，保存成功即清空。归档来源停止刷新；恢复后可再次提交更新。</p>
  </FormDialog>
}
