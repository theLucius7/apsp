import { useEffect, useState } from 'react'
import { api, errorMessage } from '../api'
import { requestReauthentication } from '../governance/control-center/reauthentication-request'
import type { Server } from '../types'
import type { ActionRunner } from './types'
import JobsTab from './JobsTab'
import IncidentsTab from '../observability/IncidentsTab'
import CloudTab from '../resources/cloud/CloudTab'

import { PageHeader, TierBadge, TierNotice } from '../components'
import { visibleByTier } from '@/lib/tiers'
import type { Tier } from '@/lib/tiers'
import { useFeatureStore } from '@/stores/features'
import { Button } from '@/components/ui/button'
import { Alert } from '@/components/ui/alert'
import { SegmentedButton, SegmentedNav } from '@/components/segmented'
type Actor = { role: string; all_servers: boolean; capabilities: string[] }
// Panel backup and recovery moved to 系统 › 备份与恢复; cloud resources are frozen.
const tabDefinitions: readonly { key: string; name: string; capability: string; tier?: Tier }[] = [
  { key: 'jobs', name: '批量与计划', capability: 'operations:read' },
  { key: 'incidents', name: '故障事件', capability: 'operations:read' },
  { key: 'cloud', name: '云资源与费用', capability: 'cloud:read', tier: 'frozen' },
]

export default function OperationsPage({ selectedServerId }: { selectedServerId?: number }) {
  const [tab, setTab] = useState('')
  const [actor, setActor] = useState<Actor | null>(null)
  const [servers, setServers] = useState<Server[]>([])
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [version, setVersion] = useState(0)
  const showFrozen = useFeatureStore(state => state.showFrozen)
  const allows = (capability: string) => actor?.role === 'owner' || Boolean(actor?.capabilities.includes(capability))
  const tabs = visibleByTier(tabDefinitions, showFrozen).filter(definition => allows(definition.capability))
  const canChooseServers = allows('servers:read')
  useEffect(() => { let live = true; void api<Actor>('/api/control-center/me').then(value => { if (live) { setActor(value); const initial = tabDefinitions.find(definition => definition.tier !== 'frozen' && (value.role === 'owner' || value.capabilities.includes(definition.capability))); setTab(initial?.key ?? '') } }).catch(value => { if (live) setError(errorMessage(value)) }); return () => { live = false } }, [])
  useEffect(() => { if (!actor || !(actor.role === 'owner' || actor.capabilities.includes('servers:read')) || !(actor.role === 'owner' || actor.capabilities.some(capability => ['operations:read', 'cloud:read'].includes(capability)))) return; let live = true; void api<Server[]>('/api/servers').then(values => { if (live) setServers(values) }).catch(value => { if (live) setError(errorMessage(value)) }); return () => { live = false } }, [actor])
  const execute = async (action: () => Promise<void>) => { setBusy(true); setError(''); try { await action() } catch (value) { setError(errorMessage(value)) } finally { setBusy(false) } }
  // Actions that need a recent proof ask for it through the shared dialog first.
  const run: ActionRunner = (action, proof = false) => { void execute(async () => { if (proof) await requestReauthentication(); await action() }) }
  return <div data-slot="operations-page" className="grid min-w-0 grid-cols-1 gap-4 *:min-w-0" aria-busy={busy}><PageHeader title="自动化与维护"><Button variant="outline" disabled={busy || !actor} onClick={() => setVersion(value => value + 1)}>刷新状态</Button></PageHeader><TierNotice tier="experimental"><a className="underline underline-offset-4" href="#/settings/backup">前往备份与恢复</a></TierNotice><SegmentedNav aria-label="运维功能">{tabs.map(definition => <SegmentedButton type="button" key={definition.key} aria-pressed={tab === definition.key} onClick={() => setTab(definition.key)}>{definition.name}{definition.tier && <TierBadge tier={definition.tier} />}</SegmentedButton>)}</SegmentedNav>{error && <Alert variant="destructive" className="wrap-anywhere">{error}</Alert>}{!actor && !error && <p role="status" className="text-muted-foreground text-sm">正在读取当前授权。</p>}{actor && !tabs.length && <p className="text-muted-foreground text-sm">当前账号没有运维或云资源的读取授权。</p>}{actor && !canChooseServers && ['jobs', 'cloud'].includes(tab) && <p className="text-muted-foreground text-sm">当前账号没有服务器列表读取授权，无法选择新目标；已有授权任务和资源仍可查看。</p>}{busy && <p role="status" className="text-muted-foreground text-sm">正在处理，请等待结果。</p>}{tab === 'jobs' && <JobsTab servers={servers} selectedServerId={selectedServerId} allowTargetSelection={canChooseServers} run={run} version={version} />}{tab === 'incidents' && <IncidentsTab servers={servers} run={run} version={version} />}{tab === 'cloud' && tabs.some(definition => definition.key === 'cloud') && <CloudTab servers={servers} run={run} version={version} />}
  </div>
}
