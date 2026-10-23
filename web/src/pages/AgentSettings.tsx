import { useEffect, useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Loading } from '../components'
import { useAction, useResource } from '../hooks'
import type { AgentSettings as Settings } from '../types'
import TelemetrySettings from './TelemetrySettings'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

export function AgentSettings({ serverId, liveSupported = false }: { serverId: number; liveSupported?: boolean }) {
  const resource = useResource<Settings>(`/api/servers/${serverId}/agent-settings`)
  const action = useAction()
  const [form, setForm] = useState<Settings | null>(null)
  const [saved, setSaved] = useState(false)
  useEffect(() => { setForm(null); setSaved(false) }, [serverId])
  useEffect(() => { if (!form && resource.data) setForm(resource.data) }, [form, resource.data])
  return <><Panel><PanelHeader><PanelTitle>Agent 设置</PanelTitle><span className="text-muted-foreground text-xs">设备在线后自动同步</span></PanelHeader><div className="p-4">
    <ErrorNotice message={resource.error || action.error} />
    {!form ? <Loading /> : <form onSubmit={event => { event.preventDefault(); void action.run(() => api<Settings>(`/api/servers/${serverId}/agent-settings`, 'PATCH', form), value => { setForm(value); setSaved(true); resource.reload() }) }}>
      <div className="grid gap-x-4 sm:grid-cols-2"><label className="grid min-w-0 gap-1.5 text-sm">采样间隔（秒）<Input type="number" min="1" max="60" required value={form.sample_interval_secs} onChange={event => { setSaved(false); setForm({ ...form, sample_interval_secs: Number(event.target.value) }) }} /></label>
      <label className="grid min-w-0 gap-1.5 text-sm">实时上报间隔（秒）<Input type="number" min={form.sample_interval_secs} max="60" required value={form.upload_interval_secs} onChange={event => { setSaved(false); setForm({ ...form, upload_interval_secs: Number(event.target.value) }) }} /></label></div>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">采样决定观测精度，实时上报决定看板何时收到新读数。历史批量写入单独配置；旧 Agent 仍按这里的上报间隔写入。</p>
      <label className="flex items-center gap-2 text-sm"><Checkbox checked={form.discover_public_ips} onCheckedChange={checked => { setSaved(false); setForm({ ...form, discover_public_ips: checked === true }) }} /> 自动识别公网 IPv4 / IPv6</label>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">公网识别使用固定的 icanhazip 接口；设备本地关闭该功能时，面板设置不会覆盖本地限制。</p>
      <label className="flex items-center gap-2 text-sm"><Checkbox checked={form.auto_update} onCheckedChange={checked => { setSaved(false); setForm({ ...form, auto_update: checked === true }) }} /> 自动更新 Agent</label>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">从 GitHub 下载面板选定的兼容签名 Agent，保留身份与账本。旧版 Agent 需先通过兼容的新签名接入入口手工迁移，之后才能从 GitHub 自动更新。可在编辑服务器中设置下载加速。</p>
      <Button disabled={action.busy} type="submit">{action.busy ? '保存中…' : '保存设置'}</Button>{saved && <span className="text-muted-foreground text-xs"> 已保存，设备会在一分钟内同步。</span>}
    </form>}
  </div></Panel><TelemetrySettings serverId={serverId} liveSupported={liveSupported} /></>
}
