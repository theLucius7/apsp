import { useRef, useState, type FormEvent, type ReactNode } from 'react'
import { ActivityIcon, ArrowRightIcon, EyeIcon, EyeOffIcon, KeyRoundIcon, LockKeyholeIcon, ShieldCheckIcon, UserRoundIcon, UsersIcon, WaypointsIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { api, ApiError } from '../api'
import { Brand, BrandMark, ErrorNotice } from '../components'
import { useAction } from '../hooks'
import { loginPasskey, passkeySupport } from '../passkeys'
import { LanguageSelect, useI18n } from '../i18n'
import { useSystemTheme } from '@/lib/use-system-theme'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'

const features: { icon: LucideIcon; title: string; text: string }[] = [
  { icon: WaypointsIcon, title: '节点与链路', text: '入口、中转、落地一处配置，自动下发到每台服务器。' },
  { icon: UsersIcon, title: '用户与订阅', text: '按策略组授权，流量逐用户记账，订阅与用量自助查看。' },
  { icon: ActivityIcon, title: '服务器与费用', text: '实时监控、三网拨测与告警，续费到期提前提醒。' },
]

/** A labelled field with an icon at its start; the label names the input alone. */
function LoginField({ id, label, icon: Icon, children }: { id: string; label: string; icon: LucideIcon; children: ReactNode }) {
  return <div className="grid gap-1.5">
    <label htmlFor={id} className="text-[13px] font-medium">{label}</label>
    <div className="relative"><Icon className="text-muted-foreground pointer-events-none absolute top-1/2 left-3.5 z-10 size-4 -translate-y-1/2" aria-hidden="true" />{children}</div>
  </div>
}

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

  return <main data-slot="login-page" className="relative grid min-h-dvh place-items-center px-5 py-12 sm:px-8">
    <LanguageSelect className="absolute top-4 right-4" />
    <div className="grid w-full max-w-[64rem] items-center gap-14 lg:grid-cols-[minmax(0,1.1fr)_minmax(0,1fr)]">
      {/* What apsp is, beside the form on wide screens. */}
      <section aria-label={t('apsp 简介')} className="hidden min-w-0 lg:block">
        <BrandMark className="size-16 drop-shadow-[0_12px_28px_color-mix(in_oklch,var(--brand-from)_45%,transparent)]" />
        <p className="text-primary mt-8 text-[13px] font-semibold tracking-[0.18em]">{t('代理网络控制平面')}</p>
        <h2 className="mt-3 text-[40px] leading-[1.15] font-semibold tracking-[-0.02em]">{t('一处配置，')}<br />{t('整张网络随之运转')}</h2>
        <p className="text-muted-foreground mt-5 max-w-[30rem] text-[15px] leading-relaxed">{t('入口、中转与落地在面板里编排，节点和订阅自动下发；面板离线时，已应用的节点照常工作。')}</p>
        <ul className="mt-9 grid max-w-[30rem] gap-4">
          {features.map(({ icon: Icon, title, text }) => <li key={title} className="flex items-start gap-3.5">
            <span className="bg-card text-primary shadow-soft grid size-10 shrink-0 place-items-center rounded-xl border" aria-hidden="true"><Icon className="size-[18px]" /></span>
            <span className="grid gap-0.5 pt-0.5"><strong className="text-sm font-medium">{t(title)}</strong><span className="text-muted-foreground text-[13px] leading-relaxed">{t(text)}</span></span>
          </li>)}
        </ul>
      </section>
      <div className="mx-auto w-full max-w-[26rem] min-w-0">
        <Brand className="mb-9 lg:hidden" />
        <h1 className="text-[28px] leading-tight font-semibold tracking-[-0.02em]">{t('欢迎回来')}</h1>
        <p className="text-muted-foreground mt-1.5 mb-6 text-sm">{t('登录以管理你的代理网络。')}</p>
        <div className="bg-card shadow-lifted rounded-[20px] border p-6 sm:p-7">
          <ErrorNotice message={action.error || notice} />
          <form ref={form} onSubmit={submit} className="grid gap-4">
            <LoginField id="login-name" label={t('管理员登录名')} icon={UserRoundIcon}>
              <Input id="login-name" name="login_name" required defaultValue="admin" autoComplete="username" disabled={action.busy} className="h-11 pl-10" />
            </LoginField>
            <LoginField id="login-password" label={t('管理员密码')} icon={LockKeyholeIcon}>
              <Input id="login-password" name="password" type={revealed ? 'text' : 'password'} required autoComplete="current-password" autoFocus
                disabled={action.busy} placeholder={t('请输入密码')} className="h-11 pr-11 pl-10" />
              <button type="button" onClick={() => setRevealed(value => !value)} aria-label={t(revealed ? '隐藏密码' : '显示密码')} aria-pressed={revealed}
                className="text-muted-foreground hover:text-foreground focus-visible:shadow-focus absolute top-1/2 right-1.5 grid size-8 -translate-y-1/2 place-items-center rounded-lg outline-none transition-colors">
                {revealed ? <EyeOffIcon className="size-4" aria-hidden="true" /> : <EyeIcon className="size-4" aria-hidden="true" />}
              </button>
            </LoginField>
            <LoginField id="login-totp" label={t('二步验证码')} icon={ShieldCheckIcon}>
              <Input id="login-totp" name="totp_code" inputMode="numeric" pattern="[0-9]{6}" maxLength={6}
                autoComplete="one-time-code" disabled={action.busy} placeholder={t('未启用时留空')} className="h-11 pl-10 tabular-nums" />
            </LoginField>
            <Button size="lg" className="mt-1 h-11 w-full text-sm" disabled={action.busy}>
              {action.busy ? <><Spinner role="presentation" aria-hidden="true" aria-label={undefined} />{t('正在登录…')}</> : <>{t('登录面板')}<ArrowRightIcon aria-hidden="true" className="transition-transform group-hover/button:translate-x-0.5" /></>}
            </Button>
          </form>
          <div className="my-5 flex items-center gap-3 text-xs"><span className="bg-border h-px flex-1" aria-hidden="true" /><span className="text-muted-foreground">{t('或')}</span><span className="bg-border h-px flex-1" aria-hidden="true" /></div>
          <Button variant="outline" size="lg" className="h-11 w-full text-sm" type="button" disabled={action.busy || Boolean(passkeyUnavailable)}
            onClick={passkey}><KeyRoundIcon aria-hidden="true" />{t('使用 Passkey 登录')}</Button>
          {passkeyUnavailable
            ? <p className="text-muted-foreground mt-2.5 text-xs leading-relaxed">{passkeyUnavailable}</p>
            : <p className="text-muted-foreground mt-2.5 text-xs leading-relaxed">{t('按上面的登录名验证该管理员的 Passkey，无需密码。')}</p>}
        </div>
        <p className="text-muted-foreground mt-5 text-center text-xs">{t('代理用户请使用管理员发给你的登录地址。')}</p>
      </div>
    </div>
  </main>
}
