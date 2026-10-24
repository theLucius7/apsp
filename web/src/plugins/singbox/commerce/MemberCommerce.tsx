import GuideBody from './GuideBody'
import { Panel, PanelTitle } from '@/components/panel'
import { SegmentedNav, SegmentedButton } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from '../../../api'
import { CopyField, ErrorNotice, Field, FormDialog } from '../../../components'
import { useAction } from '../../../hooks'
import { money, parseMoney, paymentFee, RequestIdentity } from './types'
import type { Channel, Content, History, Offer } from './types'
import { InvitationTable, LedgerTable, OrdersTable, TopupsTable } from './Records'
import AccountServices, { MemberMessages } from './AccountServices'
import type { MemberProfile } from './AccountServices'

type View = { enabled: false } | { enabled: true; profile: MemberProfile & { user_id: number; balance_cents: number; sponsor_required: boolean; state: string }; offers: Offer[]; channels: Channel[]; content: Content[]; history: History; invite_settings: { enabled: boolean; price_cents: number }; friends: { user_id: number; name: string; awaiting_sponsor: boolean }[] }
type Dialog = { kind: 'purchase'; offer: Offer; beneficiary?: number } | { kind: 'invite' } | { kind: 'topup' } | { kind: 'revoke'; id: string; prepaid: number }

