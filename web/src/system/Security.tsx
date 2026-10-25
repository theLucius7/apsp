import { useEffect, useState } from 'react'
import type { FormEvent } from 'react'
import { api } from '../api'
import { Badge, CopyField, ErrorNotice, Field, Loading, PageHeader, Refresh } from '../components'
import { useAction, useResource } from '../hooks'
import AdminPasskeys from './AdminPasskeys'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { LockIcon } from 'lucide-react'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'

type TotpStatus = { enabled: boolean; pending_expires_at: number | null }
type Setup = { secret: string; otpauth_uri: string; expires_at: number }

// The administrator's own password; previously only a database procedure could change it.
function PasswordChange({ totp }: { totp: boolean }) {
  const action = useAction()
  const [done, setDone] = useState(false)
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const form = event.currentTarget, data = new FormData(form)
    const next = String(data.get('new_password') ?? '')
    setDone(false)
    void action.run(async () => {
      if (next !== String(data.get('repeat_password') ?? '')) throw new Error('两次输入的新密码不一致。')
      await api('/api/security/password', 'POST', { current_password: String(data.get('current_password') ?? ''), new_password: next, ...(totp ? { totp_code: String(data.get('code') ?? '') } : {}) })
    }, () => { form.reset(); setDone(true) })
  }
  return <Panel>
    <PanelHeader><PanelTitle>登录密码</PanelTitle></PanelHeader>
    <form className="p-4" onSubmit={submit}>
      <Field label="当前密码"><Input name="current_password" type="password" autoComplete="current-password" required maxLength={1024} disabled={action.busy} /></Field>
      <Field label="新密码" hint="至少 12 个字符，不能与当前密码相同。"><Input name="new_password" type="password" autoComplete="new-password" required minLength={12} maxLength={1024} disabled={action.busy} /></Field>
      <Field label="再次输入新密码"><Input name="repeat_password" type="password" autoComplete="new-password" required minLength={12} maxLength={1024} disabled={action.busy} /></Field>
      {totp && <Field label="当前验证码"><Input name="code" inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" minLength={6} maxLength={6} required disabled={action.busy} /></Field>}
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">修改后，其他设备上的管理员会话会退出，当前会话保留；所有者的管理员 Passkey 也会撤销，需要重新登记。</p>
      <ErrorNotice message={action.error} />
      {done && <p role="status" className="text-sm">密码已修改。</p>}
      <Button disabled={action.busy}>{action.busy ? '正在验证…' : '修改密码'}</Button>
    </form>
  </Panel>
}

