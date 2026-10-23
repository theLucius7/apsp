import { useRef, useState } from 'react'
import { Field } from '../../components'
import { useResource } from '../../hooks'
import { validatedSnapshot } from './groupTypes'
import type { ChainBatchDraft, ChainDraftHop, ProxyWriteSnapshot } from './Chains'
import { sourceRoot, validSourceNodePage } from './orderedSourceTypes'
import type { SourceNodePage } from './orderedSourceTypes'
import { Button } from '@/components/ui/button'
import { NativeSelect } from '@/components/ui/native-select'
import { Notice } from '@/components/panel'

export function expandChainExits(draft: ChainBatchDraft, exits: ChainDraftHop[]): ChainBatchDraft {
  if (!draft.hops?.length || !exits.length || exits.length > 32 || draft.mode === 'existing' && exits.length !== 1) throw new Error('请明确选择 1–32 个出口；已有入口只支持一条链路。')
  const prefix = draft.rows[0]?.name.trim() || '链路'
  const middle = draft.hops.slice(0, -1)
  return { ...draft, rows: exits.map((exit, index) => ({ name: `${prefix} · 出口 ${index + 1}`, port: '', hops: [...middle.map(hop => ({ ...hop })), { ...exit }] })) }
}
export default function ChainExitExpansion({ draft, snapshot, onChange }: { draft: ChainBatchDraft; snapshot: ProxyWriteSnapshot; onChange: (draft: ChainBatchDraft) => void }) {
  const [managedIds, setManaged] = useState<string[]>([]), [sourceId, setSource] = useState(''), [externalIds, setExternal] = useState<string[]>([])
  const query = useResource<unknown>(sourceId ? `${sourceRoot}/${sourceId}/nodes` : null)
  const history = useRef<SourceNodePage | undefined>(undefined), page = validatedSnapshot(query, validSourceNodePage, history.current)
  if (page.fresh && page.data?.source_id === Number(sourceId)) history.current = page.data
  const source = snapshot.sources?.data?.find(source => source.id === Number(sourceId))
  const fresh = page.fresh && !page.error && page.data?.source_id === source?.id && page.data?.current_identity_epoch === source?.identity_epoch && snapshot.sources?.fresh && !source?.archived
  const managed = snapshot.nodes.data?.filter(node => node.protocol === 'vless-reality' && node.enabled !== false && snapshot.resources.data?.some(resource => resource.kind === 'direct' && resource.id === node.id && resource.available) && snapshot.servers.data?.some(server => server.id === node.server_id && server.enabled)) ?? []
  const count = managedIds.length + externalIds.length
  const [error, setError] = useState('')
  return <details className="my-4 rounded-lg border p-3.5 [&[open]>summary]:mb-5"><summary className="cursor-pointer text-sm font-semibold">多选出口，明确展开为独立链路</summary><p className="text-muted-foreground mt-3 text-xs leading-relaxed">复用共享路径中的中间段，为每个所选出口生成一行；不组合出笛卡尔积。展开会替换当前预览行并清空监听端口，每行仍可单独编辑。</p>
    <Field label="选择多个受管出口"><NativeSelect className="w-full" multiple value={managedIds} onChange={event => { setError(''); setManaged([...event.target.selectedOptions].map(option => option.value)) }}>{managed.map(node => <option key={node.id} value={node.id}>{node.name} · 服务器 #{node.server_id}</option>)}</NativeSelect></Field>
    <Field label="选择出口订阅来源"><NativeSelect className="w-full" value={sourceId} disabled={!snapshot.sources?.fresh || Boolean(snapshot.sources?.error)} onChange={event => { setError(''); setSource(event.target.value); setExternal([]); history.current = undefined }}><option value="">暂不选择订阅出口</option>{snapshot.sources?.data?.map(source => <option key={source.id} value={source.id} disabled={source.archived || !source.latest_success}>{source.name}</option>)}</NativeSelect></Field>
    {sourceId && <Field label="选择多个订阅出口"><NativeSelect className="w-full" multiple value={externalIds} disabled={!fresh} onChange={event => { setError(''); setExternal([...event.target.selectedOptions].map(option => option.value)) }}>{page.data?.source_id === Number(sourceId) && page.data.nodes.map(node => <option key={node.version_id} value={node.version_id} disabled={!node.selectable}>{node.name} · {node.version_id.slice(0, 8)}{node.selectable ? '' : ' · 不可引用'}</option>)}</NativeSelect></Field>}
    {(error || page.error) && <Notice tone="error" role="alert">{error || page.error}</Notice>}
    <Button type="button" variant="outline" disabled={!count || count > (draft.mode === 'existing' ? 1 : 32) || externalIds.length > 0 && !fresh} onClick={() => {
      try {
        const exits: ChainDraftHop[] = managedIds.map(node_id => ({ kind: 'managed', node_id }))
        for (const id of externalIds) { const node = page.data?.nodes.find(node => node.version_id === id && node.selectable); if (!node || !fresh) throw new Error('订阅出口资格已变化，请重新选择。'); exits.push({ kind: 'subscription', source_id: node.source_id, node, update_mode: 'follow_node' }) }
        onChange(expandChainExits(draft, exits)); setError('')
      } catch (error) { setError(error instanceof Error ? error.message : '出口展开失败。') }
    }}>用所选 {count} 个出口替换预览行</Button>
  </details>
}
