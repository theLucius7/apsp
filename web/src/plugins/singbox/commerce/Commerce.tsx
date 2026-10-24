import { Panel, PanelTitle } from '@/components/panel'
import { SegmentedNav, SegmentedButton } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { UsersIcon, PackageIcon, WalletIcon } from 'lucide-react'
import { useState } from 'react'
import { api } from '../../../api'
import { ErrorNotice, Field, PageHeader, Stat } from '../../../components'
import { useAction, useResource } from '../../../hooks'
import { root, money, parseMoney } from './types'
import type { Settings, Order, Invitation } from './types'
import { OrdersTable, InvitationTable } from './Records'
import MemberOperations from './MemberOperations'
import Catalog from './Catalog'
import Payments from './Payments'
import ContentEditor from './ContentEditor'
import Migration from './Migration'
import MailSettings from './MailSettings'

const tabs = ['总览', '成员', '销售套餐', '订单', '邀请', '充值与付款', '公告指南', '迁移', '成员邮件', '运营设置'] as const
export default function Commerce() {
  const [tab, setTab] = useState<typeof tabs[number]>('总览')
  return <>
    <PageHeader eyebrow="代理服务" title="成员运营" description="管理成员、钱包、销售套餐、订单与邀请，为成员提供统一的自助服务。" />
    <SegmentedNav aria-label="成员运营">{tabs.map(t => <SegmentedButton key={t} aria-pressed={tab === t} onClick={() => setTab(t)}>{t}</SegmentedButton>)}</SegmentedNav>
    <div className="mt-5 grid min-w-0 gap-4">
      {tab === '总览' && <Overview />}{tab === '成员' && <MemberOperations />}{tab === '销售套餐' && <Catalog />}{tab === '订单' && <Orders />}{tab === '邀请' && <Invitations />}{tab === '充值与付款' && <Payments />}{tab === '公告指南' && <ContentEditor />}{tab === '迁移' && <Migration />}{tab === '成员邮件' && <MailSettings />}{tab === '运营设置' && <Configuration />}
    </div>
  </>
}
function Overview() {
  const data = useResource<{ members: number; pending_members: number; balance_cents: string; orders: number; sales_cents: string; open_exceptions: number; invitations: number; ledger_entries: number }>(`${root}/overview`)
  return <><ErrorNotice message={data.error} retry={data.reload} /><div className="mb-4 grid grid-cols-2 gap-3 lg:grid-cols-4"><Stat icon={UsersIcon} label="成员" value={data.data?.members ?? '—'} note={`${data.data?.pending_members ?? 0} 位等待首次开通`} /><Stat icon={WalletIcon} label="账户余额合计" value={data.data ? money(data.data.balance_cents) : '—'} note={`${data.data?.ledger_entries ?? 0} 条资金流水`} /><Stat icon={PackageIcon} label="已交付订单" value={data.data?.orders ?? '—'} note={data.data ? `金额 ${money(data.data.sales_cents)}` : ''} /><Stat icon={WalletIcon} label="待核对付款" value={data.data?.open_exceptions ?? '—'} note="在充值与付款中核对回执" /></div><Panel><div className="p-4 space-y-3"><p>成员沿用代理用户身份。节点、授权、流量与套餐在现有代理服务中管理；钱包和邀请不包含返佣或提现。</p><div className="flex flex-wrap items-center gap-2"><a className="text-primary underline-offset-4 hover:underline text-sm" href="#/plugins/sing-box/users">代理用户与登录开通</a><a className="text-primary underline-offset-4 hover:underline text-sm" href="#/plugins/sing-box/groups">策略与基础套餐</a></div></div></Panel></>
}
function Orders() { const [offset, setOffset] = useState(0); const data = useResource<{ entries: Order[] }>(`${root}/orders?offset=${offset}`); return <Panel><ErrorNotice message={data.error} retry={data.reload} /><OrdersTable entries={data.data?.entries ?? []} /><Paging offset={offset} count={data.data?.entries.length ?? 0} setOffset={setOffset} /></Panel> }
function Invitations() { const [offset, setOffset] = useState(0); const data = useResource<{ entries: Invitation[] }>(`${root}/invitations?offset=${offset}`); return <Panel><ErrorNotice message={data.error} retry={data.reload} /><InvitationTable entries={data.data?.entries ?? []} /><Paging offset={offset} count={data.data?.entries.length ?? 0} setOffset={setOffset} /></Panel> }
function Paging({ offset, count, setOffset }: { offset: number; count: number; setOffset: (n: number) => void }) { return <div className="flex flex-wrap items-center gap-2 p-4"><Button variant="ghost" size="sm" disabled={!offset} onClick={() => setOffset(Math.max(0, offset - 100))}>上一页</Button><span>第 {offset / 100 + 1} 页</span><Button variant="ghost" size="sm" disabled={count < 100} onClick={() => setOffset(offset + 100)}>下一页</Button></div> }
function Configuration() {
  const settings = useResource<Settings>(`${root}/settings`), action = useAction()
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>运营设置</PanelTitle></div><div className="p-4 space-y-3"><ErrorNotice message={settings.error || action.error} retry={settings.reload} />{settings.data && <form key={settings.data.version} onSubmit={e => { e.preventDefault(); const form = new FormData(e.currentTarget); void action.run(() => api(`${root}/settings`, 'PUT', { enabled: form.get('enabled') === 'on', invitations_enabled: form.get('invitations') === 'on', invite_price_cents: parseMoney(form.get('price')), version: settings.data!.version }), settings.reload) }}>
    <label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="enabled"  defaultChecked={settings.data.enabled} />启用成员运营入口</label><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="invitations"  defaultChecked={settings.data.invitations_enabled} />开放成员购买邀请</label><Field label="每份邀请准入费（元）"><Input name="price" required inputMode="decimal" defaultValue={(settings.data.invite_price_cents / 100).toFixed(2)} /></Field><p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">普通邀请准入费不退。预付邀请另收套餐价；尚未领取时撤销会全额退回，包含准入费。普通邀请的首次开通由邀请人购买，之后成员可以自行续费。</p><Button  disabled={action.busy || !settings.isCurrent() || Boolean(settings.error)}>保存运营设置</Button>
  </form>}</div></Panel>
}
