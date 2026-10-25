import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Loading, PageHeader, SettingRow } from '../components'
import { useAction, useResource } from '../hooks'
import type { Preferences } from './panelSettings'
import TelemetryPolicy from './TelemetryPolicy'
import ExchangeRateSettings from './ExchangeRateSettings'
import { Button, buttonVariants } from '@/components/ui/button'
import { Switch } from '@/components/ui/switch'
import { useFeatureStore } from '@/stores/features'

function DashboardAccess({ initial }: { initial: boolean }) {
  const [enabled, setEnabled] = useState(initial), [saved, setSaved] = useState(false), [dirty, setDirty] = useState(false)
  const action = useAction()
  // Only the public dashboard switch changes here; the panel keeps every other setting.
  const save = () => action.run(() => api<Preferences>('/api/settings', 'PATCH', { public_dashboard: enabled }), result => { setEnabled(result.public_dashboard); setSaved(true); setDirty(false) })
  return <form data-slot="monitoring-settings" className="bg-card text-card-foreground ring-foreground/10 mb-4 min-w-0 overflow-hidden rounded-xl ring-1" onSubmit={event => { event.preventDefault(); void save() }}><div className="p-4"><fieldset disabled={action.busy}>
    <h2 className="mb-3 text-base font-semibold">看板访问</h2>
    <SettingRow title="公开服务器看板" description="开启后，无需登录即可查看未隐藏服务器的运行状态和拨测数据。管理操作仍需登录。"><Switch checked={enabled} onCheckedChange={checked => { setEnabled(checked); setSaved(false); setDirty(true) }} /></SettingRow>
  </fieldset><ErrorNotice message={action.error} />{saved && <p role="status" className="text-sm">设置已保存。</p>}<div className="mt-4 flex flex-wrap items-center gap-2.5"><Button disabled={action.busy || !dirty}>{action.busy ? '正在保存…' : '保存设置'}</Button></div></div></form>
}

// Frozen features only receive security fixes; this switch decides whether the
// panel still shows them. Experimental features always stay visible with a label.
function FeatureVisibility({ initial }: { initial: boolean }) {
  const [enabled, setEnabled] = useState(initial), [saved, setSaved] = useState(false), [dirty, setDirty] = useState(false)
  const action = useAction()
  const save = () => action.run(() => api<Preferences>('/api/settings', 'PATCH', { show_frozen_features: enabled }), result => { const shown = Boolean(result.show_frozen_features); setEnabled(shown); useFeatureStore.getState().setShowFrozen(shown); setSaved(true); setDirty(false) })
  return <form data-slot="feature-visibility" className="bg-card text-card-foreground ring-foreground/10 mb-4 min-w-0 overflow-hidden rounded-xl ring-1" onSubmit={event => { event.preventDefault(); void save() }}><div className="p-4"><fieldset disabled={action.busy}>
    <h2 className="mb-3 text-base font-semibold">功能显示</h2>
    <label className="relative flex cursor-pointer items-center gap-4 py-3.5"><span className="min-w-0 flex-1"><strong className="block text-sm font-medium">显示冻结功能</strong><small className="text-muted-foreground mt-1 block text-xs leading-relaxed">冻结功能不再投入，只修安全问题，默认隐藏：系统网络参数、反向隧道、私有组网、受管防火墙、云资源与费用，以及硬件测试与 Speedtest。开启后在原页面显示并标注“冻结”；已有数据和接口不受影响。标注“实验”的功能始终显示。</small></span><Switch checked={enabled} onCheckedChange={checked => { setEnabled(checked); setSaved(false); setDirty(true) }} /></label>
  </fieldset><ErrorNotice message={action.error} />{saved && <p role="status" className="text-sm">设置已保存。</p>}<div className="mt-4 flex flex-wrap items-center gap-2.5"><Button disabled={action.busy || !dirty}>{action.busy ? '正在保存…' : '保存功能显示'}</Button></div></div></form>
}

export default function Settings() {
  const resource = useResource<Preferences>('/api/settings', 0)
  return <>
    <PageHeader eyebrow="系统" title="面板设置" description="面板本身的全局设置：看板是否公开、显示哪些功能、金额按哪种币种显示、监控历史保存多久。">
      <a className={buttonVariants({ variant: 'outline' })} href="#/monitoring/settings">告警设置</a>
    </PageHeader>
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.data ? <><DashboardAccess initial={resource.data.public_dashboard} /><FeatureVisibility initial={Boolean(resource.data.show_frozen_features)} /></> : resource.loading ? <Loading /> : null}
    <ExchangeRateSettings />
    <TelemetryPolicy />
  </>
}
