import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Checkbox } from '@/components/ui/checkbox'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { useState } from 'react'
import { api } from '../../api'
import { ErrorNotice, Field, FormDialog, DialogPresence } from '../../components'
import { useAction, useResource } from '../../hooks'
import { decimal, parseAmount, root } from './types'
import type { Label, Settings } from './types'

export default function Configuration() {
  const settings = useResource<Settings>(`${root}/settings`, 30000), labels = useResource<{ entries: Label[] }>(`${root}/labels`, 30000), action = useAction(), labelAction = useAction()
  const [editing, setEditing] = useState<Label | 'new' | null>(null), [removing, setRemoving] = useState<Label | null>(null)
  const kinds = { category: '分类', payment_method: '付款方式', payer: '付款人' }
  return <><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>统计与提醒设置</PanelTitle></div><div className="p-4 space-y-3"><ErrorNotice message={settings.error || action.error} retry={settings.reload} />{settings.data && <form key={settings.data.version} onSubmit={e => {
    e.preventDefault(); if (!settings.isCurrent()) return; const form = new FormData(e.currentTarget)
    void action.run(() => api(`${root}/settings`, 'PUT', { base_currency: form.get('currency'), budget_units: form.get('budget') ? parseAmount(form.get('budget')) : null, timezone: form.get('timezone'), reminders_enabled: form.get('reminders') === 'on', version: settings.data!.version }), settings.reload)
  }}><Field label="统计本位币"><Input name="currency" pattern="[A-Z]{3}" required maxLength={3} defaultValue={settings.data.base_currency} /></Field><Field label="每月预算（本位币）"><Input name="budget" inputMode="decimal" defaultValue={settings.data.budget_units ? decimal(settings.data.budget_units) : ''} placeholder="不限制" /></Field><Field label="日期时区"><Input name="timezone" required defaultValue={settings.data.timezone} placeholder="Asia/Shanghai" /></Field><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox  name="reminders" defaultChecked={settings.data.reminders_enabled} />启用订阅续费提醒</label><p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">提醒通过已配置的 Telegram / Webhook 发送；每个订阅还须单独勾选提醒。关闭后取消待发送通知，发送中的通知可能已经投递。</p><p><a className="text-primary underline-offset-4 hover:underline text-sm" href="#/system/settings">配置通知渠道</a></p><Button  disabled={action.busy || !settings.isCurrent()}>保存设置</Button></form>}</div></Panel>
    <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>分类、付款方式与付款人</PanelTitle><Button size="sm" disabled={!labels.isCurrent()} onClick={() => { labelAction.clearError(); setEditing('new') }}>新增配置项</Button></div><ErrorNotice message={labels.error} retry={labels.reload} /><div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>类型</TableHead><TableHead>名称</TableHead><TableHead>状态</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{labels.data?.entries.map(l => <TableRow key={l.id}><TableCell>{kinds[l.kind]}</TableCell><TableCell>{l.name}</TableCell><TableCell>{l.enabled ? '可用' : '已停用'}</TableCell><TableCell><Button variant="ghost" size="sm" disabled={!labels.isCurrent()} onClick={() => { labelAction.clearError(); setEditing(l) }}>编辑</Button><Button variant="ghost" size="sm" disabled={!labels.isCurrent()} onClick={() => { labelAction.clearError(); setRemoving(l) }}>删除</Button></TableCell></TableRow>)}</TableBody></Table></div></Panel>
    <DialogPresence>{editing && <FormDialog title={editing === 'new' ? '新增配置项' : '编辑配置项'} onClose={() => setEditing(null)} busy={labelAction.busy} error={labelAction.error} submitDisabled={!labels.isCurrent()} onSubmit={form => {
      if (!labels.isCurrent()) return
      void labelAction.run(() => api(editing === 'new' ? `${root}/labels` : `${root}/labels/${editing.id}`, editing === 'new' ? 'POST' : 'PUT', { kind: editing === 'new' ? form.get('kind') : editing.kind, name: form.get('name'), enabled: form.get('enabled') === 'on' }), () => { setEditing(null); labels.reload() })
    }}><Field label="类型"><NativeSelect name="kind" disabled={editing !== 'new'} defaultValue={editing === 'new' ? 'category' : editing.kind}>{Object.entries(kinds).map(([kind, name]) => <option key={kind} value={kind}>{name}</option>)}</NativeSelect></Field><Field label="名称"><Input name="name" required maxLength={100} defaultValue={editing === 'new' ? '' : editing.name} /></Field><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox  name="enabled" defaultChecked={editing === 'new' || editing.enabled} />可用于新订阅</label></FormDialog>}</DialogPresence>
    <DialogPresence>{removing && <FormDialog title={`删除 ${removing.name}`} onClose={() => setRemoving(null)} busy={labelAction.busy} error={labelAction.error} submitDisabled={!labels.isCurrent()} onSubmit={() => { if (labels.isCurrent()) void labelAction.run(() => api(`${root}/labels/${removing.id}`, 'DELETE'), () => { setRemoving(null); labels.reload() }) }}><p>仅可删除没有订阅引用的配置项；已有引用时可以将其停用。</p></FormDialog>}</DialogPresence>
  </>
}
