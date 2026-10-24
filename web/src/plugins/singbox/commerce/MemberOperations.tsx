import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'
import { Checkbox } from '@/components/ui/checkbox'
import { useRef, useState } from 'react'
import { api } from '../../../api'
import { ErrorNotice, Field, FormDialog, Loading } from '../../../components'
import { useAction, useResource } from '../../../hooks'
import { root, money, parseMoney, status, RequestIdentity } from './types'
import type { Member, MemberDetail } from './types'
import { LedgerTable, OrdersTable } from './Records'
import UserPasskeyAccess from '../UserPasskeyAccess'

export default function MemberOperations() {
  const [offset, setOffset] = useState(0), [selected, setSelected] = useState<number | null>(null)
  const members = useResource<{ entries: Member[]; total: number }>(`${root}/members?offset=${offset}`)
  const detail = useResource<MemberDetail>(selected ? `${root}/members/${selected}` : null)
  const [checked, setChecked] = useState<number[]>([]), [bulk, setBulk] = useState(false), [query, setQuery] = useState('')
  const [editing, setEditing] = useState<'profile' | 'adjust' | null>(null)
  const action = useAction(), identity = useRef(new RequestIdentity())
  const refresh = () => { members.reload(); detail.reload() }
  const error = members.error || detail.error
  const member = detail.data?.profile
  const current = member && member.user_id === selected && detail.isCurrent() && !detail.error && !detail.loading
  const submit = (form: FormData) => {
    if (!member || !current) return
    void action.run(async () => {
      if (editing === 'profile') await api(`${root}/members/${member.user_id}`, 'PUT', { name: String(form.get('name')), email: String(form.get('email')) || null, state: form.get('state'), notes: String(form.get('notes')), version: member.version, reason: String(form.get('reason')), release_sponsor: form.get('release_sponsor') === 'on' })
      else { const payload = { delta_cents: parseMoney(form.get('delta')), expected_balance_cents: member.balance_cents, reason: String(form.get('reason')) }; await api(`${root}/members/${member.user_id}/adjust`, 'POST', { ...payload, request_id: identity.current.for([member.user_id, payload]) }) }
    }, () => { setEditing(null); refresh() })
  }
  return <>
    <ErrorNotice message={error || (!editing ? action.error : '')} retry={refresh} />
    <div className="grid min-w-0 items-start gap-5 lg:grid-cols-[minmax(240px,300px)_minmax(0,1fr)]"><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>成员</PanelTitle><Button variant="ghost" size="sm" disabled={!checked.length || !members.isCurrent()} onClick={() => { action.clearError(); setBulk(true) }}>批量修改状态（{checked.length}）</Button><Button variant="ghost" size="sm" onClick={refresh}>刷新</Button></div>
      <div className="p-4 space-y-3"><Input aria-label="搜索当前页成员" placeholder="搜索当前页姓名、邮箱或编号" value={query} onChange={e => setQuery(e.target.value)} /><a className="text-primary underline-offset-4 hover:underline text-sm" href="#/plugins/sing-box/users">创建代理用户 / 分配套餐</a></div>
      {members.loading && !members.data ? <Loading /> : <div className="grid gap-1 p-3 [&_small]:block [&_small]:text-muted-foreground [&_small]:text-xs">{members.data?.entries.filter(m => `${m.name} ${m.email ?? ''} ${m.user_id}`.toLowerCase().includes(query.toLowerCase())).map(m => <div key={m.user_id} className="flex flex-wrap items-center gap-2"><Checkbox  aria-label={`选择成员 ${m.name}`}  disabled={Boolean(editing) || action.busy || bulk || m.state === 'staged'} checked={checked.includes(m.user_id)} onCheckedChange={value => setChecked((value === true) ? [...checked, m.user_id] : checked.filter(id => id !== m.user_id))} /><Button key={m.user_id} variant="ghost" className="min-w-0 flex-1 justify-start h-auto py-2 text-left aria-pressed:bg-muted [&_strong]:block [&_strong]:truncate" aria-pressed={selected === m.user_id} disabled={Boolean(editing) || action.busy} onClick={() => { setSelected(m.user_id); action.clearError() }}><span><strong>{m.name}</strong><small>#{m.user_id} · {status(m.state)} · {money(m.balance_cents)}</small></span></Button></div>)}</div>}
      <div className="flex flex-wrap items-center gap-2 p-4"><Button variant="ghost" size="sm" disabled={offset === 0 || Boolean(editing)} onClick={() => (setChecked([]), setOffset(Math.max(0, offset - 100)))}>上一页</Button><span>{offset + 1} / {members.data?.total ?? 0}</span><Button variant="ghost" size="sm" disabled={offset + 100 >= (members.data?.total ?? 0) || Boolean(editing)} onClick={() => (setChecked([]), setOffset(offset + 100))}>下一页</Button></div>
    </Panel><div>{member && current ? <>
      <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>{member.name}</PanelTitle><span>{status(member.state)}</span></div><div className="p-4 space-y-3"><p>{member.email || '未填写联系邮箱'} · 余额 {money(member.balance_cents)}</p><div className="flex flex-wrap items-center gap-2"><Button variant="ghost" size="sm" onClick={() => setEditing('profile')}>编辑资料与状态</Button><Button variant="ghost" size="sm" onClick={() => { identity.current = new RequestIdentity(); setEditing('adjust') }}>资金调整</Button><Button variant="ghost" size="sm" disabled={action.busy} onClick={() => void action.run(() => api(`${root}/members/${member.user_id}/sessions/revoke`, 'POST'), refresh)}>退出全部成员会话</Button></div></div></Panel>
      <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>钱包流水</PanelTitle></div><LedgerTable entries={detail.data!.history.ledger} /></Panel>
      <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>订单</PanelTitle></div><OrdersTable entries={detail.data!.history.orders} /></Panel>
      <UserPasskeyAccess key={member.user_id} id={member.user_id} userError={() => current ? '' : '请刷新成员资料'} refreshRevision={member.version} />
      {!!detail.data!.history.legacy.length && <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>原系统历史</PanelTitle></div><div className="p-4 space-y-3">{detail.data!.history.legacy.map(r => <details key={`${r.kind}:${r.source_id}`}><summary>{r.kind} · #{r.source_id}</summary><pre className="max-h-90 overflow-auto whitespace-pre-wrap wrap-anywhere text-xs">{JSON.stringify(r.record, null, 2)}</pre></details>)}</div></Panel>}
    </> : <Panel><p className="p-4 space-y-3">{selected ? '正在读取所选成员…' : '选择成员，查看资料、资金及订单。'}</p></Panel>}</div></div>
    {bulk && <FormDialog title="批量修改成员状态" onClose={() => setBulk(false)} busy={action.busy} error={action.error} submitDisabled={!members.isCurrent()} onSubmit={form => { if (!members.isCurrent()) return; const rows = members.data!.entries.filter(m => checked.includes(m.user_id)); void action.run(() => api(`${root}/members/bulk`, 'POST', { members: rows.map(m => ({ id: m.user_id, version: m.version })), state: form.get('state'), reason: form.get('reason') }), () => { setBulk(false); setChecked([]); refresh() }) }}><p>对当前选择的 {checked.length} 位成员执行同一操作。任一成员版本变化，整个批次均不执行。</p><Field label="目标状态"><NativeSelect name="state"><option value="suspended">暂停</option><option value="archived">归档</option><option value="active">恢复可用</option></NativeSelect></Field><Field label="原因"><Textarea name="reason" required maxLength={500} /></Field></FormDialog>}
    {editing && member && <FormDialog title={editing === 'profile' ? '编辑成员' : '资金调整'} onClose={() => setEditing(null)} onSubmit={submit} busy={action.busy} submitDisabled={!current} error={action.error || (!current ? '成员资料已失效，请刷新后操作。' : '')}>
      {editing === 'profile' ? <><Field label="名称"><Input name="name" required maxLength={128} defaultValue={member.name} /></Field><Field label="联系邮箱"><Input type="email" name="email" defaultValue={member.email ?? ''} /></Field><Field label="成员状态"><NativeSelect name="state" defaultValue={member.state}>{['active', 'pending', 'suspended', 'archived', 'staged'].map(v => <option key={v} value={v}>{status(v)}</option>)}</NativeSelect></Field><Field label="备注"><Textarea name="notes" maxLength={4000} defaultValue={member.notes ?? ''} /></Field>{member.sponsor_required && <label className="flex items-center gap-2 text-sm"><input type="checkbox" name="release_sponsor" />解除等待邀请人首购（邀请人已停用或无法完成购买时使用）</label>}<p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">停用和归档会撤销成员会话，并停止订阅及受管节点授权。待迁移成员仅在完成节点、计费和登录核对后激活。</p></> : <><p>当前余额 {money(member.balance_cents)}。调整将保留不可覆盖的资金流水。</p><Field label="调整金额（元，可为负）"><Input name="delta" required inputMode="decimal" placeholder="例如 10.00 或 -5.00" /></Field></>}
      <Field label="操作原因"><Textarea name="reason" required maxLength={500} /></Field>
    </FormDialog>}
  </>
}
