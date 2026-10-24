import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'
import { Checkbox } from '@/components/ui/checkbox'
import { Table, TableHeader, TableBody, TableRow, TableHead, TableCell } from '@/components/ui/table'
import { useState } from 'react'
import { api } from '../../../api'
import { ErrorNotice, Field, FormDialog } from '../../../components'
import { useAction, useResource } from '../../../hooks'
import { money, parseMoney, root } from './types'
import type { Channel, PaymentException, Topup } from './types'
import { TopupsTable } from './Records'

export default function Payments() {
  const channels = useResource<Channel[]>(`${root}/channels`), payments = useResource<{ entries: Topup[]; exceptions: PaymentException[] }>(`${root}/payments`)
  const [editing, setEditing] = useState<Channel | 'new' | null>(null), [resolving, setResolving] = useState<PaymentException | null>(null), action = useAction()
  const refresh = () => { channels.reload(); payments.reload() }, stale = !channels.isCurrent() || !payments.isCurrent() || Boolean(channels.error || payments.error)
  const save = (form: FormData) => {
    if (!editing || stale) return
    void action.run(() => api(editing === 'new' ? `${root}/channels` : `${root}/channels/${editing.id}`, editing === 'new' ? 'POST' : 'PUT', { name: form.get('name'), gateway: form.get('gateway'), merchant: form.get('merchant'), credential_id: form.get('credential'), payment_type: form.get('type'), fixed_fee_cents: parseMoney(form.get('fixed')), fee_basis_points: parseMoney(form.get('percent')), enabled: form.get('enabled') === 'on', version: editing === 'new' ? null : editing.version }), () => { setEditing(null); refresh() })
  }
  return <>
    <ErrorNotice message={channels.error || payments.error} retry={refresh} />
    <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>EPay 支付渠道</PanelTitle><Button size="sm" disabled={stale} onClick={() => setEditing('new')}>新增渠道</Button></div><div className="p-4 space-y-3"><p>先在凭据库添加「外部 API」凭据，内容为支付渠道的 key，再填写凭据编号。已有交易的渠道保留原商户与密钥引用；更换商户时新增渠道。</p><a className="text-primary underline-offset-4 hover:underline text-sm" href="#/system/control-center">打开凭据库</a></div>
      <div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>名称</TableHead><TableHead>手续费</TableHead><TableHead>状态</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{channels.data?.map(c => <TableRow key={c.id}><TableCell>{c.name}</TableCell><TableCell>{money(c.fixed_fee_cents)} + {(c.fee_basis_points / 100).toFixed(2)}%</TableCell><TableCell>{c.enabled ? '启用' : '停用'}</TableCell><TableCell><Button variant="ghost" size="sm" disabled={stale} onClick={() => { action.clearError(); setEditing(c) }}>编辑</Button></TableCell></TableRow>)}</TableBody></Table></div>
    </Panel>
    <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>待核对付款</PanelTitle></div><div className="min-w-0 max-w-full"><Table><TableHeader><TableRow><TableHead>成员</TableHead><TableHead>付款回执</TableHead><TableHead>金额</TableHead><TableHead>原因</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{payments.data?.exceptions.map(e => <TableRow key={e.id}><TableCell>#{e.user_id}</TableCell><TableCell>{e.provider_reference}</TableCell><TableCell>{money(e.amount_cents)}</TableCell><TableCell>{({ duplicate_receipt: '重复付款', late_cancelled: '取消后付款', late_expired: '过期后付款', balance_limit: '余额超限', member_unavailable: '成员不可用' } as Record<string, string>)[e.code] ?? e.code}</TableCell><TableCell><Button variant="ghost" size="sm" disabled={stale} onClick={() => { action.clearError(); setResolving(e) }}>核对处理</Button></TableCell></TableRow>)}</TableBody></Table>{!payments.data?.exceptions.length && <p className="text-muted-foreground py-6 text-sm">没有待核对付款</p>}</div></Panel>
    <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>充值记录</PanelTitle></div><TopupsTable entries={payments.data?.entries ?? []} /></Panel>
    {editing && <FormDialog title="支付渠道" onClose={() => setEditing(null)} onSubmit={save} busy={action.busy} submitDisabled={stale} error={action.error}>
      <Field label="名称"><Input name="name" required maxLength={128} defaultValue={editing === 'new' ? '' : editing.name} /></Field><Field label="完整支付入口（HTTPS，通常以 /submit.php 结尾）"><Input name="gateway" type="url" required defaultValue={editing === 'new' ? '' : editing.gateway} /></Field><Field label="商户编号"><Input name="merchant" required defaultValue={editing === 'new' ? '' : editing.merchant} /></Field><Field label="加密凭据编号"><Input name="credential" required defaultValue={editing === 'new' ? '' : editing.credential_id} /></Field>
      <Field label="支付类型"><NativeSelect name="type" defaultValue={editing === 'new' ? 'alipay' : editing.payment_type}><option value="alipay">支付宝</option><option value="wxpay">微信支付</option><option value="qqpay">QQ 支付</option></NativeSelect></Field><Field label="固定手续费（元）"><Input name="fixed" required defaultValue={editing === 'new' ? '0' : (editing.fixed_fee_cents / 100).toFixed(2)} /></Field><Field label="比例手续费（%）"><Input name="percent" required defaultValue={editing === 'new' ? '0' : (editing.fee_basis_points / 100).toFixed(2)} /></Field><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="enabled"  defaultChecked={editing !== 'new' && editing.enabled} />启用</label>
    </FormDialog>}
    {resolving && <FormDialog title="核对异常付款" onClose={() => setResolving(null)} onSubmit={form => void action.run(() => api(`${root}/exceptions/${resolving.id}/resolve`, 'POST', { action: form.get('action'), reason: form.get('reason') }), () => { setResolving(null); refresh() })} busy={action.busy} submitDisabled={stale} error={action.error}>
      <p>请核对支付平台回执与原交易。登记退款只记录已经完成的原路退款，不会调用支付平台退款。</p><Field label="处理方式"><NativeSelect name="action"><option value="refunded">已在支付平台原路退款</option>{resolving.code !== 'duplicate_receipt' && <option value="credit">核对后计入成员余额</option>}</NativeSelect></Field><Field label="核对依据与原因"><Textarea name="reason" required maxLength={500} /></Field>
    </FormDialog>}
  </>
}
