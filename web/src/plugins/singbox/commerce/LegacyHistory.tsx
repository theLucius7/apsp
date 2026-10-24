import { useState } from 'react'
import { useResource } from '../../../hooks'
import { ErrorNotice } from '../../../components'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { NativeSelect } from '@/components/ui/native-select'
import { Button } from '@/components/ui/button'
import { root } from './types'
export default function LegacyHistory() {
  const [kind, setKind] = useState('order'), [offset, setOffset] = useState(0)
  const data = useResource<{ entries: { kind: string; source_id: string; user_id: number | null; record: unknown }[] }>(`${root}/migration/records?kind=${kind}&offset=${offset}`)
  return <Panel><PanelHeader><PanelTitle>原系统历史记录</PanelTitle><NativeSelect aria-label="历史类型" value={kind} onChange={e => { setKind(e.target.value); setOffset(0) }}>{Object.entries({ order: '订单', wallet: '钱包', invitation: '邀请', payment: '付款', traffic: '流量', reset: '重置' }).map(([key, label]) => <option key={key} value={key}>{label}</option>)}</NativeSelect></PanelHeader><div className="space-y-3 p-4"><ErrorNotice message={data.error} retry={data.reload} />{data.data?.entries.map(r => <details key={r.source_id}><summary className="cursor-pointer text-sm">{r.source_id} · 成员 {r.user_id ?? '—'}</summary><pre className="max-h-72 overflow-auto whitespace-pre-wrap break-all rounded-lg bg-muted/50 p-3 text-xs">{JSON.stringify(r.record, null, 2)}</pre></details>)}<div className="flex gap-3"><Button variant="outline" size="sm" disabled={!offset} onClick={() => setOffset(offset - 100)}>上一页</Button><Button variant="outline" size="sm" disabled={(data.data?.entries.length ?? 0) < 100} onClick={() => setOffset(offset + 100)}>下一页</Button></div></div></Panel>
}
