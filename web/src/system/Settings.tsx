import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Loading, PageHeader } from '../components'
import { useAction, useResource } from '../hooks'
import type { Preferences } from './panelSettings'
import TelemetryPolicy from './TelemetryPolicy'
import ExchangeRateSettings from './ExchangeRateSettings'
import '../servers/server-setup.css'
import '../monitoring/monitoring.css'

function DashboardAccess({ initial }: { initial: boolean }) {
  const [enabled, setEnabled] = useState(initial), [saved, setSaved] = useState(false), [dirty, setDirty] = useState(false)
  const action = useAction()
  // Only the public dashboard switch changes here; alert settings keep their latest values.
  const save = () => action.run(async () => {
    const latest = await api<Preferences>('/api/settings')
    return api<Preferences>('/api/settings', 'PATCH', { public_dashboard: enabled, offline_alerts: latest.offline_alerts, offline_minutes: latest.offline_minutes, telegram_enabled: latest.telegram_enabled, telegram_chat_id: latest.telegram_chat_id })
  }, result => { setEnabled(result.public_dashboard); setSaved(true); setDirty(false) })
  return <form className="panel monitoring-settings" onSubmit={event => { event.preventDefault(); void save() }}><div className="panel-body"><fieldset disabled={action.busy}>
    <h2>看板访问</h2>
    <label className="server-setup-toggle"><span><strong>公开服务器看板</strong><small>开启后，无需登录即可查看未隐藏服务器的运行状态和拨测数据。管理操作仍需登录。</small></span><input role="switch" type="checkbox" checked={enabled} onChange={event => { setEnabled(event.target.checked); setSaved(false); setDirty(true) }} /><span className="server-setup-switch" aria-hidden="true" /></label>
  </fieldset><ErrorNotice message={action.error} />{saved && <p role="status">设置已保存。</p>}<div className="monitoring-actions"><button className="button button-primary" disabled={action.busy || !dirty}>{action.busy ? '正在保存…' : '保存设置'}</button></div></div></form>
}

export default function Settings() {
  const resource = useResource<Preferences>('/api/settings', 0)
  return <>
    <PageHeader eyebrow="系统" title="面板设置" description="面板本身的全局设置：看板是否公开、金额按哪种币种显示、监控历史保存多久。">
      <a className="button button-secondary" href="#/monitoring/settings">告警设置</a>
    </PageHeader>
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.data ? <DashboardAccess initial={resource.data.public_dashboard} /> : resource.loading ? <Loading /> : null}
    <ExchangeRateSettings />
    <TelemetryPolicy />
  </>
}
