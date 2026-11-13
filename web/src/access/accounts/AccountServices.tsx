import { Notice } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { useEffect, useState } from 'react'
import type { ReactNode } from 'react'
import { BellIcon, CheckCheckIcon, InboxIcon, MailIcon } from 'lucide-react'
import { api, errorMessage } from '../../api'
import { Badge, ErrorNotice, Field } from '../../components'
import { ago, time } from '../../format'
import { useAction } from '../../hooks'
import { loginPasskey } from '../../governance/passkeys'
import { onOwnAddress } from './types'

export type MemberProfile = { email: string | null; email_verified: boolean; has_password: boolean; remind_expire: boolean; remind_traffic: boolean }

function Section({ title, children }: { title: string; children: ReactNode }) {
  return <details className="bg-card rounded-xl border open:shadow-soft">
    <summary className="px-4 py-3 text-sm font-medium">{title}</summary>
    <div className="border-t px-4 py-4">{children}</div>
  </details>
}

/** Contact email, password and reminder preferences; changes are confirmed with the password or a passkey. */
export default function AccountServices({ account, profile, reload }: { account: string; profile: MemberProfile; reload: () => void }) {
  const base = `/api/access/portal/${account}`, action = useAction()
  const [email, setEmail] = useState(profile.email ?? ''), [challenge, setChallenge] = useState<{ challenge_id: string; old_code_required: boolean }>(), [notice, setNotice] = useState('')
  async function verify(password: FormDataEntryValue | null) {
    if (password) {
      const value = await api<{ account_id: string }>('/api/access/portal-public/account/login', 'POST', { email: profile.email, password }, undefined, false)
      if (value.account_id !== account) throw new Error('登录账户不一致，请重新打开个人入口。')
    } else await loginPasskey(`${base}/login`, false)
  }
  // Password checks go through the panel's address; a user's own address verifies with a Passkey.
  const authentication = onOwnAddress() ? <p className="text-muted-foreground mb-4 text-xs">提交时用 Passkey 验证身份。</p>
    : <Field label="当前密码（留空则使用 Passkey 验证）"><Input type="password" name="old_password" autoComplete="current-password" maxLength={1024} /></Field>
  return <div data-slot="account-services" className="grid gap-3">
    {(action.error || notice) && <div><ErrorNotice message={action.error} />{notice && <Notice tone="success" role="status" className="mb-0">{notice}</Notice>}</div>}
    <div className="flex min-w-0 items-center gap-3 rounded-xl border px-4 py-3">
      <MailIcon className="text-muted-foreground size-4 shrink-0" aria-hidden="true" />
      <p className="min-w-0 flex-1 text-sm wrap-anywhere">联系邮箱：{profile.email ?? '未设置'} · {profile.email_verified ? '已验证' : '未验证'}</p>
      {profile.email && <Badge tone={profile.email_verified ? 'good' : 'warm'} className="max-sm:hidden">{profile.email_verified ? '可收信' : '待验证'}</Badge>}
    </div>
    <Section title="验证或修改邮箱">
      <form onSubmit={e => { e.preventDefault(); const form = new FormData(e.currentTarget); void action.run(async () => { await verify(form.get('old_password')); setChallenge(await api(`${base}/account/email/start`, 'POST', { email }, undefined, false)); setNotice('验证码已发送，请在十分钟内完成。') }) }}>
        <Field label="新邮箱"><Input type="email" required value={email} onChange={e => { setEmail(e.target.value); setChallenge(undefined) }} /></Field>{authentication}
        <Button variant="outline" disabled={action.busy}>验证身份并发送验证码</Button>
      </form>
      {challenge && <form className="mt-5 border-t pt-4" onSubmit={e => { e.preventDefault(); const form = new FormData(e.currentTarget); void action.run(() => api(`${base}/account/email/finish`, 'POST', { challenge_id: challenge.challenge_id, code: form.get('code'), old_code: form.get('old_code') || null }, undefined, false), () => { setChallenge(undefined); setNotice('邮箱已更新，其他会话已退出。'); reload() }) }}>
        <Field label="新邮箱验证码"><Input name="code" required pattern="[0-9]{6}" inputMode="numeric" autoComplete="one-time-code" /></Field>
        {challenge.old_code_required && <Field label="旧邮箱验证码"><Input name="old_code" required pattern="[0-9]{6}" inputMode="numeric" /></Field>}
        <Button disabled={action.busy}>确认邮箱</Button>
      </form>}
    </Section>
    <Section title={profile.has_password ? '修改密码' : '设置登录密码'}>
      <form onSubmit={e => { e.preventDefault(); const form = new FormData(e.currentTarget); void action.run(async () => { await verify(form.get('old_password')); await api(`${base}/account/password`, 'POST', { password: form.get('password') }, undefined, false) }, () => { setNotice('密码已更新，其他会话已退出。'); reload() }) }}>
        {authentication}<Field label="新密码"><Input name="password" type="password" required minLength={12} maxLength={1024} autoComplete="new-password" /></Field>
        <Button disabled={action.busy || (!profile.email_verified && !profile.has_password)}>验证身份并保存</Button>
        {(!profile.email_verified && !profile.has_password) && <p className="text-muted-foreground mt-2 text-xs leading-relaxed">先验证联系邮箱，再启用邮箱密码登录。</p>}
      </form>
    </Section>
    <form key={`${profile.remind_expire}/${profile.remind_traffic}`} className="rounded-xl border px-4 py-3.5" onSubmit={e => { e.preventDefault(); const form = new FormData(e.currentTarget); void action.run(() => api(`${base}/account/reminders`, 'PUT', { remind_expire: form.get('expire') === 'on', remind_traffic: form.get('traffic') === 'on' }, undefined, false), () => { setNotice('提醒设置已保存。'); reload() }) }}>
      <h3 className="flex items-center gap-2 text-sm font-medium"><BellIcon className="text-muted-foreground size-4" aria-hidden="true" />邮件提醒偏好</h3>
      <div className="my-3 grid gap-2.5 sm:grid-cols-2">
        <label className="flex items-center gap-2.5 text-sm"><Checkbox name="expire" defaultChecked={profile.remind_expire} />套餐到期提醒</label>
        <label className="flex items-center gap-2.5 text-sm"><Checkbox name="traffic" defaultChecked={profile.remind_traffic} />流量达到 80% 提醒</label>
      </div>
      <Button variant="outline" size="sm" disabled={action.busy}>保存提醒偏好</Button>
    </form>
  </div>
}

