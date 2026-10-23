import { useEffect, useRef, useState } from 'react'
import { api } from '../../api'
import { Badge, ErrorNotice, Field, FormDialog, Loading } from '../../components'
import { resourceWriteError, useAction, useResource } from '../../hooks'

import { eligibleExternalSelection, externalDraftError, externalReference as reference, validExternalAccessView } from './externalAccessTypes'
import type { ExternalDraft as Draft, ExternalReference as Reference, ExternalAccessView as AccessView } from './externalAccessTypes'
import { validatedSnapshot } from './groupTypes'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
const reasons: Record<string, string> = { source_deleted: '来源已删除', source_archived: '来源已归档', source_replaced: '来源身份已更换', node_deleted: '节点已删除', node_not_adopted: '节点已取消采用', node_disabled: '节点已停用', ambiguous_node_identity: '节点身份不明确', node_missing: '来源中已缺失', version_missing: '版本不可用', version_identity_mismatch: '版本身份不匹配', invalid_version: '版本校验失败', parser_update_required: '请先更新订阅来源' }

export default function ExternalUserAccess({ userId, userError, refreshRevision, onChanged }: { userId: number; userError: () => string; refreshRevision: number; onChanged?: () => void }) {
  const query = useResource<unknown>(`/api/plugins/sing-box/users/${userId}/external-accesses`)
  const history = useRef<{ userId: number; data?: AccessView }>({ userId })
  if (history.current.userId !== userId) history.current = { userId }
  const view = validatedSnapshot(query, validExternalAccessView, history.current.data)
  if (view.fresh) history.current.data = view.data
  const resource = { ...query, ...view, data: view.data, getCurrent: () => view.getCurrent?.(), isCurrent: () => view.isCurrent?.() === true }
  useEffect(() => { query.reload() }, [refreshRevision, query.reload])
  const action = useAction()
  const [draft, setDraft] = useState<Draft | null>(null)
  const [search, setSearch] = useState('')
  const [saved, setSaved] = useState<number | null>(null)
  const [readDraft, setReadDraft] = useState<number | null>(null)
  const currentDraft = draft?.userId === userId ? draft : null
  const writeError = () => userError() || resourceWriteError(resource)
  const error = () => writeError() || (draft ? externalDraftError(draft, resource.getCurrent?.(), userId) : '')
  const open = () => {
    const current = resource.getCurrent?.()
    if (writeError() || !current) return
    action.clearError(); setSearch(''); setDraft({ userId, revision: current.revision, accesses: current.accesses.map(reference) })
  }
  useEffect(() => {
    if (readDraft === null) return
    if (readDraft !== userId || (resource.error && !resource.loading)) { setReadDraft(null); return }
    const current = resource.getCurrent?.()
    if (!current || userError()) return
    action.clearError(); setDraft({ userId, revision: current.revision, accesses: current.accesses.map(reference) }); setReadDraft(null)
  }, [readDraft, userId, resource.ready, resource.data, resource.error, resource.loading, userError])
  const update = (accesses: Reference[]) => { if (writeError()) return; setDraft(previous => previous?.userId === userId ? { ...previous, accesses } : previous) }
  const submit = () => {
    if (!currentDraft || error()) return
    const selectedUser = userId
    void action.run(() => api(`/api/plugins/sing-box/users/${selectedUser}/external-accesses`, 'PUT', { revision: currentDraft.revision, accesses: currentDraft.accesses }), () => { setDraft(null); setSaved(selectedUser); resource.reload(); onChanged?.() })
  }
  const entries = resource.data?.accesses ?? []
  const available = resource.data?.available_nodes ?? []
  const chosen = new Set(currentDraft?.accesses.map(value => value.external_node_id))
  const needle = search.trim().toLocaleLowerCase()
  const candidates = available.filter(entry => !chosen.has(entry.external_node_id) && (!needle || `${entry.name} ${entry.source_name} ${entry.protocol} ${entry.server}`.toLocaleLowerCase().includes(needle)))
  return <Panel aria-label="外部节点授权" style={{ overflowWrap: 'anywhere' }}>
    <PanelHeader><PanelTitle>外部节点 <span className="bg-muted text-muted-foreground min-w-5 rounded px-1.5 text-center text-xs font-medium tabular-nums">{entries.length}</span></PanelTitle><Button variant="outline" size="sm" disabled={Boolean(writeError())} onClick={open}>管理分配</Button></PanelHeader>
    <div className="p-4">
      <ErrorNotice message={resource.error} retry={resource.reload} />
      {resource.loading && !resource.data ? <Loading /> : entries.length ? <div className="my-3 mb-4 grid gap-2.5 border-0 p-0">{entries.map(entry => <div className="flex cursor-pointer items-center gap-3 py-2.5 text-sm" style={{ flexWrap: 'wrap' }} key={entry.external_node_id}><span><strong>{entry.name}</strong><small className="text-muted-foreground mt-0.5 block text-xs">{entry.source_name} · {entry.protocol} · {entry.update_mode === 'pinned' ? '固定版本' : '跟随更新'}</small></span><Badge tone={entry.available ? 'good' : 'warm'}>{entry.available ? entry.source_last_error ? '保留上次成功版本' : '可生成订阅' : reasons[entry.reason ?? ''] ?? '当前不可用'}</Badge></div>)}</div> : <p className="text-muted-foreground mt-3 text-xs leading-relaxed">尚未分配外部节点。</p>}
      {saved === userId && <p role="status" className="text-muted-foreground mt-3 text-xs leading-relaxed">外部节点分配已保存，后续订阅按新分配生成。</p>}
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">外部运行和流量由提供方控制，本面板不计量外部用量；取消分配不会让已下载的外部凭据失效。</p>
    </div>
    {currentDraft && <FormDialog title="分配外部节点" wide onClose={() => setDraft(null)} onSubmit={submit} busy={action.busy} submitDisabled={Boolean(error())} error={error() || action.error} submitLabel="保存分配">
      <div className="flex flex-wrap items-center justify-end gap-2"><strong>已选 {currentDraft.accesses.length} 个</strong><Button variant="ghost" size="sm" type="button" disabled={action.busy || readDraft !== null || Boolean(userError())} onClick={() => { if (userError()) return; setReadDraft(userId); resource.reload() }}>重新读取授权</Button></div>
      <fieldset data-slot="group-choices" className="my-3 mb-4 grid gap-2.5 border-0 p-0"><legend className="mb-2.5 text-sm font-semibold">已分配节点</legend>{currentDraft.accesses.map(value => {
        const candidate = available.find(entry => entry.external_node_id === value.external_node_id)
        const entry = entries.find(entry => entry.external_node_id === value.external_node_id) ?? candidate
        return <div className="grid gap-4 sm:grid-cols-2" key={value.external_node_id}><label className="flex cursor-pointer items-center gap-3 py-2.5 text-sm"><Checkbox checked disabled={action.busy || Boolean(writeError())} onCheckedChange={() => update(currentDraft.accesses.filter(item => item.external_node_id !== value.external_node_id))} /><span>{entry?.name ?? `外部节点 #${value.external_node_id}`}<small className="text-muted-foreground mt-0.5 block text-xs">{entry?.source_name}{entry && !entry.available ? ` · ${reasons[entry.reason ?? ''] ?? '当前不可用'}` : ''}</small></span></label><Field label="更新方式"><NativeSelect className="w-full" aria-label={`${entry?.name ?? value.external_node_id} 更新方式`} value={value.update_mode} disabled={action.busy || Boolean(writeError()) || !candidate || !eligibleExternalSelection(candidate)} onChange={event => { if (writeError() || !['follow_node', 'pinned'].includes(event.target.value)) return; const current = resource.getCurrent?.()?.available_nodes.find(entry => entry.external_node_id === value.external_node_id); if (current && eligibleExternalSelection(current)) update(currentDraft.accesses.map(item => item.external_node_id === value.external_node_id ? { ...reference(current), update_mode: event.target.value as Reference['update_mode'] } : item)) }}><option value="follow_node">跟随同一节点更新</option><option value="pinned">固定所选版本</option></NativeSelect></Field></div>
      })}{!currentDraft.accesses.length && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">从下方选择已采用的外部节点。</p>}</fieldset>
      <Field label="搜索外部节点"><Input value={search} onChange={event => setSearch(event.target.value)} placeholder="名称、来源、协议或地址" autoComplete="off" /></Field>
      <fieldset data-slot="group-choices" className="my-3 mb-4 grid gap-2.5 border-0 p-0"><legend className="mb-2.5 text-sm font-semibold">可选节点</legend>{candidates.map(entry => <label className="flex cursor-pointer items-center gap-3 py-2.5 text-sm" key={entry.external_node_id}><Checkbox checked={false} disabled={action.busy || Boolean(writeError()) || !eligibleExternalSelection(entry)} onCheckedChange={() => { if (writeError()) return; const current = resource.getCurrent?.()?.available_nodes.find(value => value.external_node_id === entry.external_node_id && value.source_id === entry.source_id && value.identity_epoch === entry.identity_epoch && value.node_version_id === entry.node_version_id && value.metadata_revision === entry.metadata_revision); if (current && eligibleExternalSelection(current) && !currentDraft.accesses.some(value => value.external_node_id === current.external_node_id)) update([...currentDraft.accesses, reference(current)]) }} /><span>{entry.name}<small className="text-muted-foreground mt-0.5 block text-xs">{entry.source_name} · {entry.protocol} · {entry.server}:{entry.port}{!entry.available ? ` · ${reasons[entry.reason ?? ''] ?? '当前不可用'}` : entry.source_last_error ? ' · 保留上次成功版本' : ''}</small></span></label>)}{!candidates.length && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">没有符合条件的未选节点。请先在节点管理中采用并启用外部节点。</p>}</fieldset>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">生成订阅仍检查用户套餐状态；外部节点使用提供方现有凭据，用量未知。</p>
    </FormDialog>}
  </Panel>
}
