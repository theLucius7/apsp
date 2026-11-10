import { useEffect, useRef, useState } from 'react'
import { ArrowRightIcon, HashIcon, MailIcon, TicketCheckIcon, TicketIcon, UserRoundIcon } from 'lucide-react'
import { api, errorMessage } from '../../api'
import { ErrorNotice, Loading } from '../../components'
import { useAction } from '../../hooks'
import { publicRoot, RequestIdentity } from './types'
import { useSystemTheme } from '@/lib/use-system-theme'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { Notice } from '@/components/panel'
import { AuthField, AuthShell, AuthSteps } from '@/components/auth-shell'

type Inspection = { available: boolean; package_name: string | null; email_verification_required: boolean; configuration: { enabled: boolean; reason?: string } }
const link = 'text-primary font-medium underline-offset-4 hover:underline focus-visible:shadow-focus rounded-sm outline-none'

/** Redeems an invitation code: the visitor creates their own account with it, then binds a passkey. */
export default function Join({ token }: { token?: string }) {
  useSystemTheme()
  const [code, setCode] = useState(token ?? '')
  const [checked, setChecked] = useState(token ?? '')
  const [data, setData] = useState<Inspection>(), [error, setError] = useState('')
  const [email, setEmail] = useState(''), [challenge, setChallenge] = useState(''), [notice, setNotice] = useState('')
  const action = useAction(), identity = useRef(new RequestIdentity())
  useEffect(() => {
    if (!checked) return
    const c = new AbortController(); setData(undefined); setError(''); setChallenge('')
    void api<Inspection>(`${publicRoot}/invitation`, 'POST', { token: checked }, c.signal, false).then(setData).catch(e => { if (!c.signal.aborted) setError(errorMessage(e).includes('不存在') || errorMessage(e).includes('404') ? '邀请码不存在，请核对后重新输入。' : errorMessage(e)) })
    return () => c.abort()
  }, [checked])
  const change = () => { setChecked(''); setData(undefined); setNotice(''); action.clearError() }
  const entering = !checked || Boolean(error)
  return <AuthShell slot="member-join" title="用邀请码创建账号"
    steps={<AuthSteps steps={['核对邀请码', '填写信息', '绑定 Passkey']} current={!entering && data?.available ? 1 : 0} />}
    footer={<>已有账号？<a className={link} href="#/member">邮箱密码登录</a></>}>
    <ErrorNotice message={error || action.error} />
    {notice && <Notice tone="success" role="status">{notice}</Notice>}
    {entering ? <form className="grid gap-4" onSubmit={e => { e.preventDefault(); setChecked(code.trim()) }}>
      <AuthField id="join-code" label="邀请码" icon={TicketIcon}>
        <Input id="join-code" required autoComplete="off" autoCapitalize="characters" spellCheck={false} placeholder="XXXX-XXXX-XXXX-XXXX" value={code} onChange={e => setCode(e.target.value)} className="h-11 pl-10 font-mono text-[13px] tracking-wider" />
      </AuthField>
      <Button size="lg" className="h-11 w-full text-sm" disabled={!code.trim()}>下一步<ArrowRightIcon aria-hidden="true" className="transition-transform group-hover/button:translate-x-0.5" /></Button>
    </form>
      : !data ? <Loading /> : data.available ? <form className="grid gap-4" onSubmit={e => {
        e.preventDefault(); const form = new FormData(e.currentTarget), payload = { token: checked, name: form.get('name'), email }
        void action.run(() => api<{ url: string }>(`${publicRoot}/claim`, 'POST', { ...payload, request_id: identity.current.for(payload), challenge_id: challenge || null, code: form.get('code') || null }, undefined, false), result => { const url = new URL(result.url); if (url.origin !== window.location.origin) throw new Error('请从面板配置的公开地址重新打开邀请。'); window.location.assign(url.toString()) })
      }}>
        <div data-slot="invitation-summary" className="bg-success/5 border-success/25 flex items-center gap-3 rounded-xl border px-3.5 py-3">
          <TicketCheckIcon className="text-success size-5 shrink-0" aria-hidden="true" />
          <div className="min-w-0 flex-1">
            <p className="text-sm leading-snug">{data.package_name ? `创建后即可使用「${data.package_name}」。` : '创建后，管理员为你分配套餐即可使用。'}</p>
            <p className="text-muted-foreground mt-0.5 truncate font-mono text-xs">{checked}</p>
          </div>
          <Button type="button" variant="ghost" size="sm" disabled={action.busy} onClick={change}>更换</Button>
        </div>
        <AuthField id="join-name" label="名称" icon={UserRoundIcon}>
          <Input id="join-name" name="name" required maxLength={128} autoComplete="nickname" className="h-11 pl-10" />
        </AuthField>
        <AuthField id="join-email" label="联系邮箱" icon={MailIcon}>
          <Input id="join-email" name="email" required type="email" maxLength={254} autoComplete="email" value={email} onChange={e => { setEmail(e.target.value); setChallenge(''); setNotice('') }} className="h-11 pl-10" />
        </AuthField>
        {data.email_verification_required && <AuthField id="join-mail-code" label="六位验证码" icon={HashIcon}
          action={<Button type="button" variant="link" size="sm" className="h-auto px-0" disabled={action.busy || !email} onClick={() => void action.run(() => api<{ challenge_id: string; message: string }>(`${publicRoot}/account/challenge`, 'POST', { email, purpose: 'invite', invitation: checked }, undefined, false), value => { setChallenge(value.challenge_id); setNotice(value.message) })}>{challenge ? '重新发送' : '发送邮箱验证码'}</Button>}>
          <Input id="join-mail-code" name="code" required pattern="[0-9]{6}" inputMode="numeric" autoComplete="one-time-code" maxLength={6} className="h-11 pl-10 tracking-[0.25em] tabular-nums" />
        </AuthField>}
        {!data.configuration.enabled && <Notice className="mb-0">{data.configuration.reason}</Notice>}
        <Button size="lg" className="mt-1 h-11 w-full text-sm" disabled={action.busy || !data.configuration.enabled || (data.email_verification_required && !challenge)}>
          {action.busy && <Spinner role="presentation" aria-hidden="true" aria-label={undefined} />}创建账号
        </Button>
        <p className="text-muted-foreground -mt-1 text-center text-xs">创建后绑定 Passkey，作为以后的登录方式。</p>
      </form> : <div className="grid gap-4 text-center">
        <p className="text-sm leading-relaxed">这个邀请码已使用、已撤销或暂不可用，请联系邀请你的人。</p>
        <Button variant="outline" size="lg" className="h-11 w-full text-sm" onClick={change}>换一个邀请码</Button>
      </div>}
  </AuthShell>
}