/** Messages the panel sent this user; unread ones are marked until they are read. */
export function MemberMessages({ account }: { account: string }) {
  type Message = { id: number; message: string; created_at: number; read_at: number | null }
  const [entries, setEntries] = useState<Message[]>([]), [error, setError] = useState(''), [revision, setRevision] = useState(0), action = useAction(), base = `/api/access/portal/${account}/account/messages`
  useEffect(() => { const c = new AbortController(); void api<{ entries: Message[] }>(base, 'GET', undefined, c.signal, false).then(v => { setEntries(v.entries); setError('') }).catch(e => { if (!c.signal.aborted) { setEntries([]); setError(errorMessage(e)) } }); return () => c.abort() }, [base, revision])
  const unread = entries.filter(e => !e.read_at)
  return <div data-slot="member-messages" className="grid gap-3">
    <ErrorNotice message={error || action.error} />
    <div className="flex items-center justify-between gap-3">
      <p className="text-muted-foreground text-sm">{unread.length ? `${unread.length} 条未读` : '没有未读通知'}</p>
      <Button variant="ghost" size="sm" disabled={action.busy || !unread.length} onClick={() => void action.run(() => api(`${base}/read`, 'POST', { ids: unread.map(e => e.id) }, undefined, false), () => setRevision(v => v + 1))}><CheckCheckIcon aria-hidden="true" />全部标为已读</Button>
    </div>
    {entries.length > 0 && <ul className="divide-y rounded-xl border">{entries.map(e => <li key={e.id} data-slot="member-message" data-unread={e.read_at ? undefined : ''} className="flex min-w-0 items-start gap-3 px-4 py-3">
      <span className={e.read_at ? 'bg-border mt-1.5 size-2 shrink-0 rounded-full' : 'bg-primary mt-1.5 size-2 shrink-0 rounded-full shadow-[0_0_0_3px_color-mix(in_oklch,var(--primary)_18%,transparent)]'} aria-hidden="true" />
      <div className="min-w-0 flex-1">
        <p className={e.read_at ? 'text-sm wrap-anywhere' : 'text-sm font-medium wrap-anywhere'}>{!e.read_at && <span className="sr-only">未读：</span>}{e.message}</p>
        <p className="text-muted-foreground mt-0.5 text-xs" title={time(e.created_at)}>{ago(e.created_at)}</p>
      </div>
    </li>)}</ul>}
    {!entries.length && !error && <div className="text-muted-foreground grid place-items-center gap-2 rounded-xl border border-dashed py-8 text-sm"><InboxIcon className="size-5" aria-hidden="true" />暂无通知</div>}
  </div>
}