export default function Security() {
  const resource = useResource<TotpStatus>('/api/security/totp')
  const action = useAction()
  const [setup, setSetup] = useState<Setup>()
  const [now, setNow] = useState(Math.floor(Date.now() / 1000))
  const [notice, setNotice] = useState('')
  useEffect(() => {
    if (!setup) return
    const timer = window.setInterval(() => {
      const time = Math.floor(Date.now() / 1000)
      setNow(time)
      if (time >= setup.expires_at) { setSetup(undefined); setNotice('本次设置已过期，请重新生成。'); resource.reload() }
    }, 1000)
    return () => window.clearInterval(timer)
  }, [setup, resource.reload])
  useEffect(() => { if (resource.data?.enabled) setSetup(undefined) }, [resource.data?.enabled])
  const begin = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const form = event.currentTarget
    const password = String(new FormData(form).get('password') ?? '')
    void action.run(() => api<Setup>('/api/security/totp/setup', 'POST', { password }), value => {
      form.reset(); setSetup(value); setNow(Math.floor(Date.now() / 1000)); setNotice(''); resource.reload()
    })
  }
  const confirm = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const code = String(new FormData(event.currentTarget).get('code') ?? '')
    void action.run(() => api('/api/security/totp/confirm', 'POST', { code }), () => {
      setSetup(undefined); setNotice('二步验证已启用，其他设备的管理员会话已退出。下次登录请使用验证器更新后的验证码。'); resource.reload()
    })
  }
  const disable = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    const form = event.currentTarget
    const data = new FormData(form)
    void action.run(() => api('/api/security/totp/disable', 'POST', {
      password: String(data.get('password') ?? ''), code: String(data.get('code') ?? ''),
    }), () => { form.reset(); setSetup(undefined); setNotice('二步验证已关闭，其他设备的管理员会话已退出。'); resource.reload() })
  }
  return <>
    <PageHeader eyebrow="系统" title="账号安全" description="管理当前管理员的登录密码、Passkey 和二步验证。其他管理员、会话与 API 令牌在「管理与安全」中管理。"><Refresh onClick={resource.reload} /></PageHeader>
    {resource.data && <PasswordChange totp={resource.data.enabled} />}
    <AdminPasskeys />
    <ErrorNotice message={resource.error} retry={resource.reload} />
    <ErrorNotice message={action.error} />
    {notice && <Notice tone="quiet" role="status"><LockIcon className="text-muted-foreground mt-0.5 size-4 shrink-0" aria-hidden="true" /><p className="mt-1 text-muted-foreground">{notice}</p></Notice>}
    <Panel>
      <PanelHeader><PanelTitle>二步验证</PanelTitle>{resource.data && <Badge tone={resource.data.enabled ? 'good' : 'neutral'}>{resource.data.enabled ? '已启用' : '未启用'}</Badge>}</PanelHeader>
      <div className="p-4">
        {resource.loading && !resource.data ? <Loading /> : resource.data?.enabled ? <>
          <p>登录时需要管理员密码和验证器中的六位验证码。每个验证码只能成功使用一次。</p>
          <form onSubmit={disable}>
            <Field label="管理员密码"><Input name="password" type="password" autoComplete="current-password" required maxLength={1024} disabled={action.busy} /></Field>
            <Field label="当前验证码" hint="请输入尚未用于本次登录的验证码；必要时等待验证器更新。"><Input name="code" inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" minLength={6} maxLength={6} required disabled={action.busy} /></Field>
            <p className="text-muted-foreground mt-3 text-xs leading-relaxed">关闭后将退出其他设备的管理员会话，当前会话保留。</p>
            <Button variant="destructive" disabled={action.busy}>{action.busy ? '正在验证…' : '验证并关闭二步验证'}</Button>
          </form>
        </> : resource.data ? <>
          <p>使用支持动态验证码的验证器添加账户，然后输入验证码完成启用。确认前，原有登录方式仍然有效。</p>
          {setup ? <>
            <p className="text-muted-foreground mt-3 text-xs leading-relaxed">本次设置剩余 {Math.max(0, setup.expires_at - now)} 秒。秘密仅在本次生成后显示，请勿分享；离开此页面后需要重新生成。</p>
            <div data-slot="field" className="mb-4 grid min-w-0 gap-1.5 last:mb-0"><span className="text-sm leading-snug font-medium">手动添加的秘密</span><CopyField text={setup.secret} label="复制秘密" /></div>
            <div data-slot="field" className="mb-4 grid min-w-0 gap-1.5 last:mb-0"><span className="text-sm leading-snug font-medium">验证器导入地址</span><CopyField text={setup.otpauth_uri} label="复制导入地址" /></div>
            <form onSubmit={confirm}>
              <Field label="验证器中的六位验证码"><Input name="code" inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" minLength={6} maxLength={6} required disabled={action.busy} /></Field>
              <p className="text-muted-foreground mt-3 text-xs leading-relaxed">启用成功后，其他设备的管理员会话将退出，当前会话保留。</p>
              <Button disabled={action.busy || now >= setup.expires_at}>{action.busy ? '正在验证…' : '确认启用'}</Button>
            </form>
          </> : <form onSubmit={begin}>
            {!!resource.data.pending_expires_at && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">存在待确认设置。重新生成会使之前的秘密失效。</p>}
            <Field label="管理员密码"><Input name="password" type="password" autoComplete="current-password" required maxLength={1024} disabled={action.busy} /></Field>
            <Button disabled={action.busy}>{action.busy ? '正在验证…' : resource.data.pending_expires_at ? '重新生成设置' : '开始设置'}</Button>
          </form>}
        </> : null}
      </div>
    </Panel>
    <p className="text-muted-foreground my-5 flex items-center justify-center gap-2 text-center text-xs leading-relaxed"><LockIcon className="size-3.5" aria-hidden="true" />请保持手机和服务器时间准确。丢失验证器时，需要部署所有者通过受保护的数据库管理通道恢复，密码本身不能关闭二步验证。</p>
  </>
}
