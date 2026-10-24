import { Panel } from '@/components/panel'
import { useSystemTheme } from '@/lib/use-system-theme'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { useState } from 'react'
import { api } from '../../../api'
import { Brand, ErrorNotice, Field } from '../../../components'
import { useAction } from '../../../hooks'

const root = '/api/plugins/sing-box/commerce-public/account'
export default function MemberLogin() {
  useSystemTheme()
  const [recover, setRecover] = useState(false), [email, setEmail] = useState(''), [challenge, setChallenge] = useState(''), [notice, setNotice] = useState(''), action = useAction()
  const navigate = (result: { account_id: string }) => { if (!/^[0-9a-f-]{36}$/.test(result.account_id)) throw new Error('返回的账户入口无效。'); window.location.hash = `/plugins/sing-box/account/${result.account_id}` }
  return <main className="mx-auto max-w-[880px] px-6 pt-7 pb-16 max-sm:px-3 max-sm:py-5 [&>header]:mb-7 [&>header]:flex [&>header]:items-center [&>header]:gap-4"><header><Brand /><span>成员登录</span></header><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><h1>{recover ? '设置或恢复密码' : '邮箱密码登录'}</h1></div><div className="p-4 space-y-3"><ErrorNotice message={action.error} />{notice && <p role="status">{notice}</p>}<form onSubmit={e => {
    e.preventDefault(); const form = new FormData(e.currentTarget)
    void action.run(() => api<{ account_id: string }>(`${root}/${recover ? 'reset' : 'login'}`, 'POST', recover ? { challenge_id: challenge, code: form.get('code'), password: form.get('password') } : { email, password: form.get('password') }, undefined, false), navigate)
  }}><Field label="邮箱"><Input type="email" required maxLength={254} autoComplete="username" value={email} onChange={e => { setEmail(e.target.value); setChallenge(''); setNotice('') }} /></Field>
    {recover && <><Button type="button" variant="ghost" size="sm" disabled={action.busy || !email} onClick={() => void action.run(() => api<{ challenge_id: string; message: string }>(`${root}/challenge`, 'POST', { email, purpose: 'reset', invitation: null }, undefined, false), value => { setChallenge(value.challenge_id); setNotice(value.message) })}>发送邮箱验证码</Button><Field label="六位验证码"><Input name="code" required pattern="[0-9]{6}" inputMode="numeric" autoComplete="one-time-code" /></Field></>}
    <Field label={recover ? '新密码（至少 12 位）' : '密码'}><Input name="password" type="password" required minLength={recover ? 12 : 1} maxLength={1024} autoComplete={recover ? 'new-password' : 'current-password'} /></Field><Button  disabled={action.busy || (recover && !challenge)}>{recover ? '验证并设置密码' : '登录'}</Button></form><p><Button variant="ghost" size="sm" onClick={() => { setRecover(!recover); setChallenge(''); setNotice(''); action.clearError() }}>{recover ? '返回密码登录' : '首次设置密码 / 忘记密码'}</Button></p><p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">已有 Passkey 可从保存的个人入口登录。新成员须先领取邀请；无法收信时可联系管理员恢复个人入口。</p></div></Panel></main>
}
