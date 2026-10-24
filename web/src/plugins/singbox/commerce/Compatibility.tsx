import { useState } from 'react'
import { api } from '../../../api'
import { ErrorNotice, Field, FormDialog } from '../../../components'
import { useAction, useResource } from '../../../hooks'
import { Panel, PanelHeader, PanelTitle, Notice } from '@/components/panel'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { NativeSelect } from '@/components/ui/native-select'
import { root } from './types'

type Import = { id: string; source_id: string; digest: string }
type Compatibility = { bridge: { enabled: boolean; subscription_base: string; last_sync_at: number | null } | null; mappings: { package_group_id: number; package_name: string; legacy_plan_id: number; legacy_group_id: number | null }[] }
export default function Compatibility() {
  const state = useResource<Compatibility>(`${root}/compatibility`, 15000), imports = useResource<{ entries: Import[] }>(`${root}/migration`), action = useAction()
  const [editing, setEditing] = useState(false)
  const reload = () => { state.reload(); imports.reload() }
  return <Panel><PanelHeader><PanelTitle>原订阅兼容通道</PanelTitle>{!state.data?.bridge && <Button size="sm" disabled={!state.isCurrent() || !imports.isCurrent()} onClick={() => setEditing(true)}>激活已核对的迁移</Button>}</PanelHeader><div className="space-y-3 p-4"><ErrorNotice message={state.error || imports.error} retry={reload} />
    {state.data?.bridge ? <><p>保留订阅路径：{state.data.bridge.subscription_base}</p><Notice role="status">{state.data.bridge.last_sync_at ? `最后同步：${new Date(state.data.bridge.last_sync_at * 1000).toLocaleString()}。` : '等待本机同步器首次同步。'}{!state.data.bridge.last_sync_at || Date.now() / 1000 - state.data.bridge.last_sync_at > 120 ? '同步过期，新购买暂不可用。' : '同步正常。'}</Notice></> : <p>完成旧业务只读、最终快照和套餐映射核对后激活。旧订阅与节点协议由兼容后端继续提供。</p>}
    {state.data?.mappings.map(p => <p key={p.package_group_id}>{p.package_name} → 原套餐 #{p.legacy_plan_id} · 权限组 {p.legacy_group_id ?? '缺失，禁止激活'}</p>)}
    {editing && <FormDialog title="激活 Nexus 迁移" onClose={() => setEditing(false)} busy={action.busy} error={action.error} submitDisabled={!imports.isCurrent() || !state.isCurrent()} submitLabel="确认激活" onSubmit={form => {
      const source = imports.data?.entries.find(i => i.id === form.get('import_id')); if (!source) return
      void action.run(() => api(`${root}/compatibility`, 'POST', { import_id: source.id, confirmed_digest: source.digest, subscription_base: form.get('subscription_base'), confirm_legacy_read_only: form.get('read_only') === 'on' }), () => { setEditing(false); reload() })
    }}><Field label="已核对的导入"><NativeSelect name="import_id" required>{imports.data?.entries.map(i => <option key={i.id} value={i.id}>{i.source_id}</option>)}</NativeSelect></Field><Field label="原订阅基础路径（以 / 结尾）"><Input name="subscription_base" type="url" placeholder="https://legacy.example.com/assets/" required /></Field><label className="my-4 flex items-center gap-2"><Checkbox name="read_only" required />已停止旧成员业务写入，并完成数据、备份和节点核对</label></FormDialog>}
  </div></Panel>
}
