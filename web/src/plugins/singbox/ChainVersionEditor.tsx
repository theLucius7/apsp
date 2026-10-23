import { useEffect, useRef, useState } from 'react'
import { Field, FormDialog } from '../../components'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import { validatedSnapshot } from './groupTypes'
import type { ProxyResource, ResourceSnapshot } from './groupTypes'
import { chainMutationError, prepareChainMutation, submitChainMutation } from './chainRequests'
import type { ChainMutation, PendingChainMutation } from './chainRequests'
import { sourceRoot, validSourceNodePage } from './orderedSourceTypes'
import type { SourceNodePage } from './orderedSourceTypes'
import { NativeSelect } from '@/components/ui/native-select'

export default function ChainVersionEditor({ resource, snapshot, open, onClose, onSaved, onPending, refresh }: { resource: ProxyResource; snapshot: ResourceSnapshot<ProxyResource[]>; open: boolean; onClose: () => void; onSaved: () => void; onPending: (value: boolean) => void; refresh: () => void }) {
  const external = resource.hops.filter(hop => hop.kind === 'subscription')
  const [pages, setPages] = useState<Record<number, ResourceSnapshot<SourceNodePage>>>({})
  const [selected, setSelected] = useState<Record<number, string>>({})
  const observe = useRef((id: number, value: ResourceSnapshot<SourceNodePage>) => setPages(current => ({ ...current, [id]: value }))).current
  const action = useAction(), pending = useRef<PendingChainMutation | undefined>(undefined)
  const [requestId, setRequestId] = useState('')
  const versions = external.filter(hop => selected[hop.position]).map(hop => ({ hop_position: hop.position, node_version_id: selected[hop.position] }))
  const command: ChainMutation = { kind: 'chain', id: resource.id, settings_revision: resource.settings_revision, operation: 'versions', fields: { generation: resource.path_state!.desired_generation, versions } }
  const replay = pending.current?.attempted === true && pending.current.command === JSON.stringify(command)
  const guard = chainMutationError(snapshot, command, replay)
  const selectionError = () => {
    for (const hop of external.filter(hop => selected[hop.position])) {
      const page = pages[hop.source_id], current = page?.getCurrent ? page.getCurrent() : page?.data
      const node = current?.nodes.find(node => node.id === hop.external_node_id && node.version_id === selected[hop.position])
      if (!page || resourceWriteError(page) || current?.source_id !== hop.source_id || current.current_identity_epoch !== hop.identity_epoch || !node?.selectable || node.identity_epoch !== hop.identity_epoch) return `第 ${hop.position} 跳的同节点、同来源代次最新资格尚未确认。`
    }
    return ''
  }
  if (!open) return null
  return <FormDialog wide title="应用同节点新版本" onClose={onClose} busy={action.busy} disabled={Boolean(guard)} submitDisabled={!versions.length || Boolean(selectionError())} error={guard || selectionError() || action.error} retry={guard ? refresh : undefined} submitLabel={replay ? '重试原版本请求' : '创建版本候选'} onSubmit={() => {
    if (action.busy || chainMutationError(snapshot, command, replay) || selectionError() || !versions.length) return
    void action.run(async () => { const stale = selectionError(); if (stale) throw new Error(stale); const request = prepareChainMutation(command, snapshot, pending.current); pending.current = request; setRequestId(request.request_id); onPending(true); return submitChainMutation(request, snapshot) }, () => { pending.current = undefined; onPending(false); refresh(); onSaved() })
  }}><p className="text-muted-foreground mt-3 text-xs leading-relaxed">仅选择同一明确节点的新版本，保持路径身份、顺序、入口和授权不变。固定模式通过此操作显式更新；跟随模式已进入候选时，先等待该候选结束。每条链路同时只允许一个候选；刷新失败、源归档、缺失或身份歧义不会自动换节点。</p>
    {[...new Set(external.map(hop => hop.source_id))].map(id => <VersionObservation key={id} id={id} observe={observe} />)}
    {external.map(hop => { const page = pages[hop.source_id]; const choices = page?.data?.nodes.filter(node => node.id === hop.external_node_id && node.identity_epoch === hop.identity_epoch && node.selectable && node.version_id !== hop.node_version_id) ?? []
      return <Field key={hop.position} label={`第 ${hop.position} 跳：${hop.name}`} hint={`当前输入 ${hop.node_version_id} · ${hop.update_mode === 'pinned' ? '固定版本' : '跟随所选节点'} · 来源代次 ${hop.identity_epoch}`}><NativeSelect className="w-full" value={selected[hop.position] ?? ''} disabled={!page?.fresh || Boolean(page.error)} onChange={event => { action.clearError(); setSelected(current => ({ ...current, [hop.position]: event.target.value })) }}><option value="">保持当前输入版本</option>{selected[hop.position] && !choices.some(node => node.version_id === selected[hop.position]) && <option value={selected[hop.position]}>所选版本已变化，等待确认</option>}{choices.map(node => <option key={node.version_id} value={node.version_id}>{node.name} · {node.version_id}</option>)}</NativeSelect>{page?.error && <small>{page.error}</small>}{page?.fresh && !choices.length && <small>当前视图暂无可选新版本；此表单不提供历史版本或其他身份选择。</small>}</Field>
    })}
    {requestId && <p className="text-muted-foreground mt-3 text-xs leading-relaxed" role="status">操作编号 <code>{requestId}</code>。未改选择时重试原键和原内容；返回收据仅表示候选已规划，以资源详情中的已应用代为准。</p>}
  </FormDialog>
}
function VersionObservation({ id, observe }: { id: number; observe: (id: number, value: ResourceSnapshot<SourceNodePage>) => void }) {
  const query = useResource<unknown>(`${sourceRoot}/${id}/nodes`), history = useRef<SourceNodePage | undefined>(undefined)
  const value = validatedSnapshot(query, validSourceNodePage, history.current)
  if (value.fresh && value.data?.source_id === id) history.current = value.data
  useEffect(() => observe(id, { ...value, isCurrent: () => value.isCurrent?.() === true && value.getCurrent?.()?.source_id === id, getCurrent: () => value.getCurrent?.()?.source_id === id ? value.getCurrent() : undefined, fresh: value.fresh && value.data?.source_id === id }), [id, observe, value.data, value.fresh, value.error])
  return null
}
