import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'
import { Checkbox } from '@/components/ui/checkbox'
import { api } from '../../../api'
import { ErrorNotice, Field } from '../../../components'
import { useAction, useResource } from '../../../hooks'
import { root } from './types'
type Mail = { enabled: boolean; reminders_enabled: boolean; host: string; port: number; security: string; username: string; from_address: string; credential_id: string | null; blocked_domains: string[]; version: number }
export default function MailSettings() {
  const data = useResource<Mail>(`${root}/mail`, 30000), action = useAction()
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>成员邮件</PanelTitle></div><div className="p-4 space-y-3"><ErrorNotice message={data.error || action.error} retry={data.reload} />{data.data && <form key={data.data.version} onSubmit={e => {
    e.preventDefault(); if (!data.isCurrent()) return; const form = new FormData(e.currentTarget)
    void action.run(() => api(`${root}/mail`, 'PUT', { enabled: form.get('enabled') === 'on', reminders_enabled: form.get('reminders_enabled') === 'on', host: form.get('host'), port: Number(form.get('port')), security: form.get('security'), username: form.get('username'), from_address: form.get('from_address'), credential_id: form.get('credential_id') || null, blocked_domains: String(form.get('blocked_domains') ?? '').split(/[\s,]+/).filter(Boolean).map(s => s.toLowerCase()), version: data.data!.version }), data.reload)
  }}><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox  name="enabled" defaultChecked={data.data.enabled} />启用 SMTP 邮件与邮箱验证</label><Field label="SMTP 主机"><Input name="host" maxLength={253} defaultValue={data.data.host} placeholder="smtp.example.com" /></Field><Field label="端口"><Input type="number" name="port" min={1} max={65535} required defaultValue={data.data.port} /></Field><Field label="加密方式"><NativeSelect name="security" defaultValue={data.data.security}><option value="tls">SMTPS（TLS）</option><option value="starttls">强制 STARTTLS</option></NativeSelect></Field><Field label="SMTP 用户名"><Input name="username" maxLength={254} defaultValue={data.data.username} autoComplete="off" /></Field><Field label="SMTP 密码凭据引用"><Input name="credential_id" defaultValue={data.data.credential_id ?? ''} placeholder="凭据 UUID" /></Field><p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">在管理与安全的凭据库中创建 external-api 凭据，key 字段填写 SMTP 密码。</p><Field label="发件邮箱"><Input name="from_address" type="email" maxLength={254} defaultValue={data.data.from_address} /></Field><Field label="不投递的邮箱域名"><Textarea name="blocked_domains" defaultValue={data.data.blocked_domains.join('\n')} placeholder="每行一个域名，可留空" /></Field><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox  name="reminders_enabled" defaultChecked={data.data.reminders_enabled} />通过邮件发送套餐到期、流量和邀请人提醒</label><p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">仅向已验证且未关闭相应提醒的成员发信。保存配置不会发送邮件。</p><Button  disabled={action.busy || !data.isCurrent()}>保存邮件配置</Button></form>}</div></Panel>
}
