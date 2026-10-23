import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Field, Modal, SettingRow } from '../components'
import { useAction } from '../hooks'
import { assetDraft, assetPayload } from '../server-assets'
import type { Server } from '../types'
import ServerAssetFields, { SetupNavigation } from './ServerAssetFields'

import ServerOperationsFields from './ServerOperationsFields'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'

export default function ServerEdit({ server, onClose, onSaved }: { server: Server; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState(server.name)
  const [asset, setAsset] = useState(() => assetDraft(server.asset_settings))
  const [autoUpdate, setAutoUpdate] = useState(server.agent_settings?.auto_update ?? false)
  const action = useAction()
  return <Modal title="编辑服务器" onClose={onClose} busy={action.busy} className="sm:max-w-[52.5rem]"><form onSubmit={event => { event.preventDefault(); void action.run(() => api(`/api/servers/${server.id}`, 'PATCH', { name: name.trim(), asset_settings: assetPayload(asset), auto_update: autoUpdate }), onSaved) }}>
    <SetupNavigation />
    <div className="bg-muted/40 px-4 py-5 sm:px-7 sm:py-6"><fieldset disabled={action.busy}>
      <section className="bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4" id="setup-basics"><Field className="mb-0" label="服务器名称"><Input required pattern=".*\S.*" maxLength={128} value={name} onChange={event => setName(event.target.value)} /></Field></section>
      <section className="bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4"><SettingRow title="自动更新 Agent" description="从 GitHub 获取面板选定的兼容签名版本；下载失败时保留现有版本。"><Switch checked={autoUpdate} onCheckedChange={checked => setAutoUpdate(checked)} /></SettingRow></section>
      <ServerAssetFields value={asset} onChange={setAsset} />
      <ServerOperationsFields asset={asset} onChange={setAsset} />
    </fieldset><ErrorNotice message={action.error} /></div>
    <footer className="bg-popover sticky bottom-0 z-[1] flex flex-wrap items-center justify-end gap-2 border-t px-4 py-3 sm:px-7"><span className="text-muted-foreground min-w-32 flex-1 text-xs max-sm:mb-1 max-sm:basis-full">采样与拨测可在服务器详情中调整</span><Button type="button" variant="outline" disabled={action.busy} onClick={onClose}>取消</Button><Button disabled={action.busy}>{action.busy ? '正在保存…' : '保存修改'}</Button></footer>
  </form></Modal>
}
