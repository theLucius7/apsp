import LegacyHistory from './LegacyHistory'
import Compatibility from './Compatibility'
import { Panel, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Checkbox } from '@/components/ui/checkbox'
import { useState } from 'react'
import { api, errorMessage } from '../../../api'
import { ErrorNotice, Field } from '../../../components'
import { useAction } from '../../../hooks'
import { root } from './types'

type Preview = { digest: string; counts: Record<string, number | string>; conflicts: string[]; already_imported: boolean; remaining: string[] }
export default function Migration() {
  const [snapshot, setSnapshot] = useState<unknown>(), [preview, setPreview] = useState<Preview>(), [error, setError] = useState(''), [confirmed, setConfirmed] = useState(false), [result, setResult] = useState(''), action = useAction()
  const load = async (file: File | undefined) => {
    setPreview(undefined); setSnapshot(undefined); setConfirmed(false); setResult(''); setError('')
    if (!file) return
    try { if (file.size > 8 * 1024 * 1024) throw new Error('文件不能超过 8 MiB。'); setSnapshot(JSON.parse(await file.text())) } catch (e) { setError(errorMessage(e)) }
  }
  return <><Compatibility /><Panel><div className="flex min-h-14 flex-wrap items-center justify-between gap-3 border-b px-4 py-3"><PanelTitle>Nexus 迁移工作台</PanelTitle></div><div className="p-4 space-y-3">
    <p>使用仓库的只读导出工具生成迁移文件。文件含成员及订阅资料，请仅在可信设备和 HTTPS 面板中处理。</p><p>先预览编号、邮箱、余额和引用关系，确认后一次性导入。导入成员保持待迁移，销售套餐及公告指南先存为草稿；不会连接原数据库、切换节点或重新入账历史交易。</p>
    <Field label="Nexus 导出文件"><Input type="file" accept=".json,application/json" disabled={action.busy} onChange={e => void load(e.target.files?.[0])} /></Field>
    <ErrorNotice message={error || action.error} />
    <Button variant="outline" disabled={!snapshot || action.busy} onClick={() => void action.run(() => api<Preview>(`${root}/migration/preview`, 'POST', snapshot), value => { setPreview(value); setConfirmed(false) })}>预览导入</Button>
    {preview && <><dl className="my-4 grid gap-3 sm:grid-cols-2 [&_dt]:text-muted-foreground [&_dt]:text-xs [&_dd]:font-medium">{Object.entries(preview.counts).map(([key, value]) => <div key={key}><dt>{({ members: '成员', plans: '原套餐', history: '历史记录', content: '公告指南', opening_balance_cents: '期初余额合计（分）' } as Record<string, string>)[key] ?? key}</dt><dd>{value}</dd></div>)}</dl>
      {preview.conflicts.map(c => <p className="my-3 rounded-lg border border-destructive/25 bg-destructive/5 p-3 text-sm text-destructive" key={c}>{c}</p>)}
      {preview.already_imported ? <p className="my-3 rounded-lg border border-success/25 bg-success/5 p-3 text-sm">此快照已导入，再次提交不会重复创建或入账。</p> : <><label className="my-3 flex items-center gap-2.5 text-sm"><Checkbox  checked={confirmed} onCheckedChange={value => setConfirmed((value === true))} />确认以待迁移状态导入，后续再核对授权和计费</label><Button  disabled={!confirmed || preview.conflicts.length > 0 || action.busy} onClick={() => void action.run(() => api(`${root}/migration/import`, 'POST', { snapshot, expected_digest: preview.digest, confirm_staged: true }), () => { setResult('导入完成，成员保持待迁移状态。'); setConfirmed(false) })}>确认导入</Button></>}
      <h3>切换前需完成</h3><ul>{preview.remaining.map(item => <li key={item}>{item}</li>)}</ul>
    </>}
    {result && <p className="my-3 rounded-lg border border-success/25 bg-success/5 p-3 text-sm" role="status">{result}</p>}
  </div></Panel><LegacyHistory /></>
}
