import { Field } from '../components'
import type { AssetDraft } from '../server-assets'

export default function ServerOperationsFields({ asset, onChange }: { asset: AssetDraft; onChange: (asset: AssetDraft) => void }) {
  return <section className="server-setup-section" aria-labelledby="setup-operations">
    <div className="server-setup-heading"><div><h3 id="setup-operations">告警与下载</h3><p>控制此服务器的离线通知与 Agent 下载方式。</p></div></div>
    <label className="server-setup-toggle"><span><strong>离线告警</strong><small>持续离线后生成站内告警；在「告警设置」中配置阈值和 Telegram。</small></span><input type="checkbox" role="switch" checked={asset.offline_notify} onChange={event => onChange({ ...asset, offline_notify: event.target.checked })} /><span className="server-setup-switch" aria-hidden="true" /></label>
    <Field label="Agent 下载加速" hint="留空直接从 GitHub Release 下载。填写 HTTPS 镜像前缀后，用于安装与自动更新，仍需通过签名校验。"><input type="url" maxLength={512} value={asset.agent_mirror} placeholder="https://mirror.example.com" onChange={event => onChange({ ...asset, agent_mirror: event.target.value })} /></Field>
  </section>
}
