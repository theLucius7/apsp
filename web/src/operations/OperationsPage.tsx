import { useEffect, useState } from 'react'
import { api, errorMessage } from '../api'
import type { Server } from '../types'
import JobsTab from './JobsTab'
import IncidentsTab from './IncidentsTab'
import RecoveryTab from './RecoveryTab'
import CloudTab from './CloudTab'

import { Field, PageHeader } from '../components'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { Alert } from '@/components/ui/alert'
import { SegmentedButton, SegmentedNav } from '@/components/segmented'
type Actor = { role: string; all_servers: boolean; capabilities: string[] }
const tabDefinitions = [['jobs', '自动化与维护', 'operations:read'], ['incidents', '故障事件', 'operations:read'], ['recovery', '备份与恢复', 'recovery:read'], ['cloud', '云资源与费用', 'cloud:read']]

export default function OperationsPage({ selectedServerId }: { selectedServerId?: number }) {
  const [tab, setTab] = useState('')
  const [actor, setActor] = useState<Actor | null>(null)
  const [servers, setServers] = useState<Server[]>([])
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [version, setVersion] = useState(0)
  const [pendingAction, setPendingAction] = useState<(() => Promise<void>) | null>(null)
  const [password, setPassword] = useState('')
  const [code, setCode] = useState('')
  const allows = (capability: string) => actor?.role === 'owner' || Boolean(actor?.capabilities.includes(capability))
  const tabs = tabDefinitions.filter(([, , capability]) => allows(capability) && (capability !== 'recovery:read' || actor?.all_servers || actor?.role === 'owner'))
  const canChooseServers = allows('servers:read')
  useEffect(() => { let live = true; void api<Actor>('/api/control-center/me').then(value => { if (live) { setActor(value); const initial = tabDefinitions.find(([, , capability]) => (value.role === 'owner' || value.capabilities.includes(capability)) && (capability !== 'recovery:read' || value.all_servers || value.role === 'owner')); setTab(initial?.[0] ?? '') } }).catch(value => { if (live) setError(errorMessage(value)) }); return () => { live = false } }, [])
  useEffect(() => { if (!actor || !(actor.role === 'owner' || actor.capabilities.includes('servers:read')) || !(actor.role === 'owner' || actor.capabilities.some(capability => ['operations:read', 'cloud:read'].includes(capability)))) return; let live = true; void api<Server[]>('/api/servers').then(values => { if (live) setServers(values) }).catch(value => { if (live) setError(errorMessage(value)) }); return () => { live = false } }, [actor])
  const execute = async (action: () => Promise<void>) => { setBusy(true); setError(''); try { await action() } catch (value) { setError(errorMessage(value)) } finally { setBusy(false) } }
  const run = (action: () => Promise<void>, proof = false) => { if (proof) { setPendingAction(() => action); setPassword(''); setCode('') } else { void execute(action) } }
  return <div data-slot="operations-page" className="grid min-w-0 grid-cols-1 gap-4 *:min-w-0" aria-busy={busy}><PageHeader eyebrow="服务器" title="运维、故障与恢复" description="批量操作保留固定目标、每步结果与恢复记录；云费用与备份材料均注明实际来源。"><Button variant="outline" disabled={busy || !actor} onClick={() => setVersion(value => value + 1)}>刷新状态</Button></PageHeader><SegmentedNav aria-label="运维功能">{tabs.map(([key, name]) => <SegmentedButton type="button" key={key} aria-pressed={tab === key} onClick={() => setTab(key)}>{name}</SegmentedButton>)}</SegmentedNav>{error && !pendingAction && <Alert variant="destructive" className="wrap-anywhere">{error}</Alert>}{!actor && !error && <p role="status" className="text-muted-foreground text-sm">正在读取当前授权。</p>}{actor && !tabs.length && <p className="text-muted-foreground text-sm">当前账号没有运维、云资源或全局恢复的读取授权。</p>}{actor && !canChooseServers && ['jobs', 'cloud'].includes(tab) && <p className="text-muted-foreground text-sm">当前账号没有服务器列表读取授权，无法选择新目标；已有授权任务和资源仍可查看。</p>}{busy && <p role="status" className="text-muted-foreground text-sm">正在处理，请等待结果。</p>}{tab === 'jobs' && <JobsTab servers={servers} selectedServerId={selectedServerId} allowTargetSelection={canChooseServers} run={run} version={version} />}{tab === 'incidents' && <IncidentsTab servers={servers} run={run} version={version} />}{tab === 'recovery' && <RecoveryTab run={run} version={version} />}{tab === 'cloud' && <CloudTab servers={servers} run={run} version={version} />}
    {pendingAction && <Dialog open onOpenChange={open => { if (!open && !busy) { setPendingAction(null); setPassword(''); setCode('') } }}>
      <DialogContent showCloseButton={false} className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>再次验证管理员身份</DialogTitle>
          <DialogDescription>执行确认绑定当前管理会话，验证后继续已预览的操作。</DialogDescription>
        </DialogHeader>
        <form className="grid" onSubmit={event => { event.preventDefault(); const action = pendingAction; void execute(async () => { await api('/api/control-center/reauth', 'POST', { password, totp_code: code || null }); setPassword(''); setCode(''); setPendingAction(null); await action() }) }}>
          <Field label="管理员密码"><Input autoFocus type="password" autoComplete="current-password" value={password} onChange={event => setPassword(event.target.value)} required /></Field>
          <Field label="动态验证码（如已启用）"><Input inputMode="numeric" autoComplete="one-time-code" value={code} onChange={event => setCode(event.target.value)} /></Field>
          {error && <Alert variant="destructive" className="mb-3 wrap-anywhere">{error}</Alert>}
          <DialogFooter>
            <Button variant="outline" type="button" onClick={() => { setPendingAction(null); setPassword(''); setCode('') }}>返回修改</Button>
            <Button type="submit" disabled={busy}>验证并继续</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>}
  </div>
}
