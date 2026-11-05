import { Checkbox } from '@/components/ui/checkbox'
import { NativeSelect } from '@/components/ui/native-select'
import { eligibleExternalSelection, externalReference, sameExternalReference } from './externalAccessTypes'
import type { ExternalEntry, ExternalReference } from './externalAccessTypes'

export const externalReasons: Record<string, string> = { source_deleted: '来源已删除', source_archived: '来源已归档', source_replaced: '来源身份已更换', node_deleted: '节点已删除', node_not_adopted: '节点已取消采用', node_disabled: '节点已停用', ambiguous_node_identity: '节点身份不唯一', node_missing: '来源中暂时缺少此节点', version_missing: '版本缺失', version_identity_mismatch: '版本与节点身份不一致', invalid_version: '版本无法使用', parser_update_required: '需要更新解析器后重新选择' }

// A group keeps what it already holds even while unavailable; a node added to it
// must be adopted, enabled and on its current version.
export function externalSelectionError(selected: ExternalReference[], held: ExternalEntry[], candidates: ExternalEntry[]) {
  for (const value of selected) {
    if (held.some(entry => sameExternalReference(entry, value))) continue
    const entry = candidates.find(entry => entry.external_node_id === value.external_node_id)
    if (!entry || !eligibleExternalSelection(entry) || entry.source_id !== value.source_id || entry.identity_epoch !== value.identity_epoch || entry.node_version_id !== value.node_version_id || entry.metadata_revision !== value.metadata_revision) return '所选外部节点已缺失、停用或来源与版本已变化，请重新选择；其余草稿已保留。'
  }
  return ''
}

export default function GroupExternalNodes({ selected, held, candidates, disabled, onChange }: { selected: ExternalReference[]; held: ExternalEntry[]; candidates: ExternalEntry[]; disabled: boolean; onChange: (next: ExternalReference[]) => void }) {
  const rows = [...held, ...candidates.filter(entry => !held.some(value => value.external_node_id === entry.external_node_id))]
  // A ticked node that is neither held nor offered any more stays visible so it can be unticked.
  const orphans = selected.filter(value => !rows.some(entry => entry.external_node_id === value.external_node_id))
  if (!rows.length && !orphans.length) return null
  const toggle = (entry: ExternalEntry, checked: boolean) => {
    if (!checked) return onChange(selected.filter(value => value.external_node_id !== entry.external_node_id))
    const kept = held.find(value => value.external_node_id === entry.external_node_id)
    const candidate = candidates.find(value => value.external_node_id === entry.external_node_id)
    const next = kept ?? (candidate && eligibleExternalSelection(candidate) ? candidate : undefined)
    if (next && !selected.some(value => value.external_node_id === next.external_node_id)) onChange([...selected, externalReference(next)])
  }
  const mode = (entry: ExternalEntry, value: string) => {
    if (value !== 'follow_node' && value !== 'pinned') return
    const kept = held.find(item => item.external_node_id === entry.external_node_id && item.update_mode === value)
    const candidate = candidates.find(item => item.external_node_id === entry.external_node_id)
    const next = kept ?? (candidate && eligibleExternalSelection(candidate) ? { ...externalReference(candidate), update_mode: value } : undefined)
    if (next) onChange(selected.map(item => item.external_node_id === entry.external_node_id ? externalReference(next) : item))
  }
  return <fieldset data-slot="group-choices" className="my-3 mb-4 grid gap-2.5 border-0 p-0"><legend className="mb-2.5 text-sm font-semibold">订阅来源中的外部节点</legend>
    {rows.map(entry => {
      const chosen = selected.find(value => value.external_node_id === entry.external_node_id)
      const selectable = Boolean(chosen) || eligibleExternalSelection(entry)
      return <div key={entry.external_node_id} className="grid grid-cols-[minmax(0,1fr)] items-center gap-2 sm:grid-cols-[minmax(0,1fr)_11rem]">
        <label className="flex min-w-0 cursor-pointer items-center gap-3 py-1.5 text-sm"><Checkbox checked={Boolean(chosen)} disabled={disabled || !selectable} onCheckedChange={checked => toggle(entry, checked === true)} /><span className="min-w-0">{entry.name}<small className="text-muted-foreground mt-0.5 block truncate text-xs">{entry.source_name}{entry.protocol ? ` · ${entry.protocol}` : ''}{!entry.available ? ` · ${externalReasons[entry.reason ?? ''] ?? '当前不可用'}` : entry.source_last_error ? ' · 保留上次成功版本' : ''}</small></span></label>
        {chosen && <NativeSelect className="w-full" aria-label={`${entry.name} 的更新方式`} value={chosen.update_mode} disabled={disabled} onChange={event => mode(entry, event.target.value)}><option value="follow_node">跟随节点更新</option><option value="pinned">固定当前版本</option></NativeSelect>}
      </div>
    })}
    {orphans.map(value => <label key={`missing-${value.external_node_id}`} className="flex cursor-pointer items-center gap-3 py-1.5 text-sm"><Checkbox checked disabled={disabled} onCheckedChange={() => onChange(selected.filter(item => item.external_node_id !== value.external_node_id))} /><span>外部节点 #{value.external_node_id}<small className="text-muted-foreground mt-0.5 block text-xs">已不可用，请取消选择</small></span></label>)}
  </fieldset>
}
