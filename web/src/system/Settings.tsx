import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Loading, PageHeader } from '../components'
import { useAction, useResource } from '../hooks'
import type { Preferences } from './panelSettings'
import TelemetryPolicy from './TelemetryPolicy'
import ExchangeRateSettings from './ExchangeRateSettings'
import { Button, buttonVariants } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'

function DashboardAccess({ initial }: { initial: boolean }) {
  const [enabled, setEnabled] = useState(initial), [saved, setSaved] = useState(false), [dirty, setDirty] = useState(false)
  const action = useAction()
  // Only the public dashboard switch changes here; alert settings keep their latest values.
  const save = () => action.run(async () => {
    const latest = await api<Preferences>('/api/settings')
    return api<Preferences>('/api/settings', 'PATCH', { public_dashboard: enabled, offline_alerts: latest.offline_alerts, offline_minutes: latest.offline_minutes, telegram_enabled: latest.telegram_enabled, telegram_chat_id: latest.telegram_chat_id })
  }, result => { setEnabled(result.public_dashboard); setSaved(true); setDirty(false) })
  return <form data-slot="monitoring-settings" className="bg-card text-card-foreground ring-foreground/10 mb-4 min-w-0 overflow-hidden rounded-xl ring-1" onSubmit={event => { event.preventDefault(); void save() }}><div className="p-4"><fieldset disabled={action.busy}>
    <h2 className="mb-3 text-base font-semibold">看板访问</h2>
    <label className="relative flex cursor-pointer items-center gap-4 py-3.5"><span className="min-w-0 flex-1"><strong className="block text-sm font-medium">公开服务器看板</strong><small className="text-muted-foreground mt-1 block text-xs leading-relaxed">开启后，无需登录即可查看未隐藏服务器的运行状态和拨测数据。管理操作仍需登录。</small></span><Switch checked={enabled} onCheckedChange={checked => { setEnabled(checked); setSaved(false); setDirty(true) }} /></label>
  </fieldset><ErrorNotice message={action.error} />{saved && <p role="status" className="text-sm">设置已保存。</p>}<div className="mt-4 flex flex-wrap items-center gap-2.5"><Button disabled={action.busy || !dirty}>{action.busy ? '正在保存…' : '保存设置'}</Button></div></div></form>
}

export default function Settings() {
  const resource = useResource<Preferences>('/api/settings', 0)
  return <>
    <PageHeader eyebrow="系统" title="面板设置" description="面板本身的全局设置：看板是否公开、金额按哪种币种显示、监控历史保存多久。">
      <a className={buttonVariants({ variant: 'outline' })} href="#/monitoring/settings">告警设置</a>
    </PageHeader>
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.data ? <DashboardAccess initial={resource.data.public_dashboard} /> : resource.loading ? <Loading /> : null}
    <ExchangeRateSettings />
    <TelemetryPolicy />
  </>
}
