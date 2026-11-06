import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Loading, PageHeader, SettingRow } from '../components'
import { useAction, useResource } from '../hooks'
import type { Preferences } from './panelSettings'
import TelemetryPolicy from './TelemetryPolicy'
import ExchangeRateSettings from './ExchangeRateSettings'
import { buttonVariants } from '@/components/ui/button'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { Switch } from '@/components/ui/switch'
import { useFeatureStore } from '@/stores/features'

type Switches = { public_dashboard: boolean; show_frozen_features: boolean }

/**
 * The panel's two switches. Each applies as soon as it is flipped: a switch is
 * its own confirmation, so a separate save button only added a step. A failed
 * save puts the switch back and says why.
 */
function PanelSwitches({ initial }: { initial: Switches }) {
  const [values, setValues] = useState(initial)
  const [saved, setSaved] = useState<keyof Switches | null>(null)
  const action = useAction()
  const change = (key: keyof Switches, value: boolean) => {
    const previous = values
    let applied = false
    setValues({ ...values, [key]: value }); setSaved(null)
    void action.run(() => api<Preferences>('/api/settings', 'PATCH', { [key]: value }), result => {
      applied = true
      const next = { public_dashboard: result.public_dashboard, show_frozen_features: Boolean(result.show_frozen_features) }
      setValues(next); useFeatureStore.getState().setShowFrozen(next.show_frozen_features); setSaved(key)
    }).then(() => { if (!applied) setValues(previous) })
  }
  return <Panel data-slot="panel-switches"><PanelHeader><PanelTitle>常规</PanelTitle>{saved && <span role="status" className="text-muted-foreground text-xs">设置已保存</span>}</PanelHeader><div className="px-4 pb-1"><fieldset disabled={action.busy}>
    <SettingRow title="公开服务器看板" description="访客无需登录即可查看未隐藏服务器的状态与拨测；管理仍需登录。"><Switch checked={values.public_dashboard} onCheckedChange={checked => change('public_dashboard', checked)} /></SettingRow>
    <SettingRow title="显示实验与冻结功能" description="显示自动化与维护、网络配置、服务器运维等实验与冻结功能；关闭时数据与地址保留。"><Switch checked={values.show_frozen_features} onCheckedChange={checked => change('show_frozen_features', checked)} /></SettingRow>
  </fieldset><ErrorNotice message={action.error} /></div></Panel>
}

export default function Settings() {
  const resource = useResource<Preferences>('/api/settings', 0)
  return <>
    <PageHeader title="面板设置">
      <a className={buttonVariants({ variant: 'outline' })} href="#/alerts/settings">告警设置</a>
    </PageHeader>
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.data ? <PanelSwitches initial={{ public_dashboard: resource.data.public_dashboard, show_frozen_features: Boolean(resource.data.show_frozen_features) }} /> : resource.loading ? <Loading /> : null}
    <ExchangeRateSettings />
    <TelemetryPolicy />
  </>
}