export default function MemberCommerce({ account }: { account: string }) {
  const base = `/api/plugins/sing-box/portal/${account}/commerce`
  const [data, setData] = useState<View>(), [error, setError] = useState(''), [revision, setRevision] = useState(0), [dialog, setDialog] = useState<Dialog | null>(null), [link, setLink] = useState(''), [notice, setNotice] = useState(''), [tab, setTab] = useState('套餐'), [guideQuery, setGuideQuery] = useState('')
  const action = useAction(), identity = useRef(new RequestIdentity())
  const reload = () => setRevision(v => v + 1)
  useEffect(() => {
    const controller = new AbortController(); let active = true
    void api<View>(base, 'GET', undefined, controller.signal, false).then(v => { if (active) { setData(v); setError('') } }).catch(e => { if (active) { setError(errorMessage(e)); setData(undefined) } })
    const timer = setInterval(reload, 30000)
    return () => { active = false; controller.abort(); clearInterval(timer) }
  }, [base, revision])
  const open = (next: Dialog) => { identity.current = new RequestIdentity(); action.clearError(); setDialog(next) }
  const checkout = async (id: string) => {
    const result = await api<{ action: string; fields: Record<string, string> }>(`${base}/topups/${id}/checkout`, 'POST', undefined, undefined, false)
    const url = new URL(result.action)
    if (url.protocol !== 'https:' || url.username || url.password) throw new Error('支付入口地址无效。')
    const form = document.createElement('form'); form.method = 'POST'; form.action = url.toString()
    for (const [name, value] of Object.entries(result.fields)) { const input = document.createElement('input'); input.type = 'hidden'; input.name = name; input.value = value; form.append(input) }
    document.body.append(form); form.submit(); form.remove()
  }
  const submit = (form: FormData) => {
    if (!dialog || !data?.enabled || error) return
    void action.run(async () => {
      if (dialog.kind === 'purchase') {
        const current = data.offers.find(o => o.id === dialog.offer.id)
        if (current?.version !== dialog.offer.version) throw new Error('套餐已变化，请关闭并重新选择。')
        const payload = { offer_id: dialog.offer.id, offer_version: dialog.offer.version, beneficiary_id: dialog.beneficiary ?? null }
        await api(`${base}/purchase`, 'POST', { ...payload, request_id: identity.current.for(payload) }, undefined, false); setNotice('套餐已交付，设备应用后生效。')
      } else if (dialog.kind === 'invite') {
        const offer = data.offers.find(o => o.id === Number(form.get('offer')))
        const payload = { offer_id: offer?.id ?? null, offer_version: offer?.version ?? null, expected_admission_cents: data.invite_settings.price_cents, quantity: Number(form.get('quantity') || 1) }
        const result = await api<{ url: string | null; urls?: string[]; message?: string }>(`${base}/invitations`, 'POST', { ...payload, request_id: identity.current.for(payload) }, undefined, false)
        setLink(result.urls?.join('\n') || result.url || ''); setNotice(result.message ?? '邀请已购买，请复制链接发给朋友。')
      } else if (dialog.kind === 'topup') {
        const channel = data.channels.find(c => c.id === Number(form.get('channel')))
        if (!channel) throw new Error('支付渠道已变化。')
        const amount = parseMoney(form.get('amount')), payload = { channel_id: channel.id, channel_version: channel.version, amount_cents: amount, expected_fee_cents: paymentFee(amount, channel) }
        const result = await api<{ id: string }>(`${base}/topups`, 'POST', { ...payload, request_id: identity.current.for(payload) }, undefined, false)
        setNotice('充值单已创建，请在记录中继续支付。'); setDialog(null); reload(); await checkout(result.id)
      } else await api(`${base}/invitations/${dialog.id}/revoke`, 'POST', undefined, undefined, false)
    }, () => { setDialog(null); reload() })
  }
  if (data && !data.enabled) return null
  return <section className="space-y-4 [&_summary]:cursor-pointer [&_summary]:font-medium [&_details]:my-3">
    <ErrorNotice message={error || (!dialog ? action.error : '')} retry={reload} />
    {data?.enabled && <>
      <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>成员服务</PanelTitle><strong>余额 {money(data.profile.balance_cents)}</strong></div><div className="p-4 space-y-3"><SegmentedNav aria-label="成员服务">{['套餐', '钱包', '订单', '邀请', '公告与指南', '账户', '通知'].map(t => <SegmentedButton key={t} aria-pressed={tab === t} onClick={() => setTab(t)}>{t}</SegmentedButton>)}</SegmentedNav>
        {notice && <p className="my-3 rounded-lg border border-success/25 bg-success/5 p-3 text-sm" role="status">{notice}</p>}{link && <Field label="邀请链接"><CopyField text={link} label="复制邀请链接" /></Field>}
        {data.profile.sponsor_required && <p className="my-3 rounded-lg border bg-muted/50 p-3 text-sm">首次开通由邀请人代购。完成后可自行充值和续费。<Button variant="ghost" size="sm" disabled={action.busy} onClick={() => void action.run(() => api<{ message: string }>(`${base}/remind-sponsor`, 'POST', undefined, undefined, false), result => setNotice(result.message))}>提醒邀请人</Button></p>}
      </div></Panel>
      {tab === '套餐' && <Panel><div className="grid gap-4 p-4 sm:grid-cols-2 xl:grid-cols-3">{data.offers.map(o => <article key={o.id} className="space-y-3 rounded-xl border p-4"><h3>{o.name}</h3><strong>{money(o.price_cents)}</strong><p>{o.description}</p><p>{o.duration_months ? `${o.duration_months} 个月` : `${o.package_snapshot.duration_days} 天`}</p><Button size="sm" disabled={data.profile.sponsor_required || action.busy || Boolean(error)} onClick={() => open({ kind: 'purchase', offer: o })}>购买 / 续费</Button></article>)}</div></Panel>}
      {tab === '钱包' && <><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>充值</PanelTitle><Button size="sm" disabled={!data.channels.length || data.profile.sponsor_required || action.busy} onClick={() => open({ kind: 'topup' })}>账户充值</Button></div><TopupsTable entries={data.history.topups} actions={entry => entry.state === 'pending' && <div className="flex flex-wrap items-center gap-2"><Button variant="ghost" size="sm" disabled={action.busy} onClick={() => void action.run(() => checkout(entry.id))}>继续支付</Button><Button variant="ghost" size="sm" disabled={action.busy} onClick={() => void action.run(() => api(`${base}/topups/${entry.id}/cancel`, 'POST', undefined, undefined, false), reload)}>取消</Button></div>} /></Panel><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>资金流水</PanelTitle></div><LedgerTable entries={data.history.ledger} /></Panel></>}
      {tab === '订单' && <Panel><OrdersTable entries={data.history.orders} /></Panel>}
      {tab === '邀请' && <><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>我的邀请</PanelTitle><Button size="sm" disabled={!data.invite_settings.enabled || data.profile.sponsor_required || action.busy} onClick={() => open({ kind: 'invite' })}>购买邀请</Button></div><InvitationTable entries={data.history.invitations} actions={entry => entry.state === 'available' && <div className="flex flex-wrap items-center gap-2"><Button variant="ghost" size="sm" disabled={action.busy} onClick={() => void action.run(() => api<{ url: string }>(`${base}/invitations/${entry.id}/rotate`, 'POST', undefined, undefined, false), result => { setLink(result.url); setNotice('新链接已生成，旧链接失效。') })}>重新生成链接</Button><Button variant="ghost" size="sm" disabled={action.busy} onClick={() => open({ kind: 'revoke', id: entry.id, prepaid: entry.snapshot ? entry.prepaid_cents + entry.admission_cents : 0 })}>撤销</Button></div>} /></Panel>
        <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>邀请的朋友</PanelTitle></div><div className="p-4 space-y-3">{data.friends.map(friend => <article key={friend.user_id}><h3>{friend.name}</h3>{friend.awaiting_sponsor ? <div className="flex flex-wrap items-center gap-2">{data.offers.map(o => <Button key={o.id} variant="ghost" size="sm" disabled={action.busy} onClick={() => open({ kind: 'purchase', offer: o, beneficiary: friend.user_id })}>代购 {o.name} · {money(o.price_cents)}</Button>)}</div> : <p>已完成首次开通，后续由本人购买。</p>}</article>)}{!data.friends.length && <p>尚无受邀成员</p>}</div></Panel>
      </>}
      {tab === '账户' && <AccountServices account={account} profile={data.profile} reload={reload} />}{tab === '通知' && <MemberMessages account={account} />}
      {tab === '公告与指南' && <Panel><div className="p-4 space-y-3"><Input aria-label="搜索公告指南" placeholder="搜索标题、分类或内容" value={guideQuery} onChange={e => setGuideQuery(e.target.value)} />{data.content.filter(c => `${c.title} ${c.category} ${c.body}`.toLowerCase().includes(guideQuery.toLowerCase())).map(c => <details key={c.id}><summary>{c.kind === 'notice' ? '公告' : c.category || '指南'} · {c.title}</summary><GuideBody body={c.body} /></details>)}{!data.content.length && <p>暂无内容</p>}</div></Panel>}
      {dialog && <FormDialog title={dialog.kind === 'purchase' ? '确认购买套餐' : dialog.kind === 'invite' ? '购买邀请' : dialog.kind === 'topup' ? '账户充值' : '撤销邀请'} onClose={() => setDialog(null)} onSubmit={submit} busy={action.busy} error={action.error} submitDisabled={Boolean(error)} submitLabel="确认">
        {dialog.kind === 'purchase' && <><p>{dialog.offer.name} · {money(dialog.offer.price_cents)}</p><p>{dialog.beneficiary ? `为成员 #${dialog.beneficiary} 首次开通` : '同套餐续费延长有效期；更换套餐从现在开始，不折抵旧套餐。'}。余额支付后立即交付。历史流量不会清除。</p></>}
        {dialog.kind === 'invite' && <><p>每份准入费 {money(data.invite_settings.price_cents)}。普通邀请的准入费不退。</p><Field label="购买数量"><Input name="quantity" type="number" min={1} max={20} defaultValue={1} required /></Field><Field label="预付套餐"><NativeSelect name="offer"><option value="">普通邀请（朋友注册后由你购买首单）</option>{data.offers.map(o => <option key={o.id} value={o.id}>{o.name} · 另付 {money(o.price_cents)}</option>)}</NativeSelect></Field><p>预付套餐会保留购买时的条件与名额。尚未领取的预付邀请撤销后全额退款，包含准入费。</p></>}
        {dialog.kind === 'topup' && <><Field label="充值金额（元）"><Input name="amount" required inputMode="decimal" defaultValue="100.00" /></Field><Field label="支付渠道"><NativeSelect name="channel">{data.channels.map(c => <option key={c.id} value={c.id}>{c.name} · 手续费 {money(c.fixed_fee_cents)} + {(c.fee_basis_points / 100).toFixed(2)}%</option>)}</NativeSelect></Field><p>页面将跳转至支付渠道。付款结果以已核验的回调为准。</p></>}
        {dialog.kind === 'revoke' && <p>邀请链接将失效。退回 {money(dialog.prepaid)}。未领取的预付邀请包含准入费；普通邀请仅撤销链接。</p>}
      </FormDialog>}
    </>}
  </section>
}
