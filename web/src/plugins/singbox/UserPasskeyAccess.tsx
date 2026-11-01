import { useEffect, useRef, useState } from 'react'
import { api } from '../../api'
import { CopyButton, CopyField, ErrorNotice, Field, FormDialog, Loading, Modal, Refresh, DialogPresence } from '../../components'
import { time } from '../../format'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import { validatedSnapshot } from './groupTypes'
import { portalAccessError, validPortalAccess } from './portalAccess'
import type { PortalAccess } from './portalAccess'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

type Invitation = { url: string; expires_at: number }

export default function UserPasskeyAccess({ id, userError, refreshRevision }: { id: number; userError: () => string; refreshRevision: number }) {
  const resource = useResource<unknown>(`/api/plugins/sing-box/users/${id}/portal`, 0)
  const history = useRef<PortalAccess | undefined>(undefined)
  const snapshot = validatedSnapshot(resource, validPortalAccess, history.current)
  if (snapshot.fresh) history.current = snapshot.data
  const access = snapshot.data
  const action = useAction()
  const [editing, setEditing] = useState<{ id: number; reset: boolean; access: PortalAccess }>()
  const [invitation, setInvitation] = useState<Invitation>()
  const reset = (access?.keys ?? 0) > 0
  useEffect(() => { if (refreshRevision) resource.reload() }, [refreshRevision, resource.reload])
  const writeError = (draft?: PortalAccess) => userError() || resourceWriteError(resource) || portalAccessError(resource.getCurrent(), draft)
  const choose = () => {
    if (editing || action.busy || writeError()) return
    const current = resource.getCurrent()
    if (!validPortalAccess(current)) return
    action.clearError(); setEditing({ id, reset: current.keys > 0, access: current })
  }
  const submit = (form: FormData) => {
    if (!editing || editing.id !== id || writeError(editing.access)) return
    void action.run(() => api<Invitation>(`/api/plugins/sing-box/users/${editing.id}/portal/invitation`, 'POST', {
      password: String(form.get('password') ?? ''), totp_code: String(form.get('totp_code') ?? ''), reset: editing.reset,
    }), value => { setEditing(undefined); setInvitation(value); resource.reload() })
  }
  const state = !access ? '' : reset ? `对方已绑定 ${access.keys} 把 Passkey，可以直接用这个地址登录。`
    : access.activation_expires_at !== null ? `开通链接有效至 ${time(access.activation_expires_at)}，对方绑定后即可登录。`
      : '还没有开通。生成开通链接发给对方，绑定 Passkey 后就能自己看订阅、用量和到期。'
  // Each read carries the panel's own state; the address only helps once a Passkey is bound.
  return <Panel aria-label="登录地址" className="mb-0 flex flex-col">
    <PanelHeader className="min-h-10 py-1.5"><PanelTitle>登录地址</PanelTitle><Refresh onClick={resource.reload} /></PanelHeader>
    <div className="flex flex-1 flex-col gap-2.5 p-3.5">
      <ErrorNotice message={snapshot.error} retry={resource.reload} />
      {resource.loading && !access ? <Loading variant="panel" rows={1} /> : access && <>
        {access.url && reset && <code data-slot="portal-address" tabIndex={0} aria-label="用户登录地址" className="bg-muted/50 focus-visible:ring-ring/50 scrollbar-thin block min-w-0 overflow-x-auto rounded-md border px-2.5 py-2 text-xs break-normal whitespace-pre focus-visible:ring-3 focus-visible:outline-none">{access.url}</code>}
        <p className="text-muted-foreground text-xs leading-relaxed">{state}</p>
        {access.configuration.reason && <p className="text-muted-foreground text-xs leading-relaxed">{access.configuration.reason}</p>}
        <div className="mt-auto flex flex-wrap items-center gap-2">
          {access.url && reset && <CopyButton text={access.url} label="复制用户登录地址" primary />}
          <Button variant={reset ? 'ghost' : 'default'} size="sm" className={reset ? 'text-destructive hover:bg-destructive/10 hover:text-destructive' : undefined} disabled={action.busy || Boolean(writeError())} onClick={choose}>{reset ? '重置用户 Passkey' : '生成开通链接'}</Button>
        </div>
      </>}
    </div>
    <DialogPresence>{editing && <FormDialog title={editing.reset ? '重置用户 Passkey？' : '生成用户开通链接'} busy={action.busy} submitDisabled={Boolean(writeError(editing.access))} error={writeError(editing.access) || action.error} onClose={() => setEditing(undefined)} onSubmit={submit} submitLabel={editing.reset ? '确认重置并生成' : '验证并生成'}>
      <p>{editing.reset ? '原有 Passkey、会员登录密码和用户登录会话将立即失效，用户需要通过新链接重新开通，之后可经邮箱验证重新设置密码。订阅、授权和流量保持不变。' : '开通链接十五分钟内有效，只能使用一次；重新生成会使之前的链接失效。请仅发送给该代理用户。'}</p>
      <Field label="管理员密码"><Input name="password" type="password" required maxLength={1024} autoComplete="current-password" /></Field>
      <Field label="二步验证码" hint="已启用时必填，请使用尚未用于登录的验证码。"><Input name="totp_code" inputMode="numeric" pattern="[0-9]{6}" maxLength={6} autoComplete="one-time-code" placeholder="未启用时留空" /></Field>
    </FormDialog>}</DialogPresence>
    <DialogPresence>{invitation && <Modal title="用户开通链接" onClose={() => setInvitation(undefined)}><div data-slot="modal-body" className="p-4"><p>有效至 {time(invitation.expires_at)}，只能使用一次。关闭后不会再次显示完整链接。</p><CopyField text={invitation.url} label="复制开通链接" /><p className="text-muted-foreground mt-3 text-xs leading-relaxed">请将此链接私下交给该用户。它用于绑定 Passkey，不是代理订阅地址。</p></div></Modal>}</DialogPresence>
  </Panel>
}
