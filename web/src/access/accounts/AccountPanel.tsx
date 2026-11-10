import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { SegmentedButton, SegmentedNav } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useEffect, useRef, useState } from 'react'
import { SearchIcon, TicketIcon, UserRoundCogIcon } from 'lucide-react'
import { api, errorMessage } from '../../api'
import { Badge, CopyField, ErrorNotice, Field } from '../../components'
import type { Tone } from '../../components'
import { time } from '../../format'
import { useAction } from '../../hooks'
import AccountServices, { MemberMessages } from './AccountServices'
import type { MemberProfile } from './AccountServices'
import GuideBody from './GuideBody'
import { invitationStates, RequestIdentity } from './types'
import type { Content, IssuedInvitation } from './types'

type View = { profile: MemberProfile; guides: Content[]; invitations: { enabled: boolean; remaining: number; issued: IssuedInvitation[] } }
type Tab = '邀请' | '公告与指南' | '账号与提醒' | '通知'
const stateTones: Record<IssuedInvitation['state'], Tone> = { available: 'good', claimed: 'neutral', revoked: 'neutral' }

/** The signed-in user's own account: invitations, guides, sign-in settings, notices. */
export default function AccountPanel({ account }: { account: string }) {
  const base = `/api/plugins/sing-box/portal/${account}`
  const [data, setData] = useState<View>(), [error, setError] = useState(''), [revision, setRevision] = useState(0)
  const [tab, setTab] = useState<Tab>('邀请'), [query, setQuery] = useState(''), [created, setCreated] = useState<{ codes: string[]; links: string[] } | null>(null), [notice, setNotice] = useState('')
  const action = useAction(), identity = useRef(new RequestIdentity())
  const reload = () => setRevision(value => value + 1)
  useEffect(() => {
    const controller = new AbortController()
    void api<View>(`${base}/account`, 'GET', undefined, controller.signal, false).then(value => { setData(value); setError('') }).catch(reason => { if (!controller.signal.aborted) setError(errorMessage(reason)) })
    return () => controller.abort()
  }, [base, revision])
  if (!data) return error ? <ErrorNotice message={error} retry={reload} /> : null
  const tabs: Tab[] = [...(data.invitations.enabled || data.invitations.issued.length ? ['邀请' as const] : []), ...(data.guides.length ? ['公告与指南' as const] : []), '账号与提醒', '通知']
  const current = tabs.includes(tab) ? tab : tabs[0]
  const guides = data.guides.filter(entry => `${entry.title} ${entry.category} ${entry.body}`.toLowerCase().includes(query.toLowerCase()))
  const generate = (form: FormData) => {
    const payload = { quantity: Number(form.get('quantity') || 1) }
    void action.run(() => api<{ codes: string[]; links: string[]; message?: string }>(`${base}/invitations`, 'POST', { ...payload, request_id: identity.current.for(payload) }, undefined, false), result => {
      identity.current = new RequestIdentity(); setCreated(result.codes.length ? result : null); setNotice(result.message ?? '邀请码只显示这一次，请复制后发给对方。'); reload()
    })
  }
  return <Panel data-slot="member-account" className="mb-5">
    <PanelHeader><PanelTitle><UserRoundCogIcon className="text-primary size-4" aria-hidden="true" />我的账号</PanelTitle>
      {data.invitations.enabled && <span className="bg-muted text-muted-foreground rounded-full px-2.5 py-1 text-xs tabular-nums">剩余邀请名额 {data.invitations.remaining}</span>}</PanelHeader>
    <div className="p-4 sm:px-5">
      <SegmentedNav aria-label="我的账号" className="mb-4">{tabs.map(entry => <SegmentedButton key={entry} aria-pressed={current === entry} onClick={() => setTab(entry)}>{entry}</SegmentedButton>)}</SegmentedNav>
      <ErrorNotice message={error} retry={reload} />
      {current === '邀请' && <div className="grid gap-4">
        {(action.error || notice) && <div><ErrorNotice message={action.error} />{notice && <Notice tone="success" role="status" className="mb-0">{notice}</Notice>}</div>}
        {data.invitations.enabled ? <form className="grid gap-2" onSubmit={event => { event.preventDefault(); generate(new FormData(event.currentTarget)) }}>
          <div className="flex flex-wrap items-end gap-2">
            <Field label="生成数量" className="mb-0"><NativeSelect name="quantity" defaultValue="1">{[1, 2, 3, 5, 10].filter(value => value <= Math.max(1, data.invitations.remaining)).map(value => <option key={value} value={value}>{value} 个</option>)}</NativeSelect></Field>
            <Button disabled={action.busy || data.invitations.remaining < 1}><TicketIcon aria-hidden="true" />生成邀请码</Button>
          </div>
          <p className="text-muted-foreground text-xs leading-relaxed">每个邀请码用一个名额，对方用它创建自己的账号。没用过的邀请码撤销后名额会退回。</p>
        </form> : <p className="text-muted-foreground text-sm">管理员暂未开放邀请。</p>}
        {created && <ul className="grid gap-3">{created.codes.map((code, index) => <li key={code} data-slot="invitation-ticket" className="border-primary/35 bg-primary/5 relative rounded-xl border border-dashed px-4 pt-3 pb-1">
          <span className="text-muted-foreground text-xs">邀请码 {code}</span>
          <CopyField text={created.links[index]} label="复制邀请链接" className="mt-2 mb-3 bg-card" />
        </li>)}</ul>}
        {data.invitations.issued.length > 0 && <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>邀请码</TableHead><TableHead>状态</TableHead><TableHead className="max-sm:hidden">生成时间</TableHead><TableHead><span className="sr-only">操作</span></TableHead></TableRow></TableHeader><TableBody>{data.invitations.issued.map(entry => <TableRow key={entry.id}>
          <TableCell className="font-mono text-xs">{entry.hint ? `····-${entry.hint}` : '早期邀请链接'}</TableCell>
          <TableCell><Badge tone={stateTones[entry.state]}>{invitationStates[entry.state]}</Badge>{entry.recipient && <small className="text-muted-foreground mt-1 block">{entry.recipient.name}</small>}</TableCell>
          <TableCell className="text-muted-foreground max-sm:hidden">{time(entry.created_at)}</TableCell>
          <TableCell>{entry.state === 'available' && <div className="flex flex-wrap justify-end gap-1"><Button variant="ghost" size="sm" disabled={action.busy} onClick={() => void action.run(() => api<{ code: string; link: string }>(`${base}/invitations/${entry.id}/rotate`, 'POST', undefined, undefined, false), value => { setCreated({ codes: [value.code], links: [value.link] }); setNotice('已换成新的邀请码，旧码不能再用。'); reload() })}>换新码</Button><Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive" disabled={action.busy} onClick={() => void action.run(() => api(`${base}/invitations/${entry.id}/revoke`, 'POST', undefined, undefined, false), () => { setCreated(null); setNotice('邀请码已撤销，名额已退回。'); reload() })}>撤销</Button></div>}</TableCell>
        </TableRow>)}</TableBody></Table></div>}
      </div>}
      {current === '公告与指南' && <div className="grid gap-3">
        <div className="relative"><SearchIcon className="text-muted-foreground pointer-events-none absolute top-1/2 left-3 size-4 -translate-y-1/2" aria-hidden="true" /><Input type="search" aria-label="搜索公告指南" placeholder="搜索标题、分类或内容" value={query} onChange={event => setQuery(event.target.value)} className="pl-9" /></div>
        {guides.map(entry => <details key={entry.id} data-slot="member-guide" data-kind={entry.kind} className="bg-card rounded-xl border open:shadow-soft">
          <summary className="px-4 py-3 text-sm font-medium">{entry.kind === 'notice' ? '公告' : entry.category || '指南'} · {entry.title}</summary>
          <div className="border-t px-4 py-3 text-sm"><GuideBody body={entry.body} /></div>
        </details>)}
        {!guides.length && <p className="text-muted-foreground py-6 text-center text-sm">没有符合条件的公告或指南。</p>}
      </div>}
      {current === '账号与提醒' && <AccountServices account={account} profile={data.profile} reload={reload} />}
      {current === '通知' && <MemberMessages account={account} />}
    </div>
  </Panel>
}
