import { useState } from 'react'
import { api } from '../../api'
import { Badge, Confirm, Empty, ErrorNotice, Loading, Modal } from '../../components'
import { useAction, useResource } from '../../hooks'
import { ddnsMessage } from './types'
import type { DdnsRule } from './types'
import { Button } from '@/components/ui/button'
import { WaypointsIcon } from 'lucide-react'

type Snapshot = { id: string; name: string; kind: string; line: string; values: string[]; ttl: number; proxied: boolean; active: boolean }
type Entry = { id: string; revision: number; operation: string; previous: Snapshot | null; observed: Snapshot | null; desired_ip: string | null; status: string; error_code: string | null; occurred_at: number }
type Preview = { revision: number; desired_ip: string | null; remote: Snapshot | null; source_error: string | null; error_code: string | null; change: string; checked_at: number }
const time = (value: number) => new Date(value * 1000).toLocaleString('zh-CN', { hour12: false })
const detail = (record: Snapshot | null) => record ? `${record.values.join(', ')} · TTL ${record.ttl === 1 ? '自动' : record.ttl} · ${record.proxied ? '代理开启' : '仅 DNS'}` : '没有记录值'
const operation: Record<string, string> = { sync: '自动或手动同步', check: '提供方核对', rollback: '回退', migration: '来源绑定迁移' }

export default function Lifecycle({ rule, close, changed }: { rule: DdnsRule; close: () => void; changed: () => void }) {
  const history = useResource<Entry[]>(`/api/plugins/ddns/rules/${rule.id}/history`)
  const action = useAction()
  const [preview, setPreview] = useState<Preview | null>(null)
  const [rollback, setRollback] = useState<Entry | null>(null)
  const refresh = () => { history.reload(); changed() }
  return <Modal title={`核对与同步历史 · ${rule.config.name}`} wide busy={action.busy} onClose={close}>
    <div data-slot="modal-body" className="p-4"><ErrorNotice message={action.error || history.error} retry={history.reload} />
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">从提供方读取当前记录，不修改 DNS。回退会再次核对远端记录是否仍与该次同步结果一致，并暂停此规则的自动同步；远端发生变化时拒绝覆盖。</p>
      <Button variant="outline" disabled={action.busy || rule.busy} onClick={() => void action.run(async () => {
        const result = await api<Preview>(`/api/plugins/ddns/rules/${rule.id}/preview`, 'POST', { revision: rule.revision })
        setPreview(result); history.reload()
      })}>{action.busy ? '正在读取…' : '读取提供方并预览差异'}</Button>
      {preview && <div className="p-4"><Badge tone={preview.error_code || preview.source_error ? 'warm' : 'good'}>{preview.change === 'create' ? '将新建记录' : preview.change === 'update' ? '地址或设置有差异' : preview.change === 'unchanged' ? '与提供方记录一致' : '同步条件未满足'}</Badge>
        <dl className="my-5 grid grid-cols-1 gap-x-3 gap-y-4 text-sm min-[361px]:grid-cols-2 md:grid-cols-3 md:gap-5"><div><dt className="text-muted-foreground mb-1.5">期望地址</dt><dd className="m-0 wrap-anywhere">{preview.desired_ip ?? '不可用'}</dd></div><div><dt className="text-muted-foreground mb-1.5">提供方当前值</dt><dd className="m-0 wrap-anywhere">{detail(preview.remote)}</dd></div><div><dt className="text-muted-foreground mb-1.5">核对时间</dt><dd className="m-0 wrap-anywhere">{time(preview.checked_at)}</dd></div><div><dt className="text-muted-foreground mb-1.5">采集来源</dt><dd className="m-0 wrap-anywhere">{rule.config.address_source === 'manual' ? '手工公网地址' : `${rule.server_name} · ${rule.config.address_source === 'interface' ? `网卡 ${rule.config.interface_name}` : rule.config.address_source === 'discovered' ? '实际公网出口发现' : 'Agent 合并地址报告'}`}</dd></div></dl>
        {(preview.error_code || preview.source_error) && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{ddnsMessage(preview.error_code) || ddnsMessage(preview.source_error)}</p>}
        <p className="text-muted-foreground mt-3 text-xs leading-relaxed">这里展示提供方查询结果；指定解析器和各地缓存是否生效需另行实际观测。</p></div>}
      <h3 className="text-sm font-semibold">最近 256 条记录</h3>
      {!history.data && history.loading ? <Loading /> : !history.data?.length ? <Empty icon={WaypointsIcon} title="暂无同步历史" description="新记录会保存期望地址、旧值、核对结果、错误和时间。" /> : <div className="px-6 max-sm:px-4">{history.data.map(entry => <article data-slot="ddns-rule" className="border-t py-5 first:border-t-0" key={entry.id}>
        <div className="flex flex-wrap items-center justify-between gap-4"><strong>{operation[entry.operation] ?? '操作'} · {time(entry.occurred_at)}</strong><Badge tone={entry.error_code ? 'warm' : 'neutral'}>{ddnsMessage(entry.status) || (entry.status === 'checked' ? '已读取提供方' : '已记录')}</Badge></div>
        <dl className="my-5 grid grid-cols-1 gap-x-3 gap-y-4 text-sm min-[361px]:grid-cols-2 md:grid-cols-3 md:gap-5"><div><dt className="text-muted-foreground mb-1.5">期望地址</dt><dd className="m-0 wrap-anywhere">{entry.desired_ip ?? '不可用'}</dd></div><div><dt className="text-muted-foreground mb-1.5">修改前</dt><dd className="m-0 wrap-anywhere">{detail(entry.previous)}</dd></div><div><dt className="text-muted-foreground mb-1.5">提供方结果</dt><dd className="m-0 wrap-anywhere">{detail(entry.observed)}</dd></div><div><dt className="text-muted-foreground mb-1.5">规则版本</dt><dd className="m-0 wrap-anywhere">{entry.revision}</dd></div></dl>
        {entry.error_code && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{ddnsMessage(entry.error_code)}</p>}
        {entry.operation === 'sync' && entry.status === 'updated' && entry.previous && entry.observed && <Button variant="ghost" size="sm" disabled={action.busy || rule.busy} onClick={() => setRollback(entry)}>回退到此次修改前的值</Button>}
      </article>)}</div>}
    </div><footer><Button variant="outline" disabled={action.busy} onClick={close}>关闭</Button></footer>
    {rollback && <Confirm title="回退 DNS 记录并暂停同步" busy={action.busy} error={action.error} onClose={() => setRollback(null)} onConfirm={() => void action.run(async () => {
      const result = await api<{ error_code: string | null }>(`/api/plugins/ddns/rules/${rule.id}/rollback`, 'POST', { revision: rule.revision, history_id: rollback.id, confirmed: true })
      refresh(); setPreview(null)
      if (result.error_code) throw new Error(ddnsMessage(result.error_code))
    }, () => setRollback(null))}>将恢复为 {detail(rollback.previous)}。远端当前值必须仍为 {detail(rollback.observed)}；回退后请先核对，再手工决定是否恢复自动同步。</Confirm>}
  </Modal>
}
