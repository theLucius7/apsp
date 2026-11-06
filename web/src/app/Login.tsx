import { useRef, useState, type FormEvent } from 'react'
import { ArrowRightIcon, EyeIcon, EyeOffIcon, KeyRoundIcon, LockKeyholeIcon, ShieldCheckIcon, UserRoundIcon } from 'lucide-react'
import { api, ApiError } from '../api'
import { ErrorNotice } from '../components'
import { useAction } from '../hooks'
import { loginPasskey, passkeySupport } from '../passkeys'
import { LanguageSelect, useI18n } from '../i18n'
import { useSystemTheme } from '@/lib/use-system-theme'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { AuthDivider, AuthField, AuthShell } from '@/components/auth-shell'

export default function Login({ onLogin, notice }: { onLogin: () => void; notice: string }) {
  const { t } = useI18n()
  const action = useAction()
  const [revealed, setRevealed] = useState(false)
  useSystemTheme()
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const data = new FormData(event.currentTarget)
    const login_name = String(data.get('login_name') ?? 'admin')
    const password = String(data.get('password') ?? '')
    const totp_code = String(data.get('totp_code') ?? '')
    void action.run(async () => {
      try {
        await api('/api/login', 'POST', { login_name, password, totp_code })
      } catch (error) {
        if (error instanceof ApiError && error.status === 401) {
          throw new Error(t('密码或验证码不正确、已过期或已使用，请重新输入。'))
        }
        throw error
      }
    }, onLogin)
  }
  const passkeyUnavailable = passkeySupport()
  const form = useRef<HTMLFormElement>(null)
  // Passkey sign-in uses the login name above; each administrator has their own keys.
  const passkey = () => {
    const name = form.current ? String(new FormData(form.current).get('login_name') ?? '').trim() : ''
    void action.run(() => loginPasskey('/api/login/passkey', true, { login_name: name || 'admin' }), onLogin)
  }

  return <AuthShell slot="login-page" corner={<LanguageSelect />} title={t('欢迎回来')}
    footer={t('其他用户请使用管理员发给你的登录地址。')}>
    <ErrorNotice message={action.error || notice} />
    <form ref={form} onSubmit={submit} className="grid gap-4">
      <AuthField id="login-name" label={t('管理员登录名')} icon={UserRoundIcon}>
        <Input id="login-name" name="login_name" required defaultValue="admin" autoComplete="username" disabled={action.busy} className="h-11 pl-10" />
      </AuthField>
      <AuthField id="login-password" label={t('管理员密码')} icon={LockKeyholeIcon}>
        <Input id="login-password" name="password" type={revealed ? 'text' : 'password'} required autoComplete="current-password" autoFocus
          disabled={action.busy} placeholder={t('请输入密码')} className="h-11 pr-11 pl-10" />
        <button type="button" onClick={() => setRevealed(value => !value)} aria-label={t(revealed ? '隐藏密码' : '显示密码')} aria-pressed={revealed}
          className="text-muted-foreground hover:text-foreground focus-visible:shadow-focus absolute top-1/2 right-1.5 grid size-8 -translate-y-1/2 place-items-center rounded-lg outline-none transition-colors">
          {revealed ? <EyeOffIcon className="size-4" aria-hidden="true" /> : <EyeIcon className="size-4" aria-hidden="true" />}
        </button>
      </AuthField>
      <AuthField id="login-totp" label={t('二步验证码')} icon={ShieldCheckIcon}>
        <Input id="login-totp" name="totp_code" inputMode="numeric" pattern="[0-9]{6}" maxLength={6}
          autoComplete="one-time-code" disabled={action.busy} placeholder={t('未启用时留空')} className="h-11 pl-10 tabular-nums" />
      </AuthField>
      <Button size="lg" className="mt-1 h-11 w-full text-sm" disabled={action.busy}>
        {action.busy ? <><Spinner role="presentation" aria-hidden="true" aria-label={undefined} />{t('正在登录…')}</> : <>{t('登录面板')}<ArrowRightIcon aria-hidden="true" className="transition-transform group-hover/button:translate-x-0.5" /></>}
      </Button>
    </form>
    <AuthDivider label={t('或')} />
    <Button variant="outline" size="lg" className="h-11 w-full text-sm" type="button" disabled={action.busy || Boolean(passkeyUnavailable)}
      onClick={passkey}><KeyRoundIcon aria-hidden="true" />{t('使用 Passkey 登录')}</Button>
    {passkeyUnavailable
      ? <p className="text-muted-foreground mt-2.5 text-xs leading-relaxed">{passkeyUnavailable}</p>
      : <p className="text-muted-foreground mt-2.5 text-xs leading-relaxed">{t('按上面的登录名验证该管理员的 Passkey，无需密码。')}</p>}
  </AuthShell>
}
