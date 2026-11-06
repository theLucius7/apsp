import AccountPanel from './accounts/AccountPanel'
import { PortalSubscription, PortalSummary } from './accounts/PortalOverview'
import { onOwnAddress } from './accounts/types'
import type { SubscriptionStatus } from './accounts/PortalOverview'
import MemberLines from './lines/MemberLines'
import { useEffect, useState } from 'react'
import type { FormEvent } from 'react'
import { KeyRoundIcon, LogOutIcon, MailIcon, PlusIcon, ShieldCheckIcon, SmartphoneIcon } from 'lucide-react'
import { api, errorMessage } from '../../api'
import { Brand, CopyField, ErrorNotice, Field, FormDialog, Loading, DialogPresence } from '../../components'
import { time } from '../../format'
import { useAction } from '../../hooks'
import { loginPasskey, passkeySupport, registerPasskey } from '../../passkeys'
import type { PasskeyEntry, PasskeyInfo } from '../../passkeys'
import { LanguageSelect, useI18n } from '../../i18n'

import { Button, buttonVariants } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Spinner } from '@/components/ui/spinner'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { AuthDivider, AuthField, AuthShell } from '@/components/auth-shell'
import { useSystemTheme } from '@/lib/use-system-theme'

type View = { configuration: PasskeyInfo } & ({ authenticated: false } | {
  authenticated: true; legacy_compatibility?: { last_sync_at: number | null } | null; name: string; subscription_url: string; clash_subscription_url?: string; usage: { uplink: string; downlink: string }; keys: PasskeyEntry[];
  subscription_status: SubscriptionStatus
})

