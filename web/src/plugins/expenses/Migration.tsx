import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { useState } from 'react'
import { api } from '../../api'
import { ErrorNotice } from '../../components'
import { useAction } from '../../hooks'
import { download, root } from './types'

type Preview = { digest: string; already_imported: boolean; subscriptions?: number; labels?: number; warnings?: string[]; receipt?: unknown }
export default function Migration() {
  const action = useAction(), [snapshot, setSnapshot] = useState<unknown>(), [preview, setPreview] = useState<Preview>(), [result, setResult] = useState<unknown>(), [confirmed, setConfirmed] = useState(false)
  return <Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>导入与导出</PanelTitle><Button variant="ghost" size="sm" disabled={action.busy} onClick={() => void action.run(() => download(`${root}/export`, 'apsp-expenses.json'))}>导出全部配置</Button></div><div className="p-4 space-y-3"><p>可导入 apsp 备份，或使用随仓库提供的 Wallos 只读导出工具生成配置文件。原始数据库和凭据无需上传。</p><p className="text-muted-foreground mt-1 text-xs leading-relaxed whitespace-pre-wrap wrap-anywhere">同一来源与内容重复导入不会新增重复项目。导入后默认关闭全局提醒，请核对配置后再开启。</p><label>选择配置文件<Input type="file" accept="application/json,.json" disabled={action.busy} onChange={e => {
    const file = e.target.files?.[0]; setSnapshot(undefined); setPreview(undefined); setResult(undefined); setConfirmed(false)
    if (file) void action.run(async () => { if (file.size > 8 * 1024 * 1024) throw new Error('配置文件不能超过 8 MiB。'); const snapshot: unknown = JSON.parse(await file.text()); const value = await api<Preview>(`${root}/import/preview`, 'POST', snapshot); setSnapshot(snapshot); setPreview(value) })
  }} /></label><ErrorNotice message={action.error} />{preview && <><p>{preview.already_imported ? '此文件已经导入。' : `将新增 ${preview.subscriptions} 项订阅，映射 ${preview.labels} 个配置项。`}</p>{preview.warnings?.map(w => <p key={w}>{w}</p>)}{!preview.already_imported && <><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox  checked={confirmed} onCheckedChange={value => setConfirmed((value === true))} />已核对预览，确认添加配置并关闭全局提醒</label><Button  disabled={action.busy || !confirmed || Boolean(result)} onClick={() => void action.run(async () => { setResult(await api(`${root}/import`, 'POST', { snapshot, confirmed_digest: preview.digest })) })}>导入配置</Button></>}</>}{Boolean(result) && <p role="status">导入完成。请在订阅清单核对日期、金额和分类。</p>}</div></Panel>
}
