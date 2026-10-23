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
    <Field label="历史批量写入间隔（秒）" hint="15–3600 秒，默认 60 秒。独立于实时上报频率，采样先保存在 Agent 本地，收到面板持久化确认后才删除。"><Input type="number" required min={15} max={3600} step={1} value={interval} onChange={event => { setInterval(event.target.value); setSaved(false) }} /></Field>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">需要更新后的 Agent。旧设备继续按原上传间隔写入；面板重启时，等待下一次实时上报恢复当前读数。历史粒度和保留天数可在「面板设置」中查看、调整。</p>
    <ErrorNotice message={action.error} />
    <Button variant="outline" disabled={action.busy}>{action.busy ? '保存中…' : '保存历史写入间隔'}</Button>
    {saved && <p role="status">已保存，设备在线后会自动同步。</p>}
  </fieldset></form>
}

export default function TelemetrySettings({ serverId, liveSupported }: { serverId: number; liveSupported: boolean }) {
  const resource = useResource<Settings>(`/api/servers/${serverId}/telemetry-settings`, 0)
  return <Panel><PanelHeader><PanelTitle>数据上报与保存</PanelTitle></PanelHeader><div className="p-4">
    {!liveSupported && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">当前设备尚未声明独立实时上报能力。可以先保存历史写入配置，升级 Agent 并重新连接后生效。</p>}
    <ErrorNotice message={resource.error} retry={resource.reload} />
    {resource.data ? <Form key={serverId} serverId={serverId} initial={resource.data} /> : resource.loading ? <Loading /> : null}
  </div></Panel>
}
