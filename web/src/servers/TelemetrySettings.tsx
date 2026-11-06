import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Field, Loading } from '../components'
import { useAction, useResource } from '../hooks'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

type Settings = { persist_interval_secs: number }

function Form({ serverId, initial }: { serverId: number; initial: Settings }) {
  const [interval, setInterval] = useState(String(initial.persist_interval_secs))
  const [saved, setSaved] = useState(false)
  const action = useAction()
  return <form onSubmit={event => {
    event.preventDefault()
    void action.run(() => api<Settings>(`/api/servers/${serverId}/telemetry-settings`, 'PATCH', { persist_interval_secs: Number(interval) }), value => { setInterval(String(value.persist_interval_secs)); setSaved(true) })
  }}><fieldset disabled={action.busy}>
    <Field label="历史批量写入间隔（秒）" hint="15–3600 秒，默认 60 秒；采样在 Agent 本地暂存，面板确认后删除。"><Input type="number" required min={15} max={3600} step={1} value={interval} onChange={event => { setInterval(event.target.value); setSaved(false) }} /></Field>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">需要新版 Agent；旧设备按原上传间隔写入。历史粒度和保留天数在「面板设置」中调整。</p>
    <ErrorNotice message={action.error} />
    <Button variant="outline" disabled={action.busy}>{action.busy ? '保存中…' : '保存历史写入间隔'}</Button>
    {saved && <p role="status">已保存，设备在线后会自动同步。</p>}
  </fieldset></form>
}

export default function TelemetrySettings({ serverId, liveSupported }: { serverId: number; liveSupported: boolean }) {
  const resource = useResource<Settings>(`/api/servers/${serverId}/telemetry-settings`, 0)
  return <Panel><PanelHeader><PanelTitle>数据上报与保存</PanelTitle></PanelHeader><div className="p-4">
    {!liveSupported && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">这台设备的 Agent 还不支持单独设置实时上报，升级后生效。</p>}
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.data ? <Form key={serverId} serverId={serverId} initial={resource.data} /> : resource.loading ? <Loading /> : null}
  </div></Panel>
}
