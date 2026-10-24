import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { useEffect, useState } from 'react'
import { api, errorMessage } from '../../../api'
import { ErrorNotice, Field } from '../../../components'
import { useAction } from '../../../hooks'
import { loginPasskey } from '../../../passkeys'

export type MemberProfile = { email: string | null; email_verified: boolean; has_password: boolean; remind_expire: boolean; remind_traffic: boolean; remind_invitation: boolean }
export default function AccountServices({ account, profile, reload }: { account: string; profile: MemberProfile; reload: () => void }) {
  const base = `/api/plugins/sing-box/portal/${account}`, action = useAction()
  const [email, setEmail] = useState(profile.email ?? ''), [challenge, setChallenge] = useState<{ challenge_id: string; old_code_required: boolean }>(), [notice, setNotice] = useState('')
  async function verify(password: FormDataEntryValue | null) {
    if (password) {
      const value = await api<{ account_id: string }>('/api/plugins/sing-box/commerce-public/account/login', 'POST', { email: profile.email, password }, undefined, false)
      if (value.account_id !== account) throw new Error('登录账户不一致，请重新打开个人入口。')
    } else await loginPasskey(`${base}/login`, false)
  }
  const authentication = <Field label="当前密码（留空则使用 Passkey 验证）"><Input type="password" name="old_password" autoComplete="current-password" maxLength={1024} /></Field>
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>账户与提醒</PanelTitle></div><div className="p-4 space-y-3"><ErrorNotice message={action.error} />{notice && <p role="status">{notice}</p>}<p>联系邮箱：{profile.email ?? '未设置'} · {profile.email_verified ? '已验证' : '未验证'}</p>
    <details><summary>验证或修改邮箱</summary><form onSubmit={e => { e.preventDefault(); const form = new FormData(e.currentTarget); void action.run(async () => { await verify(form.get('old_password')); setChallenge(await api(`${base}/commerce/email/start`, 'POST', { email }, undefined, false)); setNotice('验证码已发送，请在十分钟内完成。') }) }}><Field label="新邮箱"><Input type="email" required value={email} onChange={e => { setEmail(e.target.value); setChallenge(undefined) }} /></Field>{authentication}<Button variant="outline" disabled={action.busy}>验证身份并发送验证码</Button></form>{challenge && <form onSubmit={e => { e.preventDefault(); const form = new FormData(e.currentTarget); void action.run(() => api(`${base}/commerce/email/finish`, 'POST', { challenge_id: challenge.challenge_id, code: form.get('code'), old_code: form.get('old_code') || null }, undefined, false), () => { setChallenge(undefined); setNotice('邮箱已更新，其他会话已退出。'); reload() }) }}><Field label="新邮箱验证码"><Input name="code" required pattern="[0-9]{6}" inputMode="numeric" autoComplete="one-time-code" /></Field>{challenge.old_code_required && <Field label="旧邮箱验证码"><Input name="old_code" required pattern="[0-9]{6}" inputMode="numeric" /></Field>}<Button  disabled={action.busy}>确认邮箱</Button></form>}</details>
    <details><summary>{profile.has_password ? '修改密码' : '设置登录密码'}</summary><form onSubmit={e => { e.preventDefault(); const form = new FormData(e.currentTarget); void action.run(async () => { await verify(form.get('old_password')); await api(`${base}/commerce/password`, 'POST', { password: form.get('password') }, undefined, false) }, () => { setNotice('密码已更新，其他会话已退出。'); reload() }) }}>{authentication}<Field label="新密码"><Input name="password" type="password" required minLength={12} maxLength={1024} autoComplete="new-password" /></Field><Button  disabled={action.busy || (!profile.email_verified && !profile.has_password)}>验证身份并保存</Button>{(!profile.email_verified && !profile.has_password) && <p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">先验证联系邮箱，再启用邮箱密码登录。</p>}</form></details>
    <form key={`${profile.remind_expire}/${profile.remind_traffic}/${profile.remind_invitation}`} onSubmit={e => { e.preventDefault(); const form = new FormData(e.currentTarget); void action.run(() => api(`${base}/commerce/reminders`, 'PUT', { remind_expire: form.get('expire') === 'on', remind_traffic: form.get('traffic') === 'on', remind_invitation: form.get('invitation') === 'on' }, undefined, false), () => { setNotice('提醒设置已保存。'); reload() }) }}><h3>邮件提醒偏好</h3><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="expire"  defaultChecked={profile.remind_expire} />套餐到期提醒</label><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="traffic"  defaultChecked={profile.remind_traffic} />流量达到 80% 提醒</label><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox name="invitation"  defaultChecked={profile.remind_invitation} />受邀成员等待开通提醒</label><Button variant="outline" disabled={action.busy}>保存提醒偏好</Button></form>
  </div></Panel>
}
export function MemberMessages({ account }: { account: string }) {
  type Message = { id: number; message: string; created_at: number; read_at: number | null }
  const [entries, setEntries] = useState<Message[]>([]), [error, setError] = useState(''), [revision, setRevision] = useState(0), action = useAction(), base = `/api/plugins/sing-box/portal/${account}/commerce/messages`
  useEffect(() => { const c = new AbortController(); void api<{ entries: Message[] }>(base, 'GET', undefined, c.signal, false).then(v => { setEntries(v.entries); setError('') }).catch(e => { if (!c.signal.aborted) { setEntries([]); setError(errorMessage(e)) } }); return () => c.abort() }, [base, revision])
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>站内通知</PanelTitle><Button variant="ghost" size="sm" disabled={action.busy || !entries.some(e => !e.read_at)} onClick={() => void action.run(() => api(`${base}/read`, 'POST', { ids: entries.filter(e => !e.read_at).map(e => e.id) }, undefined, false), () => setRevision(v => v + 1))}>全部标为已读</Button></div><div className="p-4 space-y-3"><ErrorNotice message={error || action.error} />{entries.map(e => <p key={e.id}>{e.read_at ? '' : '未读 · '}{new Date(e.created_at * 1000).toLocaleString()} · {e.message}</p>)}{!entries.length && !error && <p>暂无通知</p>}</div></Panel>
}
