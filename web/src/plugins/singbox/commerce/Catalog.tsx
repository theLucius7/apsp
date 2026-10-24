import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'
import { Checkbox } from '@/components/ui/checkbox'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { useState } from 'react'
import { api } from '../../../api'
import { ErrorNotice, Field, FormDialog, DialogPresence } from '../../../components'
import { useAction, useResource } from '../../../hooks'
import { bytes } from '../../../format'
import { root, money, parseMoney } from './types'
import type { Offer } from './types'

export default function Catalog() {
  const offers = useResource<Offer[]>(`${root}/offers`)
  const packages = useResource<{ id: number; name: string }[]>('/api/plugins/sing-box/package-groups')
  const policies = useResource<{ id: number; name: string }[]>('/api/plugins/sing-box/policy-groups')
  const [editing, setEditing] = useState<Offer | 'new' | null>(null), action = useAction()
  const stale = !offers.isCurrent() || !packages.isCurrent() || !policies.isCurrent() || Boolean(offers.error || packages.error || policies.error)
  const refresh = () => { offers.reload(); packages.reload(); policies.reload() }
  const submit = (form: FormData) => {
    if (!editing || stale) return
    void action.run(() => api(editing === 'new' ? `${root}/offers` : `${root}/offers/${editing.id}`, editing === 'new' ? 'POST' : 'PUT', {
      name: form.get('name'), description: form.get('description'), package_group_id: Number(form.get('package')),
      policy_group_ids: form.getAll('policy').map(Number), price_cents: parseMoney(form.get('price')),
      duration_months: form.get('months') ? Number(form.get('months')) : null, capacity: form.get('capacity') ? Number(form.get('capacity')) : null,
      enabled: form.get('enabled') === 'on', renewable: form.get('renewable') === 'on', version: editing === 'new' ? null : editing.version,
    }), () => { setEditing(null); refresh() })
  }
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>销售套餐</PanelTitle><Button size="sm" disabled={stale} onClick={() => { action.clearError(); setEditing('new') }}>新增销售套餐</Button></div><div className="p-4 space-y-3"><p>销售套餐引用现有套餐和策略组。保存时固定配额与周期快照；已有订单和预付邀请保留购买时的条件。</p><a className="text-primary underline-offset-4 hover:underline text-sm" href="#/plugins/sing-box/groups">管理策略与基础套餐</a><ErrorNotice message={offers.error || packages.error || policies.error} retry={refresh} /></div>
    <div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>套餐</TableHead><TableHead>价格</TableHead><TableHead>配额 / 周期</TableHead><TableHead>在售</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{offers.data?.map(o => <TableRow key={o.id}><TableCell>{o.name}<small className="text-muted-foreground block text-xs wrap-anywhere">{o.description}</small></TableCell><TableCell>{money(o.price_cents)}</TableCell><TableCell>{o.package_snapshot.monthly_bytes ? bytes(o.package_snapshot.monthly_bytes) : '不限量'} / {o.duration_months ? `${o.duration_months} 个月` : `${o.package_snapshot.duration_days} 天`}</TableCell><TableCell>{o.enabled ? '在售' : '未上架'}</TableCell><TableCell><Button variant="ghost" size="sm" disabled={stale} onClick={() => { action.clearError(); setEditing(o) }}>编辑</Button></TableCell></TableRow>)}</TableBody></Table></div>
    <DialogPresence>{editing && <FormDialog title={editing === 'new' ? '新增销售套餐' : '编辑销售套餐'} onClose={() => setEditing(null)} onSubmit={submit} busy={action.busy} submitDisabled={stale} error={action.error || (stale ? '请刷新当前套餐及策略数据' : '')}>
      <Field label="销售名称"><Input name="name" required maxLength={128} defaultValue={editing === 'new' ? '' : editing.name} /></Field>
      <Field label="说明"><Textarea name="description" maxLength={4000} defaultValue={editing === 'new' ? '' : editing.description} /></Field>
      <Field label="基础套餐"><NativeSelect name="package" required defaultValue={editing === 'new' ? '' : editing.package_group_id}><option value="">请选择</option>{packages.data?.map(p => <option key={p.id} value={p.id}>{p.name}</option>)}</NativeSelect></Field>
      <fieldset><legend>随套餐授权的策略组</legend>{policies.data?.map(p => <label className="my-3 flex items-center gap-2.5 text-sm" key={p.id}><Checkbox name="policy" value={String(p.id)}  defaultChecked={editing !== 'new' && editing.policy_group_ids.includes(p.id)} />{p.name}</label>)}</fieldset>
      <Field label="售价（元）"><Input name="price" required inputMode="decimal" defaultValue={editing === 'new' ? '' : (editing.price_cents / 100).toFixed(2)} /></Field>
      <Field label="自然月数（留空按基础套餐天数）"><Input name="months" type="number" min={1} max={1200} defaultValue={editing === 'new' ? '' : editing.duration_months ?? ''} /></Field>
      <Field label="共享套餐名额（留空不限）"><Input name="capacity" type="number" min={1} max={1000000} defaultValue={editing === 'new' ? '' : editing.capacity ?? ''} /></Field>
      <label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="enabled"  defaultChecked={editing !== 'new' && editing.enabled} />上架</label><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="renewable"  defaultChecked={editing === 'new' || editing.renewable} />允许续费</label>
    </FormDialog>}</DialogPresence>
  </Panel>
}
