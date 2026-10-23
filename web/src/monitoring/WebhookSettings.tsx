import { useState } from 'react'
import { api } from '../api'
import { Badge, ErrorNotice, Field, Loading } from '../components'
import { useAction, useResource } from '../hooks'
import { previewWebhook, webhookPresets, type Preset } from './webhook-presets'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Switch } from '@/components/ui/switch'
import { Textarea } from '@/components/ui/textarea'
import { Panel } from '@/components/panel'

type Summary = { enabled: boolean; preset: Preset; url_configured: boolean; headers_configured: boolean; body_configured: boolean }
const changed = () => window.dispatchEvent(new Event('apsp:notifications-updated'))
function Form({ initial }: { initial: Summary }) {
  const [saved, setSaved] = useState(initial), [enabled, setEnabled] = useState(initial.enabled), [preset, setPreset] = useState(initial.preset)
  const [url, setUrl] = useState(''), [headers, setHeaders] = useState(''), [body, setBody] = useState(initial.body_configured ? '' : webhookPresets[0].body), [clearHeaders, setClearHeaders] = useState(false)
  const [dirty, setDirty] = useState(false), [notice, setNotice] = useState(''), [removing, setRemoving] = useState(false)
  const save = useAction(), test = useAction(), remove = useAction(), busy = save.busy || test.busy || remove.busy
  const selected = webhookPresets.find(value => value.id === preset) ?? webhookPresets[0]
  const mark = () => { setDirty(true); setNotice(''); setRemoving(false); save.clearError(); test.clearError(); remove.clearError() }
  const apply = (value: Summary) => { setSaved(value); setEnabled(value.enabled); setPreset(value.preset); setUrl(''); setHeaders(''); setBody(''); setClearHeaders(false); setDirty(false); setRemoving(false); changed() }
  return <Panel aria-labelledby="webhook-title"><div className="p-4"><h2 className="mt-6 mb-3 text-base font-semibold" id="webhook-title">Webhook 通知 <Badge tone={saved.url_configured ? 'good' : 'neutral'}>{saved.url_configured ? '已配置' : '未配置'}</Badge></h2>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">可与 Telegram 同时启用。两个渠道独立重试，成功的渠道不会因另一渠道失败而重发。</p>
    <form onSubmit={event => { event.preventDefault(); void save.run(() => api<Summary>('/api/notifications/webhook', 'PATCH', { enabled, preset, url, headers, body, clear_headers: clearHeaders }), value => { apply(value); setNotice('Webhook 设置已保存。') }) }}><fieldset disabled={busy}>
      <label className="relative flex cursor-pointer items-center gap-4 border-b py-3.5 last:border-0 last:pb-0"><span className="min-w-0 flex-1"><strong className="block text-sm font-medium">启用 Webhook</strong><small className="text-muted-foreground mt-1 block text-xs leading-relaxed">自动推送已启用的告警；总通知开关关闭时暂停自动发送。</small></span><Switch checked={enabled} onCheckedChange={checked => { setEnabled(checked); mark() }} /></label>
      <div className="mb-4 grid gap-4 sm:grid-cols-2"><Field label="通知服务预设" hint="更换预设会清空旧地址和认证信息，并载入新模板。"><NativeSelect className="w-full" value={preset} onChange={event => { const next = webhookPresets.find(value => value.id === event.target.value)!; setPreset(next.id); setUrl(next.url); setHeaders(next.headers); setBody(next.body); setClearHeaders(false); mark() }}>{webhookPresets.map(value => <option key={value.id} value={value.id}>{value.name}</option>)}</NativeSelect></Field>
        <Field label="Webhook 地址" hint={saved.url_configured && preset === saved.preset ? '已配置；留空保留原地址，保存后不回显。' : '输入 HTTP 或 HTTPS 地址，支持自建服务。'}><Input type="password" autoComplete="new-password" maxLength={2048} value={url} placeholder={saved.url_configured && preset === saved.preset ? '已配置，留空保持' : '输入通知地址'} onChange={event => { setUrl(event.target.value); mark() }} /></Field></div>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{selected.hint}</p>
      <Field label="Webhook 请求头（可选）" hint="每行一个“名称: 值”，最多 8 KiB。已配置时留空保留；保存后不回显。"><Textarea className="min-h-40 resize-y text-xs leading-relaxed" rows={3} maxLength={8192} value={headers} autoComplete="off" spellCheck={false} placeholder={saved.headers_configured && preset === saved.preset ? '已配置，留空保持' : '例如 Authorization: Bearer 令牌'} onChange={event => { setHeaders(event.target.value); setClearHeaders(false); mark() }} /></Field>
      {saved.headers_configured && preset === saved.preset && <label className="mb-4 flex items-center gap-2"><Checkbox checked={clearHeaders} onCheckedChange={checked => { setClearHeaders(checked === true); if (checked === true) setHeaders(''); mark() }} />清除已保存的请求头</label>}
      <Field label="Webhook JSON 模板" hint="最多 16 KiB。支持 {{title}}、{{server}}（或 {{node}}）、{{message}}、{{time}}、{{event}} 事件类型、{{event_id}} 编号、{{category}} 类别及 {{site}} 站点名称；占位符仅放在字符串值中。"><Textarea className="min-h-40 resize-y text-xs leading-relaxed" rows={9} maxLength={16384} value={body} autoComplete="off" spellCheck={false} placeholder={saved.body_configured && preset === saved.preset ? '模板已保存，留空保持；其中的密钥不会回显。' : '输入 JSON 模板'} onChange={event => { setBody(event.target.value); mark() }} /></Field>
      <Button type="button" variant="outline" onClick={() => { setBody(selected.body); mark() }}>载入所选预设模板</Button>
      {body && <details className="my-3.5 mb-5"><summary className="text-primary cursor-pointer">查看 Webhook 模板预览</summary><pre data-slot="webhook-preview" className="rounded-lg border p-4 font-sans whitespace-pre-wrap wrap-anywhere">{previewWebhook(body)}</pre><small className="text-muted-foreground text-xs">预览仅使用本页输入，不读取已保存的密钥或模板。</small></details>}
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">更换地址、请求头或模板会取消该渠道旧的待发送消息。保存不会发送通知；测试使用已保存配置，每个渠道至少间隔 30 秒。</p>
    </fieldset><ErrorNotice message={save.error || test.error || remove.error} />{notice && <p role="status">{notice}</p>}<div className="flex flex-wrap items-center gap-2.5 mt-4">
      <Button disabled={busy}>{save.busy ? '正在保存…' : '保存 Webhook'}</Button>
      <Button type="button" variant="outline" disabled={busy || dirty || !saved.url_configured} onClick={() => void test.run(async () => { try { return await api('/api/notifications/webhook/test', 'POST') } finally { changed() } }, () => setNotice('Webhook 服务已接受测试消息，请到接收端确认。'))}>{test.busy ? '正在发送…' : '测试 Webhook'}</Button>
      {saved.url_configured && <Button type="button" variant="outline" disabled={busy} onClick={() => setRemoving(true)}>删除 Webhook 配置</Button>}
      {dirty && <small className="text-muted-foreground text-xs">请先保存配置，再发送测试。</small>}
    </div>{removing && <div className="mt-4 rounded-lg border p-4" role="alert"><p>删除后将停用 Webhook 并取消此渠道的待发送消息，通知历史保留。</p><div className="flex flex-wrap items-center gap-2.5 mt-4"><Button type="button" variant="destructive" disabled={busy} onClick={() => void remove.run(() => api<Summary>('/api/notifications/webhook', 'DELETE'), value => { apply(value); setNotice('Webhook 配置已删除。') })}>确认删除 Webhook</Button><Button type="button" variant="outline" disabled={busy} onClick={() => setRemoving(false)}>返回</Button></div></div>}</form>
  </div></Panel>
}

export default function WebhookSettings() {
  const resource = useResource<Summary>('/api/notifications/webhook', 0)
  return <><ErrorNotice message={resource.error} retry={resource.reload} />{resource.data ? <Form initial={resource.data} /> : resource.loading ? <Loading /> : null}</>
}
