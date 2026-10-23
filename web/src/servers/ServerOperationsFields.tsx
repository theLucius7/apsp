import { Field } from '../components'
import type { AssetDraft } from '../server-assets'
import { Input } from '@/components/ui/input'
import { Switch } from '@/components/ui/switch'

export default function ServerOperationsFields({ asset, onChange }: { asset: AssetDraft; onChange: (asset: AssetDraft) => void }) {
  return <section className="bg-card mb-4 scroll-mt-16 rounded-xl border p-5 last:mb-0 max-sm:p-4" aria-labelledby="setup-operations">
    <div className="mb-4 flex items-center justify-between gap-3.5 max-sm:flex-wrap"><div className="min-w-0"><h3 className="flex items-center gap-2 text-sm font-semibold" id="setup-operations">告警与下载</h3><p className="text-muted-foreground mt-1 text-xs leading-relaxed">控制此服务器的离线通知与 Agent 下载方式。</p></div></div>
    <label className="relative flex cursor-pointer items-center gap-4 border-b py-3.5 last:border-0 last:pb-0"><span className="min-w-0 flex-1"><strong className="block text-sm font-medium">离线告警</strong><small className="text-muted-foreground mt-1 block text-xs leading-relaxed">持续离线后生成站内告警；在「告警设置」中配置阈值和 Telegram。</small></span><Switch checked={asset.offline_notify} onCheckedChange={checked => onChange({ ...asset, offline_notify: checked })} /></label>
    <Field label="Agent 下载加速" hint="留空直接从 GitHub Release 下载。填写 HTTPS 镜像前缀后，用于安装与自动更新，仍需通过签名校验。"><Input type="url" maxLength={512} value={asset.agent_mirror} placeholder="https://mirror.example.com" onChange={event => onChange({ ...asset, agent_mirror: event.target.value })} /></Field>
  </section>
}
