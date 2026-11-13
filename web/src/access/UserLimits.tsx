import { useEffect, useState } from 'react'
import { api } from '../api'
import { ErrorNotice, Field, FormDialog, Refresh, DialogPresence } from '../components'
import { resourceWriteError, useAction, useResource } from '../hooks'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { isRecord } from '@/lib/validate'

type Limits = { user_id: number; revision: number; upload_mbps: number; download_mbps: number; max_connections: number; servers?: { server_id: number; name: string; state: 'pending' | 'device_confirmed' }[] }
export function validLimits(value: unknown): value is Limits {
  if (!isRecord(value)) return false
  const item = value as Limits
  return Number.isSafeInteger(item.user_id) && item.user_id > 0 && Number.isSafeInteger(item.revision) && item.revision >= 0
    && [item.upload_mbps, item.download_mbps, item.max_connections].every(n => Number.isSafeInteger(n) && n >= 0 && n <= 1000000)
    && Array.isArray(item.servers) && item.servers.every(server => isRecord(server) && Number.isSafeInteger(server.server_id) && typeof server.name === 'string' && ['pending', 'device_confirmed'].includes(server.state))
}

export default function UserLimits({ id, userError, refreshRevision }: { id: number; userError: () => string; refreshRevision: number }) {
  const resource = useResource<unknown>(`/api/access/users/${id}/limits`, 10000, { keep: true })
  const action = useAction()
  const [draft, setDraft] = useState<Limits>()
  const current = validLimits(resource.data) && resource.data.user_id === id ? resource.data : undefined
  useEffect(() => { if (refreshRevision) resource.reload() }, [refreshRevision, resource.reload])
  const writeError = () => {
    const failure = userError() || resourceWriteError(resource)
    if (failure) return failure
    const latest = resource.getCurrent()
    if (!validLimits(latest) || latest.user_id !== id) return '限制数据暂不可用，请刷新后重试。'
    if (draft && (draft.user_id !== id || draft.revision !== latest.revision)) return '限制已被其他操作修改，当前草稿已保留；请关闭后重新打开并确认。'
    return ''
  }
  const submit = (form: FormData) => {
    if (!draft || writeError()) return
    const body = { expected_revision: draft.revision, upload_mbps: Number(form.get('upload_mbps')), download_mbps: Number(form.get('download_mbps')), max_connections: Number(form.get('max_connections')) }
    void action.run(() => api(`/api/access/users/${id}/limits`, 'PUT', body), () => { setDraft(undefined); resource.reload() })
  }
  return <Panel>
    <PanelHeader><PanelTitle>用户限制</PanelTitle><div className="flex gap-2"><Refresh onClick={resource.reload} /><Button variant="outline" size="sm" disabled={action.busy || Boolean(writeError())} onClick={() => { if (!writeError() && current) { action.clearError(); setDraft(current) } }}>设置限制</Button></div></PanelHeader>
    <div className="space-y-2 p-4 pt-0 text-sm">
      <ErrorNotice message={resource.error || (!resource.loading && !current ? '限制数据暂不可用' : '')} retry={resource.reload} />
      <p className="text-muted-foreground">同一用户在每台服务器上的所有受管节点共用额度，各服务器分别计算。连接数合计 TCP 连接与 UDP 会话。外部节点及尚未接管的线路不受控。</p>
      {current && <><p>上传：{current.upload_mbps ? `${current.upload_mbps} Mbps` : '不限'} · 下载：{current.download_mbps ? `${current.download_mbps} Mbps` : '不限'} · 连接数：{current.max_connections || '不限'}</p>
        <div className="flex flex-wrap gap-x-4 gap-y-1 text-xs text-muted-foreground">{current.servers?.length ? current.servers.map(server => <span key={server.server_id}>{server.name}：{server.state === 'device_confirmed' ? '服务器已确认' : '待服务器应用或确认'}</span>) : '暂无受管节点，授权后随配置应用。'}</div></>}
    </div>
    <DialogPresence>{draft && <FormDialog title="设置用户限制" busy={action.busy} error={writeError() || action.error} submitDisabled={Boolean(writeError())} onClose={() => setDraft(undefined)} onSubmit={submit} submitLabel="保存限制">
      <p className="text-sm text-muted-foreground">0 表示不限。保存后等待服务器应用；应用配置可能重建现有连接。连接达到上限时拒绝新连接，关闭后释放名额。</p>
      <Field label="上传限速（Mbps）"><Input name="upload_mbps" type="number" min={0} max={1000000} step={1} required defaultValue={draft.upload_mbps} /></Field>
      <Field label="下载限速（Mbps）"><Input name="download_mbps" type="number" min={0} max={1000000} step={1} required defaultValue={draft.download_mbps} /></Field>
      <Field label="最大连接数"><Input name="max_connections" type="number" min={0} max={1000000} step={1} required defaultValue={draft.max_connections} /></Field>
    </FormDialog>}</DialogPresence>
  </Panel>
}
