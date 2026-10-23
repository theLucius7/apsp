import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Field, Loading, PageHeader } from '../components'
import { useAction, useResource } from '../hooks'
import type { Preferences } from '../system/panelSettings'
import AlertRules from './AlertRules'
import WebhookSettings from './WebhookSettings'
import NotificationChannels from './NotificationChannels'
import '../servers/server-setup.css'
import './monitoring.css'

const defaultTemplate = 'apsp · {{title}}\n服务器：{{server}}\n{{message}}\n时间：{{time}}\n事件：{{event}}'
const examples: Record<string, string> = { title: '资源超限告警', server: '示例服务器', message: '处理器使用率达到设定阈值。', time: '示例时间（UTC）', event: '示例事件' }
function Form({ initial }: { initial: Preferences }) {
  const [value, setValue] = useState<Preferences>({ ...initial, notification_enabled: initial.notification_enabled ?? true, expiry_alert_days: initial.expiry_alert_days ?? 0, traffic_alert_percentage: initial.traffic_alert_percentage ?? 0, telegram_thread_id: initial.telegram_thread_id ?? null, telegram_template: initial.telegram_template ?? defaultTemplate })
  const [token, setToken] = useState(''), [clearToken, setClearToken] = useState(false), [saved, setSaved] = useState(false), [dirty, setDirty] = useState(false), [tested, setTested] = useState(false)
  const action = useAction(), test = useAction(), busy = action.busy || test.busy
  const change = (next: Partial<Preferences>) => { setValue(current => ({ ...current, ...next })); setSaved(false); setDirty(true); setTested(false) }
  const save = () => action.run(async () => {
    const { telegram_token_configured: _, ...settings } = value
    // The public dashboard switch belongs to the panel settings page; keep its latest value.
    const { public_dashboard } = await api<Preferences>('/api/settings')
    return api<Preferences>('/api/settings', 'PATCH', { ...settings, public_dashboard, telegram_thread_id: settings.telegram_thread_id ?? 0, ...(clearToken ? { telegram_token: '' } : token ? { telegram_token: token } : {}) })
  }, result => { setValue(result); setToken(''); setClearToken(false); setSaved(true); setDirty(false); setTested(false); window.dispatchEvent(new Event('apsp:notifications-updated')) })
  return <form className="panel monitoring-settings" onSubmit={event => { event.preventDefault(); void save() }}><div className="panel-body"><fieldset disabled={busy}>
    <h2>告警开关与阈值</h2>
    <label className="server-setup-toggle"><span><strong>启用通知与告警</strong><small>控制站内事件和自动通知。关闭会结束当前事件并取消待发送消息，历史记录保留。</small></span><input role="switch" type="checkbox" checked={value.notification_enabled} onChange={event => change({ notification_enabled: event.target.checked })} /><span className="server-setup-switch" aria-hidden="true" /></label>
    <label className="server-setup-toggle"><span><strong>启用离线告警</strong><small>已上报过的服务器持续离线达到阈值时告警，重新上线后记录恢复；可在服务器设置中单独关闭。</small></span><input role="switch" type="checkbox" checked={value.offline_alerts} onChange={event => change({ offline_alerts: event.target.checked })} /><span className="server-setup-switch" aria-hidden="true" /></label>
    <div className="form-grid"><Field label="离线告警阈值（分钟）" hint="2–1440 分钟。面板重启后有同等时长的宽限期。"><input required type="number" min={2} max={1440} step={1} value={value.offline_minutes} onChange={event => change({ offline_minutes: Number(event.target.value) })} /></Field><Field label="到期提前提醒（天）" hint="0 表示关闭，最多 365 天；每个到期日期提醒一次，含过期后 7 天内的补提醒。"><input required type="number" min={0} max={365} step={1} value={value.expiry_alert_days} onChange={event => change({ expiry_alert_days: Number(event.target.value) })} /></Field><Field label="流量提醒起始阈值（%）" hint="0 表示关闭，启用时为 50–100。达到起始线后每增加 5 个百分点提醒一次，最多到 100%。"><input required type="number" min={0} max={100} step={1} value={value.traffic_alert_percentage} onChange={event => change({ traffic_alert_percentage: Number(event.target.value) })} /></Field></div>
    <p className="helper">流量提醒按服务器的账单周期、网卡范围及计量方向计算，使用矫正后的值。提醒不会执行续费付款或停用服务器。</p>
    <h2>Telegram 通知</h2>
    <label className="server-setup-toggle"><span><strong>Telegram 通知</strong><small>将已启用告警发送到指定会话；发送失败时自动重试，离线与资源超限恢复后也会通知。</small></span><input role="switch" type="checkbox" checked={value.telegram_enabled} onChange={event => change({ telegram_enabled: event.target.checked })} /><span className="server-setup-switch" aria-hidden="true" /></label>
    <div className="server-setup-grid"><Field label="机器人令牌" hint={value.telegram_token_configured ? '已配置；留空保留原令牌。' : '通过 BotFather 创建机器人并获取令牌。'}><input type="password" autoComplete="new-password" maxLength={256} value={token} onChange={event => { setToken(event.target.value); setClearToken(false); change({}) }} placeholder={value.telegram_token_configured ? '已配置，留空保持' : '输入机器人令牌'} /></Field><Field label="会话 ID" hint="支持数字会话 ID 或公开频道 @名称，需先向机器人发送消息或将其加入群组。"><input maxLength={128} value={value.telegram_chat_id} onChange={event => change({ telegram_chat_id: event.target.value })} /></Field><Field label="话题 ID（可选）" hint="用于发送到指定群组话题；留空发送到默认会话。"><input type="number" min={1} max={2147483647} step={1} value={value.telegram_thread_id ?? ''} onChange={event => change({ telegram_thread_id: event.target.value ? Number(event.target.value) : null })} /></Field></div>
    {value.telegram_token_configured && <label><input type="checkbox" checked={clearToken} onChange={event => { setClearToken(event.target.checked); if (event.target.checked) { setToken(''); change({ telegram_enabled: false }) } else change({}) }} />清除已保存的机器人令牌</label>}
    <Field label="消息模板" hint="纯文本；支持 {{title}} 标题、{{server}} 服务器、{{message}} 内容、{{time}} UTC 时间和 {{event}} 事件编号。最多 4000 字符，替换后的消息超长时截断。"><textarea required rows={6} maxLength={4000} value={value.telegram_template} onChange={event => change({ telegram_template: event.target.value })} /></Field>
    <details><summary>查看模板预览</summary><pre className="monitoring-preview">{value.telegram_template.replace(/\{\{(title|server|message|time|event)\}\}/g, (_, key: string) => examples[key])}</pre></details>
  </fieldset><ErrorNotice message={action.error || test.error} />{saved && <p role="status">设置已保存。</p>}{tested && <p role="status">Telegram 已接受测试消息，请到对应会话确认。</p>}<div className="monitoring-actions"><button className="button button-primary" disabled={busy}>{action.busy ? '正在保存…' : '保存设置'}</button><button type="button" className="button button-secondary" disabled={busy || dirty || !value.telegram_token_configured || !value.telegram_chat_id} onClick={() => void test.run(async () => { try { return await api('/api/notifications/telegram/test', 'POST') } finally { window.dispatchEvent(new Event('apsp:notifications-updated')) } }, () => setTested(true))}>{test.busy ? '正在发送…' : '发送测试通知'}</button>{dirty && <small className="subtle">请先保存配置，再发送测试。</small>}</div></div></form>
}

export default function AlertSettings() {
  const resource = useResource<Preferences>('/api/settings', 0)
  return <>
    <PageHeader eyebrow="监控告警" title="告警设置" description="决定哪些情况会产生告警，以及告警通过 Telegram 或 Webhook 发给谁。">
      <a className="button button-secondary" href="#/system/notifications">查看告警事件</a>
    </PageHeader>
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.data ? <><Form initial={resource.data} /><AlertRules /><WebhookSettings /><NotificationChannels /></> : resource.loading ? <Loading /> : null}
  </>
}
