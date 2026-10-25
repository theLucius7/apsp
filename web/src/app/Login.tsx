import type { FormEvent } from 'react'
import { m } from 'motion/react'
import { ArrowRightIcon, CircleCheckIcon, KeyRoundIcon, LockIcon, ServerIcon, ActivityIcon } from 'lucide-react'
import { api, ApiError } from '../api'
import { Brand, ErrorNotice, Field } from '../components'
import { useAction } from '../hooks'
import { loginPasskey, passkeySupport } from '../passkeys'
import { LanguageSelect, useI18n } from '../i18n'
import { pageMotion } from '@/lib/motion'
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

  return <main data-slot="login-page" className="bg-background grid min-h-dvh lg:grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)]">
    <div className="bg-muted/60 relative isolate flex flex-col gap-8 overflow-hidden border-b p-6 lg:border-r lg:border-b-0 lg:p-12">
      <div aria-hidden="true" className="border-primary/15 absolute -right-40 -bottom-40 -z-10 size-[32rem] rounded-full border shadow-[0_0_0_4rem_var(--color-muted),0_0_0_4.1rem_color-mix(in_oklch,var(--primary)_12%,transparent)]" />
      <Brand />
      <div className="my-auto max-w-md space-y-5 max-lg:hidden">
        <span className="text-primary text-xs font-medium tracking-wide">{t('一处管理，始终有序')}</span>
        <h1 className="text-4xl leading-tight font-semibold tracking-tight">{t('掌握每一台')}<br />{t('服务器。')}</h1>
        <p className="text-muted-foreground text-base leading-relaxed">{t('从设备接入到节点授权，')}<br />{t('让网络的每一步都清晰可见。')}</p>
        <ul className="grid gap-3 pt-4 text-sm">
          <li className="flex items-center gap-2.5"><ServerIcon className="text-primary size-4" aria-hidden="true" />{t('设备主动连接')}</li>
          <li className="flex items-center gap-2.5"><CircleCheckIcon className="text-primary size-4" aria-hidden="true" />{t('配置自动对账')}</li>
          <li className="flex items-center gap-2.5"><ActivityIcon className="text-primary size-4" aria-hidden="true" />{t('流量按用户统计')}</li>
        </ul>
      </div>
      <div className="text-muted-foreground text-xs tracking-wide max-lg:hidden">{t('apsp · 自托管的代理网络控制平面')}</div>
    </div>
    <div className="relative flex flex-col items-center justify-center gap-8 px-6 py-10 sm:px-10">
      <LanguageSelect className="absolute top-4 right-4 sm:top-6 sm:right-8" />
      <m.div {...pageMotion} className="w-full max-w-sm">
        <span className="bg-primary/10 text-primary mb-5 inline-grid size-10 place-items-center rounded-xl max-sm:hidden"><LockIcon className="size-5" aria-hidden="true" /></span>
        <h2 className="text-2xl font-semibold tracking-tight">{t('欢迎回来')}</h2>
        <p className="text-muted-foreground mt-2 mb-6 text-sm">{t('输入管理员密码；已启用二步验证时，还需验证器中的验证码。')}</p>
        <ErrorNotice message={action.error || notice} />
        <form onSubmit={submit} className="grid">
          <Field label="管理员登录名"><Input name="login_name" required defaultValue="admin" autoComplete="username" disabled={action.busy} /></Field>
          <Field label="管理员密码">
            <Input name="password" type="password" required autoComplete="current-password" autoFocus
              disabled={action.busy} placeholder={t('请输入密码')} />
          </Field>
          <Field label="二步验证码" hint="启用二步验证后，请输入验证器当前的六位数字。">
            <Input name="totp_code" inputMode="numeric" pattern="[0-9]{6}" maxLength={6}
              autoComplete="one-time-code" disabled={action.busy} placeholder={t('未启用时留空')} />
          </Field>
          <Button size="lg" className="mt-2 w-full justify-between" disabled={action.busy}>
            {action.busy ? <><span className="flex items-center gap-2"><Spinner role="presentation" aria-hidden="true" aria-label={undefined} />{t('正在登录…')}</span></> : <>{t('登录面板')}<ArrowRightIcon aria-hidden="true" /></>}
          </Button>
        </form>
        <Button variant="outline" size="lg" className="mt-3 w-full" type="button" disabled={action.busy || Boolean(passkeyUnavailable)}
          onClick={() => void action.run(() => loginPasskey('/api/login/passkey'), onLogin)}><KeyRoundIcon aria-hidden="true" />{t('使用初始所有者 Passkey 登录')}</Button>
        {passkeyUnavailable && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{passkeyUnavailable}</p>}
        <div className="text-muted-foreground mt-6 flex items-center justify-center gap-1.5 text-xs"><LockIcon className="size-3.5" aria-hidden="true" />{t('仅限管理员访问，使用部署时设置的密码。')}</div>
      </m.div>
      <div className="text-muted-foreground text-xs tracking-wide">{t('你的服务器，你的控制权。')}</div>
    </div>
  </main>
}
