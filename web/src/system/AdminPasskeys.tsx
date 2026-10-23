import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Field, FormDialog, Loading, Refresh } from '../components'
import { time } from '../format'
import { useAction, useResource } from '../hooks'
import { passkeySupport, registerPasskey } from '../passkeys'
import type { PasskeyEntry, PasskeyInfo } from '../passkeys'

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
  return <section className="panel">
    <div className="panel-heading"><h2>Passkey 通行密钥</h2><div className="row-actions"><Refresh onClick={resource.reload} /><button className="button button-primary button-small" disabled={action.busy || !resource.ready || Boolean(disabled) || (resource.data?.keys.length ?? 0) >= 10} onClick={() => { action.clearError(); setEditing('new') }}>添加 Passkey</button></div></div>
    <div className="panel-body">
      <p>使用设备指纹、面容或安全密钥登录。原密码及二步验证码登录仍然可用；最多绑定十把密钥。</p>
      <ErrorNotice message={resource.error} retry={resource.reload} />
      {disabled && <p className="helper">{disabled}</p>}
      {notice && <p className="notice notice-success" role="status">{notice}</p>}
      {resource.loading ? <Loading /> : resource.data?.keys.length ? <div className="table-wrap"><table><thead><tr><th>名称</th><th>绑定时间</th><th>最近使用</th><th>操作</th></tr></thead><tbody>{resource.data.keys.map(key => <tr key={key.id}><td>{key.name}</td><td>{time(key.created_at)}</td><td>{key.last_used_at ? time(key.last_used_at) : '尚未使用'}</td><td><button className="text-button danger-text" disabled={action.busy || Boolean(disabled)} onClick={() => { action.clearError(); setEditing(key) }}>删除</button></td></tr>)}</tbody></table></div> : <p className="inline-empty">尚未绑定 Passkey。</p>}
    </div>
    {editing && <FormDialog title={editing === 'new' ? '添加管理员 Passkey' : `删除「${editing.name}」？`} onClose={() => setEditing(null)} onSubmit={submit} busy={action.busy} error={action.error} submitLabel={editing === 'new' ? '验证并绑定' : '验证并删除'}>
      {editing === 'new' ? <Field label="密钥名称"><input name="name" required maxLength={64} placeholder="例如：个人手机" /></Field> : <p>删除后该密钥无法再登录，其他管理员会话将退出。当前会话和密码登录保留。</p>}
      <Field label="管理员密码"><input name="password" type="password" required maxLength={1024} autoComplete="current-password" /></Field>
      <Field label="二步验证码" hint="已启用时必填，请使用尚未用于登录的验证码。"><input name="totp_code" inputMode="numeric" pattern="[0-9]{6}" maxLength={6} autoComplete="one-time-code" placeholder="未启用时留空" /></Field>
    </FormDialog>}
  </section>
}
