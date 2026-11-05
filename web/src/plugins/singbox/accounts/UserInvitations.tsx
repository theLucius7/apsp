import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { api } from '../../../api'
import { ErrorNotice, Field, Loading } from '../../../components'
import { time } from '../../../format'
import { settingsPoll, useAction, useResource } from '../../../hooks'
import { invitationStates, root } from './types'
import type { IssuedInvitation } from './types'

type View = { remaining: number; events: { id: number; delta: number; remaining: number; reason: string; detail: { reason?: string; balance_cents?: number; price_cents?: number }; created_at: number }[]; issued: IssuedInvitation[]; invited_by: { id: number; name: string } | null }
const reasons: Record<string, string> = { balance_conversion: '余额换算', administrator: '管理员调整', invitation_created: '生成邀请码', invitation_revoked: '撤销邀请码', invited_account: '受邀账号自带' }

/** One user's invitation quota, the codes they handed out and who invited them. */
export default function UserInvitations({ userId, active }: { userId: number; active: boolean }) {
  // Loads when first opened; afterwards a hidden tab keeps its data and stops polling.
  const [opened, setOpened] = useState(active)
  useEffect(() => { if (active) setOpened(true) }, [active])
  const resource = useResource<View>(opened ? `${root}/users/${userId}/invitations` : null, active ? settingsPoll : 0)
  const action = useAction()
  const data = resource.data
  return <Panel>
    <PanelHeader className="min-h-10 py-1.5"><PanelTitle>邀请</PanelTitle>{data && <span className="text-muted-foreground text-xs tabular-nums">剩余名额 {data.remaining}{data.invited_by ? ` · 由 ${data.invited_by.name} 邀请` : ''}</span>}</PanelHeader>
    <div className="space-y-3 p-3.5">
      <ErrorNotice message={resource.error || action.error} retry={resource.reload} />
      {!data ? <Loading variant="panel" rows={2} /> : <>
        <form className="flex flex-wrap items-end gap-2" onSubmit={event => {
          event.preventDefault(); const form = new FormData(event.currentTarget)
          void action.run(() => api(`${root}/users/${userId}/invitations/quota`, 'POST', { delta: Number(form.get('delta')), reason: String(form.get('reason') ?? '') }), () => { (event.target as HTMLFormElement).reset(); resource.reload() })
        }}>
          <Field label="增减名额" hint="正数增加，负数减少"><Input name="delta" type="number" required min={-100000} max={100000} step={1} className="w-32" /></Field>
          <Field label="备注"><Input name="reason" maxLength={200} placeholder="可选" /></Field>
          <Button variant="outline" disabled={action.busy}>调整</Button>
        </form>
        {data.issued.length > 0 && <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>邀请码</TableHead><TableHead>状态</TableHead><TableHead>生成时间</TableHead></TableRow></TableHeader><TableBody>{data.issued.map(entry => <TableRow key={entry.id}><TableCell className="font-mono text-xs">{entry.hint ? `····-${entry.hint}` : '早期邀请链接'}</TableCell><TableCell>{invitationStates[entry.state]}{entry.recipient && <small className="text-muted-foreground block">{entry.recipient.name}</small>}</TableCell><TableCell>{time(entry.created_at)}</TableCell></TableRow>)}</TableBody></Table></div>}
        {data.events.length > 0 && <details><summary className="cursor-pointer text-sm font-medium">名额变动记录</summary><ul className="mt-2 space-y-1 text-xs">{data.events.map(entry => <li key={entry.id} className="tabular-nums">{time(entry.created_at)} · {reasons[entry.reason] ?? entry.reason} {entry.delta > 0 ? `+${entry.delta}` : entry.delta}，剩余 {entry.remaining}{entry.reason === 'balance_conversion' && entry.detail.balance_cents !== undefined ? `（余额 ¥${(entry.detail.balance_cents / 100).toFixed(2)}，每份 ¥${((entry.detail.price_cents ?? 0) / 100).toFixed(2)}）` : ''}{entry.detail.reason ? ` · ${entry.detail.reason}` : ''}</li>)}</ul></details>}
      </>}
    </div>
  </Panel>
}
