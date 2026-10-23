import { useRef, useState } from 'react'
import { Field, FormDialog } from '../../components'
import { useAction } from '../../hooks'
import type { ProxyResource, ResourceSnapshot } from './groupTypes'
import { chainMutationError, prepareChainMutation, submitChainMutation } from './chainRequests'
import type { ChainMutation, PendingChainMutation } from './chainRequests'
import { Input } from '@/components/ui/input'

export default function ChainResourceEditor({ resource, snapshot, open, onClose, onSaved, onPending, refresh }: { resource: ProxyResource; snapshot: ResourceSnapshot<ProxyResource[]>; open: boolean; onClose: () => void; onSaved: () => void; onPending: (value: boolean) => void; refresh: () => void }) {
  const initial = useRef({ name: resource.name, entry_name: resource.entry.name, public_host: resource.entry.public_host, port: String(resource.entry.port), sni: resource.entry.sni })
  const [draft, setDraft] = useState(initial.current)
  const pending = useRef<PendingChainMutation | undefined>(undefined)
  const [submitted, setSubmitted] = useState('')
  const action = useAction()
  const fields: Record<string, unknown> = {}
  for (const key of ['name', 'entry_name', 'public_host', 'port', 'sni'] as const) if (draft[key].trim() !== initial.current[key]) fields[key] = key === 'port' ? Number(draft.port) : draft[key].trim()
  const command: ChainMutation = { kind: 'chain', id: resource.id, settings_revision: resource.settings_revision, operation: 'edit', fields }
  const replay = pending.current?.attempted === true && pending.current.command === JSON.stringify(command)
  const error = chainMutationError(snapshot, command, replay)
  if (!open) return null
  return <FormDialog wide title="编辑链路公开信息" onClose={onClose} busy={action.busy} error={error || action.error} retry={error ? refresh : undefined} disabled={Boolean(error)} submitDisabled={!Object.keys(fields).length} submitLabel={replay ? '重试原修改' : '保存公开信息'} onSubmit={() => {
    if (action.busy || chainMutationError(snapshot, command, replay) || !Object.keys(fields).length) return
    void action.run(async () => {
      for (const key of ['name', 'entry_name'] as const) if (!draft[key].trim() || [...draft[key].trim()].length > 128 || /[\u0000-\u001f\u007f]/.test(draft[key])) throw new Error('名称需为 1–128 个字符，不能含控制字符。')
      if (fields.port !== undefined && (!/^[1-9]\d*$/.test(draft.port.trim()) || !Number.isSafeInteger(fields.port) || Number(fields.port) > 65535 || fields.port === 18085)) throw new Error('监听端口需为 1–65535，18085 为保留端口。')
      if (fields.public_host !== undefined && (!draft.public_host.trim() || /[\s/@?#\\]/.test(draft.public_host) || draft.public_host.includes('://'))) throw new Error('公开地址不能含协议、路径或端口。')
      if (fields.sni !== undefined && !/^[a-zA-Z0-9.-]+$/.test(draft.sni.trim())) throw new Error('请输入有效 Reality 域名。')
      const request = prepareChainMutation(command, snapshot, pending.current)
      pending.current = request; setSubmitted(request.request_id); onPending(true)
      return submitChainMutation(request, snapshot)
    }, () => { pending.current = undefined; onPending(false); refresh(); onSaved() })
  }}><p className="text-muted-foreground mt-3 text-xs leading-relaxed">入口身份、协议、密钥及有序拓扑保持不变。修改公开接入参数会规划新路径候选；设备离线或无法并存可恢复参数时，面板会说明拒绝原因。显示名称可独立修改，入口订阅名称需明确填写。</p>
    {(['name', 'entry_name', 'public_host', 'port', 'sni'] as const).map(key => <Field key={key} label={{ name: '链路显示名称', entry_name: '入口订阅名称', public_host: '入口公开地址', port: '入口监听端口', sni: 'Reality 协议域名' }[key]}><Input name={key} required value={draft[key]} type={key === 'port' ? 'number' : 'text'} min={key === 'port' ? 1 : undefined} max={key === 'port' ? 65535 : undefined} onChange={event => { action.clearError(); setDraft(current => ({ ...current, [key]: event.target.value })) }} autoComplete="off" /></Field>)}
    {submitted && <p className="text-muted-foreground mt-3 text-xs leading-relaxed" role="status">操作编号 <code>{submitted}</code>。失败或超时后，未改草稿重用原编号与精确请求；修改草稿生成新编号。</p>}
  </FormDialog>
}
