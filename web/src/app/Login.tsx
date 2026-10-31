import type { FormEvent } from 'react'
import { ArrowRightIcon, KeyRoundIcon } from 'lucide-react'
import { api, ApiError } from '../api'
import { Brand, ErrorNotice, Field } from '../components'
import { useAction } from '../hooks'
import { loginPasskey, passkeySupport } from '../passkeys'
import { LanguageSelect, useI18n } from '../i18n'
import { useSystemTheme } from '@/lib/use-system-theme'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'

export default function Login({ onLogin, notice }: { onLogin: () => void; notice: string }) {
  const { t } = useI18n()
  const action = useAction()
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

  return <main data-slot="login-page" className="bg-background relative flex min-h-dvh flex-col items-center justify-center px-5 py-10">
    <LanguageSelect className="absolute top-3 right-3" />
    <div className="w-full max-w-sm">
      <Brand className="mb-5" />
      <h1 className="mb-3 text-base font-semibold">{t('欢迎回来')}</h1>
      <div className="bg-card rounded-lg border p-5">
        <ErrorNotice message={action.error || notice} />
        <form onSubmit={submit} className="grid">
          <Field label="管理员登录名"><Input name="login_name" required defaultValue="admin" autoComplete="username" disabled={action.busy} /></Field>
          <Field label="管理员密码">
            <Input name="password" type="password" required autoComplete="current-password" autoFocus
              disabled={action.busy} placeholder={t('请输入密码')} />
          </Field>
          <Field label="二步验证码">
            <Input name="totp_code" inputMode="numeric" pattern="[0-9]{6}" maxLength={6}
              autoComplete="one-time-code" disabled={action.busy} placeholder={t('未启用时留空')} />
          </Field>
          <Button className="mt-1 w-full" disabled={action.busy}>
            {action.busy ? <><Spinner role="presentation" aria-hidden="true" aria-label={undefined} />{t('正在登录…')}</> : <>{t('登录面板')}<ArrowRightIcon aria-hidden="true" /></>}
          </Button>
        </form>
        <div className="my-3 flex items-center gap-3 text-xs"><span className="bg-border h-px flex-1" aria-hidden="true" /><span className="text-muted-foreground">{t('或')}</span><span className="bg-border h-px flex-1" aria-hidden="true" /></div>
        <Button variant="outline" className="w-full" type="button" disabled={action.busy || Boolean(passkeyUnavailable)}
          onClick={() => void action.run(() => loginPasskey('/api/login/passkey'), onLogin)}><KeyRoundIcon aria-hidden="true" />{t('使用初始所有者 Passkey 登录')}</Button>
        {passkeyUnavailable && <p className="text-muted-foreground mt-2 text-xs leading-relaxed">{passkeyUnavailable}</p>}
      </div>
      <p className="text-muted-foreground mt-3 text-center text-xs">{t('代理用户请使用管理员发给你的登录地址。')}</p>
    </div>
  </main>
}
