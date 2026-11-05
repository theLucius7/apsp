import { Panel, PanelTitle } from '@/components/panel'
import { SegmentedButton, SegmentedNav } from '@/components/segmented'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { useEffect, useRef, useState } from 'react'
import { api, errorMessage } from '../../../api'
import { CopyField, ErrorNotice, Field } from '../../../components'
import { time } from '../../../format'
import { useAction } from '../../../hooks'
import AccountServices, { MemberMessages } from './AccountServices'
import type { MemberProfile } from './AccountServices'
import GuideBody from './GuideBody'
import { invitationStates, RequestIdentity } from './types'
import type { Content, IssuedInvitation } from './types'

type View = { profile: MemberProfile; guides: Content[]; invitations: { enabled: boolean; remaining: number; issued: IssuedInvitation[] } }
type Tab = '邀请' | '公告与指南' | '账号与提醒' | '通知'

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
  const generate = (form: FormData) => {
    const payload = { quantity: Number(form.get('quantity') || 1) }
    void action.run(() => api<{ codes: string[]; links: string[]; message?: string }>(`${base}/invitations`, 'POST', { ...payload, request_id: identity.current.for(payload) }, undefined, false), result => {
      identity.current = new RequestIdentity(); setCreated(result.codes.length ? result : null); setNotice(result.message ?? '邀请码只显示这一次，请复制后发给对方。'); reload()
    })
  }
  return <section className="mb-5">
    <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>我的账号</PanelTitle>{data.invitations.enabled && <span className="text-muted-foreground text-sm tabular-nums">剩余邀请名额 {data.invitations.remaining}</span>}</div>
      <div className="space-y-3 p-4"><SegmentedNav aria-label="我的账号">{tabs.map(entry => <SegmentedButton key={entry} aria-pressed={current === entry} onClick={() => setTab(entry)}>{entry}</SegmentedButton>)}</SegmentedNav>
        <ErrorNotice message={error || action.error} retry={reload} />{notice && <p className="border-success/25 bg-success/5 rounded-lg border p-3 text-sm" role="status">{notice}</p>}
        {current === '邀请' && <>
          {data.invitations.enabled ? <form className="flex flex-wrap items-end gap-2" onSubmit={event => { event.preventDefault(); generate(new FormData(event.currentTarget)) }}>
            <Field label="生成数量"><NativeSelect name="quantity" defaultValue="1">{[1, 2, 3, 5, 10].filter(value => value <= Math.max(1, data.invitations.remaining)).map(value => <option key={value} value={value}>{value} 个</option>)}</NativeSelect></Field>
            <Button disabled={action.busy || data.invitations.remaining < 1}>生成邀请码</Button>
            <p className="text-muted-foreground w-full text-xs">每个邀请码用一个名额，对方用它创建自己的账号。没用过的邀请码撤销后名额会退回。</p>
          </form> : <p className="text-muted-foreground text-sm">管理员暂未开放邀请。</p>}
          {created && <div className="space-y-2">{created.codes.map((code, index) => <Field key={code} label={`邀请码 ${code}`}><CopyField text={created.links[index]} label="复制邀请链接" /></Field>)}</div>}
          {data.invitations.issued.length > 0 && <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>邀请码</TableHead><TableHead>状态</TableHead><TableHead>生成时间</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{data.invitations.issued.map(entry => <TableRow key={entry.id}>
            <TableCell className="font-mono text-xs">{entry.hint ? `····-${entry.hint}` : '早期邀请链接'}</TableCell>
            <TableCell>{invitationStates[entry.state]}{entry.recipient && <small className="text-muted-foreground block">{entry.recipient.name}</small>}</TableCell>
            <TableCell>{time(entry.created_at)}</TableCell>
            <TableCell>{entry.state === 'available' && <div className="flex flex-wrap gap-1"><Button variant="ghost" size="sm" disabled={action.busy} onClick={() => void action.run(() => api<{ code: string; link: string }>(`${base}/invitations/${entry.id}/rotate`, 'POST', undefined, undefined, false), value => { setCreated({ codes: [value.code], links: [value.link] }); setNotice('已换成新的邀请码，旧码不能再用。'); reload() })}>换新码</Button><Button variant="ghost" size="sm" className="text-destructive" disabled={action.busy} onClick={() => void action.run(() => api(`${base}/invitations/${entry.id}/revoke`, 'POST', undefined, undefined, false), () => { setCreated(null); setNotice('邀请码已撤销，名额已退回。'); reload() })}>撤销</Button></div>}</TableCell>
          </TableRow>)}</TableBody></Table></div>}
        </>}
        {current === '公告与指南' && <><Input aria-label="搜索公告指南" placeholder="搜索标题、分类或内容" value={query} onChange={event => setQuery(event.target.value)} />{data.guides.filter(entry => `${entry.title} ${entry.category} ${entry.body}`.toLowerCase().includes(query.toLowerCase())).map(entry => <details key={entry.id} className="my-3"><summary className="cursor-pointer font-medium">{entry.kind === 'notice' ? '公告' : entry.category || '指南'} · {entry.title}</summary><GuideBody body={entry.body} /></details>)}</>}
      </div></Panel>
    {current === '账号与提醒' && <AccountServices account={account} profile={data.profile} reload={reload} />}
    {current === '通知' && <MemberMessages account={account} />}
  </section>
}
