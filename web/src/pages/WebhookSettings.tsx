import { useState } from 'react'
import { api } from '../api'
import { Badge, ErrorNotice, Field, Loading } from '../components'
import { useAction, useResource } from '../hooks'
import { previewWebhook, webhookPresets, type Preset } from './webhook-presets'

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
  return <section className="panel monitoring-settings" aria-labelledby="webhook-title"><div className="panel-body"><h2 id="webhook-title">Webhook 通知 <Badge tone={saved.url_configured ? 'good' : 'neutral'}>{saved.url_configured ? '已配置' : '未配置'}</Badge></h2>
    <p className="helper">可与 Telegram 同时启用。两个渠道独立重试，成功的渠道不会因另一渠道失败而重发。</p>
    <form onSubmit={event => { event.preventDefault(); void save.run(() => api<Summary>('/api/notifications/webhook', 'PATCH', { enabled, preset, url, headers, body, clear_headers: clearHeaders }), value => { apply(value); setNotice('Webhook 设置已保存。') }) }}><fieldset disabled={busy}>
      <label className="server-setup-toggle"><span><strong>启用 Webhook</strong><small>自动推送已启用的告警；总通知开关关闭时暂停自动发送。</small></span><input role="switch" type="checkbox" checked={enabled} onChange={event => { setEnabled(event.target.checked); mark() }} /><span className="server-setup-switch" aria-hidden="true" /></label>
      <div className="form-grid"><Field label="通知服务预设" hint="更换预设会清空旧地址和认证信息，并载入新模板。"><select value={preset} onChange={event => { const next = webhookPresets.find(value => value.id === event.target.value)!; setPreset(next.id); setUrl(next.url); setHeaders(next.headers); setBody(next.body); setClearHeaders(false); mark() }}>{webhookPresets.map(value => <option key={value.id} value={value.id}>{value.name}</option>)}</select></Field>
        <Field label="Webhook 地址" hint={saved.url_configured && preset === saved.preset ? '已配置；留空保留原地址，保存后不回显。' : '输入 HTTP 或 HTTPS 地址，支持自建服务。'}><input type="password" autoComplete="new-password" maxLength={2048} value={url} placeholder={saved.url_configured && preset === saved.preset ? '已配置，留空保持' : '输入通知地址'} onChange={event => { setUrl(event.target.value); mark() }} /></Field></div>
      <p className="helper">{selected.hint}</p>
      <Field label="Webhook 请求头（可选）" hint="每行一个“名称: 值”，最多 8 KiB。已配置时留空保留；保存后不回显。"><textarea rows={3} maxLength={8192} value={headers} autoComplete="off" spellCheck={false} placeholder={saved.headers_configured && preset === saved.preset ? '已配置，留空保持' : '例如 Authorization: Bearer 令牌'} onChange={event => { setHeaders(event.target.value); setClearHeaders(false); mark() }} /></Field>
      {saved.headers_configured && preset === saved.preset && <label className="notification-inline"><input type="checkbox" checked={clearHeaders} onChange={event => { setClearHeaders(event.target.checked); if (event.target.checked) setHeaders(''); mark() }} />清除已保存的请求头</label>}
      <Field label="Webhook JSON 模板" hint="最多 16 KiB。支持 {{title}}、{{server}}（或 {{node}}）、{{message}}、{{time}}、{{event}} 事件类型、{{event_id}} 编号、{{category}} 类别及 {{site}} 站点名称；占位符仅放在字符串值中。"><textarea rows={9} maxLength={16384} value={body} autoComplete="off" spellCheck={false} placeholder={saved.body_configured && preset === saved.preset ? '模板已保存，留空保持；其中的密钥不会回显。' : '输入 JSON 模板'} onChange={event => { setBody(event.target.value); mark() }} /></Field>
      <button type="button" className="button button-secondary" onClick={() => { setBody(selected.body); mark() }}>载入所选预设模板</button>
      {body && <details><summary>查看 Webhook 模板预览</summary><pre className="monitoring-preview">{previewWebhook(body)}</pre><small className="subtle">预览仅使用本页输入，不读取已保存的密钥或模板。</small></details>}
      <p className="helper">更换地址、请求头或模板会取消该渠道旧的待发送消息。保存不会发送通知；测试使用已保存配置，每个渠道至少间隔 30 秒。</p>
    </fieldset><ErrorNotice message={save.error || test.error || remove.error} />{notice && <p role="status">{notice}</p>}<div className="monitoring-actions">
      <button className="button button-primary" disabled={busy}>{save.busy ? '正在保存…' : '保存 Webhook'}</button>
      <button type="button" className="button button-secondary" disabled={busy || dirty || !saved.url_configured} onClick={() => void test.run(async () => { try { return await api('/api/notifications/webhook/test', 'POST') } finally { changed() } }, () => setNotice('Webhook 服务已接受测试消息，请到接收端确认。'))}>{test.busy ? '正在发送…' : '测试 Webhook'}</button>
      {saved.url_configured && <button type="button" className="button button-secondary" disabled={busy} onClick={() => setRemoving(true)}>删除 Webhook 配置</button>}
      {dirty && <small className="subtle">请先保存配置，再发送测试。</small>}
    </div>{removing && <div className="notification-confirm" role="alert"><p>删除后将停用 Webhook 并取消此渠道的待发送消息，通知历史保留。</p><div className="monitoring-actions"><button type="button" className="button button-danger" disabled={busy} onClick={() => void remove.run(() => api<Summary>('/api/notifications/webhook', 'DELETE'), value => { apply(value); setNotice('Webhook 配置已删除。') })}>确认删除 Webhook</button><button type="button" className="button button-secondary" disabled={busy} onClick={() => setRemoving(false)}>返回</button></div></div>}</form>
  </div></section>
}

export default function WebhookSettings() {
  const resource = useResource<Summary>('/api/notifications/webhook', 0)
  return <><ErrorNotice message={resource.error} retry={resource.reload} />{resource.data ? <Form initial={resource.data} /> : resource.loading ? <Loading /> : null}</>
}
