import { useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Field, Modal } from '../components'
import { useAction } from '../hooks'
import { assetDraft, assetPayload } from '../server-assets'
import type { Server } from '../types'
import ServerAssetFields, { SetupNavigation } from './ServerAssetFields'
import './server-setup.css'
import ServerOperationsFields from './ServerOperationsFields'

export default function ServerEdit({ server, onClose, onSaved }: { server: Server; onClose: () => void; onSaved: () => void }) {
  const [name, setName] = useState(server.name)
  const [asset, setAsset] = useState(() => assetDraft(server.asset_settings))
  const [autoUpdate, setAutoUpdate] = useState(server.agent_settings?.auto_update ?? false)
  const action = useAction()
  return <Modal title="编辑服务器" onClose={onClose} busy={action.busy} className="server-setup-modal"><form onSubmit={event => { event.preventDefault(); void action.run(() => api(`/api/servers/${server.id}`, 'PATCH', { name: name.trim(), asset_settings: assetPayload(asset), auto_update: autoUpdate }), onSaved) }}>
    <SetupNavigation />
    <div className="server-setup-body"><fieldset disabled={action.busy}>
      <section className="server-setup-section" id="setup-basics"><Field label="服务器名称"><input required pattern=".*\S.*" maxLength={128} value={name} onChange={event => setName(event.target.value)} /></Field></section>
      <section className="server-setup-section"><label className="server-setup-toggle"><span><strong>自动更新 Agent</strong><small>从 GitHub 获取面板选定的兼容签名版本；下载失败时保留现有版本。</small></span><input type="checkbox" role="switch" checked={autoUpdate} onChange={event => setAutoUpdate(event.target.checked)} /><span className="server-setup-switch" aria-hidden="true" /></label></section>
      <ServerAssetFields value={asset} onChange={setAsset} />
      <ServerOperationsFields asset={asset} onChange={setAsset} />
    </fieldset><ErrorNotice message={action.error} /></div>
    <footer className="server-setup-footer"><span>采样与拨测可在服务器详情中调整</span><button type="button" className="button button-secondary" disabled={action.busy} onClick={onClose}>取消</button><button className="button button-primary" disabled={action.busy}>{action.busy ? '正在保存…' : '保存修改'}</button></footer>
  </form></Modal>
}
