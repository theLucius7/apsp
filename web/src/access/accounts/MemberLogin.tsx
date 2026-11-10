import { useState } from 'react'
import { ArrowRightIcon, HashIcon, LockKeyholeIcon, MailIcon } from 'lucide-react'
import { api } from '../../api'
import { ErrorNotice } from '../../components'
import { useAction } from '../../hooks'
import { useSystemTheme } from '@/lib/use-system-theme'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { Notice } from '@/components/panel'
import { AuthField, AuthShell } from '@/components/auth-shell'

const root = '/api/plugins/sing-box/portal-public/account'
const link = 'text-primary font-medium underline-offset-4 hover:underline focus-visible:shadow-focus rounded-sm outline-none'

/** Email and password sign-in for proxy users; the same page sets a first password or recovers one by email. */
export default function MemberLogin() {
  useSystemTheme()
  const [recover, setRecover] = useState(false), [email, setEmail] = useState(''), [challenge, setChallenge] = useState(''), [notice, setNotice] = useState(''), action = useAction()
  const navigate = (result: { account_id: string }) => { if (!/^[0-9a-f-]{36}$/.test(result.account_id)) throw new Error('返回的账户入口无效。'); window.location.hash = `/account/${result.account_id}` }
  const toggle = () => { setRecover(!recover); setChallenge(''); setNotice(''); action.clearError() }
  const send = () => void action.run(() => api<{ challenge_id: string; message: string }>(`${root}/challenge`, 'POST', { email, purpose: 'reset', invitation: null }, undefined, false), value => { setChallenge(value.challenge_id); setNotice(value.message) })
  return <AuthShell slot="member-login" title={recover ? '设置或恢复密码' : '登录我的账户'}
    footer={<>
      <p>{recover ? '恢复已有密码会同时移除已登记的 Passkey，之后可重新添加；首次设置密码不影响 Passkey。无法收信时可联系管理员恢复个人入口。' : '已有 Passkey 可从保存的个人入口登录。'}</p>
      <p className="mt-2">新成员须先领取邀请。<a className={link} href="#/join">用邀请码创建账号</a></p>
    </>}>
    <ErrorNotice message={action.error} />
    {notice && <Notice tone="success" role="status">{notice}</Notice>}
    <form className="grid gap-4" onSubmit={e => {
      e.preventDefault(); const form = new FormData(e.currentTarget)
      void action.run(() => api<{ account_id: string }>(`${root}/${recover ? 'reset' : 'login'}`, 'POST', recover ? { challenge_id: challenge, code: form.get('code'), password: form.get('password') } : { email, password: form.get('password') }, undefined, false), navigate)
    }}>
      <AuthField id="member-email" label="邮箱" icon={MailIcon}>
        <Input id="member-email" type="email" required maxLength={254} autoComplete="username" value={email} onChange={e => { setEmail(e.target.value); setChallenge(''); setNotice('') }} className="h-11 pl-10" />
      </AuthField>
      {recover && <AuthField id="member-code" label="六位验证码" icon={HashIcon}
        action={<Button type="button" variant="link" size="sm" className="h-auto px-0" disabled={action.busy || !email} onClick={send}>{challenge ? '重新发送' : '发送邮箱验证码'}</Button>}>
        <Input id="member-code" name="code" required pattern="[0-9]{6}" inputMode="numeric" autoComplete="one-time-code" maxLength={6} className="h-11 pl-10 tracking-[0.25em] tabular-nums" />
      </AuthField>}
      <AuthField id="member-password" label={recover ? '新密码（至少 12 位）' : '密码'} icon={LockKeyholeIcon}>
        <Input id="member-password" name="password" type="password" required minLength={recover ? 12 : 1} maxLength={1024} autoComplete={recover ? 'new-password' : 'current-password'} className="h-11 pl-10" />
      </AuthField>
      <Button size="lg" className="mt-1 h-11 w-full text-sm" disabled={action.busy || (recover && !challenge)}>
        {action.busy ? <Spinner role="presentation" aria-hidden="true" aria-label={undefined} /> : null}{recover ? '验证并设置密码' : '登录'}{!action.busy && <ArrowRightIcon aria-hidden="true" className="transition-transform group-hover/button:translate-x-0.5" />}
      </Button>
    </form>
    <p className="mt-4 text-center text-[13px]"><button type="button" className={link} onClick={toggle}>{recover ? '返回密码登录' : '首次设置密码 / 忘记密码'}</button></p>
  </AuthShell>
}