export default function UserPortal({ account, activation }: { account: string; activation?: string }) {
  const { t } = useI18n()
  useSystemTheme()
  const base = `/api/plugins/sing-box/portal/${account}`
  const bookmark = `${window.location.origin}/#/plugins/sing-box/account/${account}`
  const [token, setToken] = useState(activation)
  const [data, setData] = useState<View>()
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(true)
  const [revision, setRevision] = useState(0)
  const [editing, setEditing] = useState<PasskeyEntry | 'new' | null>(null)
  const [notice, setNotice] = useState('')
  const action = useAction()
  const reload = () => setRevision(value => value + 1)
  useEffect(() => {
    document.title = `${t('我的账户')} · ${t('apsp')}`
    if (activation) window.history.replaceState(null, '', `#/plugins/sing-box/account/${account}`)
  }, [account, activation, t])
  useEffect(() => {
    const controller = new AbortController()
    let active = true, pending = false
    const load = async () => {
      if (pending) return
      pending = true
      try {
        const value = await api<View>(base, 'GET', undefined, controller.signal, false)
        if (active) { setData(value); setError('') }
      } catch (error) { if (active) { setError(errorMessage(error)); setData(undefined) } }
      finally { pending = false; if (active) setLoading(false) }
    }
    setLoading(true); void load()
    const timer = window.setInterval(() => { if (document.visibilityState === 'visible') void load() }, 30000)
    return () => { active = false; controller.abort(); window.clearInterval(timer) }
  }, [base, revision])
  const disabled = passkeySupport() || data?.configuration.reason || ''
  const activate = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const name = String(new FormData(event.currentTarget).get('name') ?? '')
    void action.run(() => registerPasskey(base, { name, activation_token: token }, false), () => {
      setToken(undefined); setNotice('Passkey 已绑定，请收藏此入口供下次登录。'); reload()
    })
  }
  const manage = (form: FormData) => {
    if (!editing) return
    void action.run(async () => {
      // Without any passkey yet, the recent password login is the proof for adding the first one.
      if (data?.authenticated && data.keys.length > 0) await loginPasskey(`${base}/login`, false)
      if (editing === 'new') await registerPasskey(base, { name: String(form.get('name') ?? '') }, false)
      else await api(`${base}/keys/${editing.id}/remove`, 'POST', undefined, undefined, false)
    }, () => { setEditing(null); setNotice(editing === 'new' ? '新 Passkey 已绑定。' : 'Passkey 已删除，其他用户会话已退出。'); reload() })
  }
  const logout = () => void action.run(() => api(`${base}/logout`, 'POST', undefined, undefined, false), () => { setData(undefined); setNotice(''); setEditing(null); reload() })

  if (loading && !data) return <main data-slot="proxy-portal" className="grid min-h-dvh place-items-center px-6"><Loading /></main>
  // Signing in and activation share the sign-in layout; the account itself is a page of cards.
  if (data && !data.authenticated) return <AuthShell slot="proxy-portal" corner={<LanguageSelect />}
    title={token ? '开通我的账户' : '登录我的账户'}
    footer={token ? '开通链接只能使用一次。' : '尚未开通或丢失密钥时，请联系管理员获取新的开通链接。'}>
    <ErrorNotice message={error} retry={reload} /><ErrorNotice message={action.error} />
    {notice && <Notice tone="success" role="status">{notice}</Notice>}
    {token ? <form onSubmit={activate} className="grid gap-4">
      <AuthField id="portal-key-name" label="密钥名称" icon={SmartphoneIcon}>
        <Input id="portal-key-name" name="name" required maxLength={64} placeholder="例如：我的手机" disabled={action.busy} className="h-11 pl-10" />
      </AuthField>
      <Button size="lg" className="h-11 w-full text-sm" disabled={action.busy || Boolean(disabled)}>
        {action.busy ? <Spinner role="presentation" aria-hidden="true" aria-label={undefined} /> : <KeyRoundIcon aria-hidden="true" />}绑定 Passkey 并开通
      </Button>
    </form> : <>
      <Button size="lg" className="h-11 w-full text-sm" disabled={action.busy || Boolean(disabled)} onClick={() => void action.run(() => loginPasskey(`${base}/login`, false), () => { setNotice(''); reload() })}>
        {action.busy ? <><Spinner role="presentation" aria-hidden="true" aria-label={undefined} />正在验证…</> : <><KeyRoundIcon aria-hidden="true" />使用 Passkey 登录</>}
      </Button>
      {/* A user's own address serves only this portal; email sign-in lives on the panel's address. */}
      {!onOwnAddress() && <>
        <AuthDivider label="或" />
        <a className={buttonVariants({ variant: 'outline', size: 'lg', className: 'h-11 w-full text-sm' })} href="#/plugins/sing-box/member"><MailIcon aria-hidden="true" />邮箱密码登录 / 找回密码</a>
      </>}
    </>}
    {disabled && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{disabled}</p>}
  </AuthShell>

  const view = data?.authenticated ? data : undefined
  return <main data-slot="proxy-portal" className="mx-auto max-w-[1040px] px-6 pt-6 pb-16 max-sm:px-3 max-sm:pt-4">
    <header className="mb-6 flex min-w-0 items-center gap-3">
      <Brand compact />
      <span className="bg-border h-5 w-px max-[380px]:hidden" aria-hidden="true" />
      <span className="text-muted-foreground min-w-0 truncate text-sm max-[380px]:hidden">{t('我的账户')}</span>
      <LanguageSelect className="ml-auto flex-none" />
      {view && <Button variant="outline" size="sm" className="max-sm:size-8 max-sm:px-0 pointer-coarse:max-sm:size-10" disabled={action.busy} onClick={logout} title={t('退出登录')}>
        <LogOutIcon aria-hidden="true" /><span className="max-sm:sr-only">{t('退出登录')}</span>
      </Button>}
    </header>
    <ErrorNotice message={error} retry={reload} /><ErrorNotice message={!editing ? action.error : ''} />
    {notice && <Notice tone="success" role="status">{notice}</Notice>}
    {view && <>
      <div className="mb-4 grid gap-4 lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
        <PortalSummary name={view.name} status={view.subscription_status} usage={view.usage} legacy={Boolean(view.legacy_compatibility)} refresh={reload} />
        <PortalSubscription url={view.subscription_url} clash={view.clash_subscription_url} status={view.subscription_status} />
      </div>
      <MemberLines key={`lines:${account}`} account={account} />
      <AccountPanel key={account} account={account} />
      <Panel data-slot="portal-security">
        <PanelHeader><PanelTitle><ShieldCheckIcon className="text-primary size-4" aria-hidden="true" />登录与安全</PanelTitle>
          <Button size="sm" disabled={action.busy || Boolean(disabled) || view.keys.length >= 10} onClick={() => { action.clearError(); setEditing('new') }}><PlusIcon aria-hidden="true" />添加 Passkey</Button></PanelHeader>
        <div className="grid gap-6 p-5">
          <section className="min-w-0">
            <h3 className="text-sm font-medium">我的登录地址</h3>
            <p className="text-muted-foreground mt-1 text-xs leading-relaxed">收藏这个地址，下次从这里用 Passkey 登录。</p>
            <CopyField text={bookmark} label="复制我的登录地址" singleLine className="mt-2.5 mb-0" />
          </section>
          <section className="min-w-0">
            <h3 className="text-sm font-medium">Passkey</h3>
            <p className="text-muted-foreground mt-1 text-xs leading-relaxed">可添加备用密钥；丢失全部密钥时，请联系管理员重置。管理密钥前需要重新验证。</p>
            {disabled && <p className="text-muted-foreground mt-1 text-xs leading-relaxed">{disabled}</p>}
            <div className="mt-2 max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>名称</TableHead><TableHead>最近使用</TableHead><TableHead className="w-16"><span className="sr-only">操作</span></TableHead></TableRow></TableHeader><TableBody>
              {view.keys.map(key => <TableRow key={key.id}>
                <TableCell><span className="flex min-w-0 items-center gap-2"><KeyRoundIcon className="text-primary size-3.5 shrink-0" aria-hidden="true" /><span className="min-w-0 wrap-anywhere">{key.name}</span></span></TableCell>
                <TableCell className="text-muted-foreground">{key.last_used_at ? time(key.last_used_at) : '尚未使用'}</TableCell>
                <TableCell className="text-right"><Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={action.busy || view.keys.length <= 1 || Boolean(disabled)} onClick={() => { action.clearError(); setEditing(key) }}>删除</Button></TableCell>
              </TableRow>)}
            </TableBody></Table></div>
            {view.keys.length === 1 && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">至少保留一把密钥。添加备用密钥后可删除旧密钥。</p>}
          </section>
        </div>
      </Panel>
    </>}
    <DialogPresence>{editing && <FormDialog title={editing === 'new' ? '添加备用 Passkey' : `删除「${editing.name}」？`} onClose={() => setEditing(null)} onSubmit={manage} busy={action.busy} error={action.error} submitLabel={editing === 'new' ? '验证并添加' : '验证并删除'}>
      {editing === 'new' ? <><p>先验证已有密钥，再选择新设备或另一把安全密钥。</p><Field label="新密钥名称"><Input name="name" required maxLength={64} /></Field></> : <p>该密钥将无法再登录，其他用户会话将退出。当前会话保留。</p>}
    </FormDialog>}</DialogPresence>
  </main>
}
