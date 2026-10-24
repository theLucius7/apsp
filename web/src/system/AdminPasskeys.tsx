import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Field, FormDialog, Loading, Refresh, DialogPresence } from '../components'
import { time } from '../format'
import { useAction, useResource } from '../hooks'
import { passkeySupport, registerPasskey } from '../passkeys'
import type { PasskeyEntry, PasskeyInfo } from '../passkeys'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'

export default function AdminPasskeys() {
  const resource = useResource<{ configuration: PasskeyInfo; keys: PasskeyEntry[] }>('/api/security/passkeys', 0)
  const action = useAction()
  const [editing, setEditing] = useState<PasskeyEntry | 'new' | null>(null)
  const [notice, setNotice] = useState('')
  const disabled = passkeySupport() || resource.data?.configuration.reason || ''
  const submit = (form: FormData) => {
    if (!editing) return
    const proof = { password: String(form.get('password') ?? ''), totp_code: String(form.get('totp_code') ?? '') }
    void action.run(() => editing === 'new'
      ? registerPasskey('/api/security/passkeys', { ...proof, name: String(form.get('name') ?? '') })
      : api(`/api/security/passkeys/${editing.id}/remove`, 'POST', proof), () => {
      setNotice(editing === 'new' ? 'Passkey 已绑定，下次可直接验证并登录。' : 'Passkey 已删除，其他管理员会话已退出。')
      setEditing(null); resource.reload()
    })
  }
  return <Panel>
    <PanelHeader><PanelTitle>Passkey 通行密钥</PanelTitle><div className="flex flex-wrap items-center justify-end gap-2"><Refresh onClick={resource.reload} /><Button size="sm" disabled={action.busy || !resource.ready || Boolean(disabled) || (resource.data?.keys.length ?? 0) >= 10} onClick={() => { action.clearError(); setEditing('new') }}>添加 Passkey</Button></div></PanelHeader>
    <div className="p-4">
      <p>使用设备指纹、面容或安全密钥登录。原密码及二步验证码登录仍然可用；最多绑定十把密钥。</p>
      <ErrorNotice message={resource.error} retry={resource.reload} />
      {disabled && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{disabled}</p>}
      {notice && <Notice tone="success" role="status">{notice}</Notice>}
      {resource.loading ? <Loading /> : resource.data?.keys.length ? <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>名称</TableHead><TableHead>绑定时间</TableHead><TableHead>最近使用</TableHead><TableHead>操作</TableHead></TableRow></TableHeader><TableBody>{resource.data.keys.map(key => <TableRow key={key.id}><TableCell>{key.name}</TableCell><TableCell>{time(key.created_at)}</TableCell><TableCell>{key.last_used_at ? time(key.last_used_at) : '尚未使用'}</TableCell><TableCell><Button variant="ghost" size="sm" className="text-destructive hover:bg-destructive/10 hover:text-destructive dark:hover:bg-destructive/20" disabled={action.busy || Boolean(disabled)} onClick={() => { action.clearError(); setEditing(key) }}>删除</Button></TableCell></TableRow>)}</TableBody></Table></div> : <p className="text-muted-foreground px-4 py-6 text-sm">尚未绑定 Passkey。</p>}
    </div>
    <DialogPresence>{editing && <FormDialog title={editing === 'new' ? '添加管理员 Passkey' : `删除「${editing.name}」？`} onClose={() => setEditing(null)} onSubmit={submit} busy={action.busy} error={action.error} submitLabel={editing === 'new' ? '验证并绑定' : '验证并删除'}>
      {editing === 'new' ? <Field label="密钥名称"><Input name="name" required maxLength={64} placeholder="例如：个人手机" /></Field> : <p>删除后该密钥无法再登录，其他管理员会话将退出。当前会话和密码登录保留。</p>}
      <Field label="管理员密码"><Input name="password" type="password" required maxLength={1024} autoComplete="current-password" /></Field>
      <Field label="二步验证码" hint="已启用时必填，请使用尚未用于登录的验证码。"><Input name="totp_code" inputMode="numeric" pattern="[0-9]{6}" maxLength={6} autoComplete="one-time-code" placeholder="未启用时留空" /></Field>
    </FormDialog>}</DialogPresence>
  </Panel>
}
