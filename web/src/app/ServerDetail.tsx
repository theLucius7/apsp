import { lazy, Suspense, useState } from 'react'
import { AgentSettings } from '../resources/servers/AgentSettings'
import AgentTasks from '../resources/servers/AgentTasks'
import { Badge, ErrorNotice, Loading, PageHeader, Refresh, TierNotice, DialogPresence } from '../components'
import { navigate } from '../format'
import { useI18n } from '../i18n'
import { useResource } from '../hooks'
import type { Server } from '../types'
import ServerNavigation from '../resources/servers/ServerNavigation'
import type { ServerSectionKey } from '../resources/servers/ServerNavigation'
import ServerEnrollment from '../resources/servers/ServerEnrollment'
import ServerEdit from '../resources/servers/ServerEdit'
import ServerAssets from '../resources/servers/ServerAssets'
import ServerMonitor from '../observability/display/ServerMonitor'
import { CurrencyProvider } from '../costs/currency/CurrencyContext'
import { useNow } from '@/lib/useNow'
import ServerBusiness from '../traffic/ServerBusiness'
import { ActionMenu } from '@/components/action-menu'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { ArrowLeftIcon, PlusIcon } from 'lucide-react'
import { useConnectionStore } from '@/stores/connection'
import { useFeatureStore } from '@/stores/features'

// Sections that bring their own heavy tooling load on first use.
const ServerIpInfo = lazy(() => import('../observability/server-diagnostics/ServerIpInfo'))
const TcpQuality = lazy(() => import('../observability/server-diagnostics/TcpQuality'))
const NodeQuality = lazy(() => import('../observability/server-diagnostics/NodeQuality'))
const FleetOperations = lazy(() => import('../operations/fleet/FleetOperations'))

export type ServerDetailSection = ServerSectionKey

const componentLabels: Record<string, string> = {
  'sing-box': 'sing-box 运行时',
  tcpquality: 'TCP 连接诊断',
  ipquality: 'IP 质量检测',
  nodequality: '节点验机',
}

/** The line under the server's name: where it is, what runs on it. */
function identity(server: Server) {
  const asset = server.asset_settings, info = server.static_info
  return [asset?.region, asset?.group_name, info.system, info.arch, asset?.hidden ? '看板已隐藏' : ''].filter(Boolean).join(' · ') || `服务器 #${server.id}`
}

/** Built-in capability versions reported directly by the connected Agent (`static_info.components`). */
function AgentComponents({ server, capabilities, showWhenEmpty = false }: { server: Server; capabilities: string[]; showWhenEmpty?: boolean }) {
  const entries = Object.entries(server.static_info.components ?? {}).sort(([a], [b]) => a.localeCompare(b))
  const selfContained = capabilities.includes('agent:self-contained:v1')
  if (!entries.length && !showWhenEmpty) return null
  return <Panel data-slot="agent-components">
    <PanelHeader>
      <PanelTitle>Agent 与内置组件{selfContained && <Badge tone="good">一体化 Agent</Badge>}</PanelTitle>
      {server.static_info.agent_version && <span className="text-muted-foreground font-mono text-xs">Agent v{server.static_info.agent_version}</span>}
    </PanelHeader>
    <div className="grid gap-3 p-4">
      {entries.length ? <dl className="m-0 grid gap-2.5 sm:grid-cols-2">
        {entries.map(([name, version]) => <div key={name} className="bg-muted/40 flex min-w-0 items-baseline justify-between gap-3 rounded-lg border px-3 py-2">
          <dt className="text-xs font-medium">{componentLabels[name] ?? name}</dt>
          <dd className="text-muted-foreground m-0 font-mono text-xs wrap-anywhere">{version}</dd>
        </div>)}
      </dl> : <p className="text-muted-foreground text-xs leading-relaxed">当前设备尚未上报内置组件版本；升级到一体化 Agent 并连接后将在此显示内置运行时与诊断能力版本。</p>}
    </div>
  </Panel>
}

/**
 * Every page about one server shares this shell: the name and state, the few
 * actions that apply to the server itself, and its sections as tabs. The
 * overview is the server's monitoring panel, the same one the board shows.
 */
export default function ServerDetail({ id, section = 'overview' }: { id: number; section?: ServerDetailSection }) {
  const { t } = useI18n()
  // Only the overview shows live readings; other sections need the record, not a 3-second poll.
  const server = useResource<Server>(`/api/servers/${id}`, section === 'overview' ? 3000 : 15_000)
  const connection = useConnectionStore(state => state.status)
  const showExperimental = useFeatureStore(state => state.showFrozen)
  const now = useNow(section === 'overview' ? 3000 : 30_000)
  const [servicesRevision, setServicesRevision] = useState(0)
  const [installing, setInstalling] = useState(false)
  const [editing, setEditing] = useState(false)
  const refresh = () => { server.reload(); setServicesRevision(value => value + 1) }
  const entry = server.data
  const back = <Button variant="ghost" size="icon-sm" className="-ml-1" aria-label="返回服务器列表" title="返回服务器列表" onClick={() => navigate('/servers')}><ArrowLeftIcon aria-hidden="true" /></Button>
  if (!entry) return <>
    <PageHeader leading={back} title={`服务器 #${id}`} />
    <ErrorNotice message={server.error} retry={server.reload} />
    {server.loading && <Loading variant="panel" rows={5} />}
    {/* Service configuration status only needs the server ID, so it stays usable while the server read fails. */}
    {section === 'services' && !server.loading && <ServerBusiness key={`${id}-${servicesRevision}`} serverId={id} />}
  </>
  const capabilities = Array.isArray(entry.capabilities) ? entry.capabilities : []
  const workspaces = [
    { path: 'ddns', label: '地址与域名' },
    { path: 'network-workbench', label: '网络检测' },
    ...(showExperimental ? [{ path: 'network-configuration', label: '网络配置（实验）' }, { path: 'operations', label: '自动化与维护（实验）' }] : []),
  ]
  return <>
    <PageHeader leading={back} title={entry.name} meta={identity(entry)}>
      <Badge tone={entry.online ? 'good' : 'neutral'}>{t(entry.online ? '在线' : entry.device_public_key ? '离线' : '待接入')}</Badge>
      {entry.metrics_stale && <Badge tone="warm">指标过期</Badge>}
      <Refresh onClick={refresh} />
      <Button variant="outline" onClick={() => setEditing(true)}>编辑</Button>
      <Button onClick={() => setInstalling(true)}><PlusIcon aria-hidden="true" />接入 / 升级</Button>
      <ActionMenu label="带此服务器打开" groups={[workspaces.map(workspace => ({ label: workspace.label, onSelect: () => navigate(`/servers/${id}/${workspace.path}`) }))]} />
    </PageHeader>
    <ServerNavigation id={id} active={section} />
    <ErrorNotice message={server.error} retry={refresh} />
    {section === 'overview' && <>
      <ServerMonitor server={entry} base={`/api/servers/${id}`} now={now} modern={Boolean(entry.telemetry_settings)}
        unavailable={Boolean(server.error) || connection !== 'online'} probeSettings={`#/servers/${id}/agent`} />
      <AgentComponents server={entry} capabilities={capabilities} />
    </>}
    {/* Only the assets tab converts money, so only it reads the reference rates. */}
    {section === 'assets' && <CurrencyProvider><ServerAssets onReload={server.reload} server={entry} onEdit={() => setEditing(true)} /></CurrencyProvider>}
    {section === 'agent' && <>
      <AgentComponents server={entry} capabilities={capabilities} showWhenEmpty />
      <AgentSettings serverId={id} liveSupported={capabilities.includes('telemetry:live:v1')} />
      <AgentTasks key={id} serverId={id} commandsEnabled={capabilities.includes('command:execute')} />
    </>}
    {section === 'services' && <ServerBusiness key={`${id}-${servicesRevision}`} serverId={id} />}
    <Suspense fallback={<Loading variant="panel" rows={4} />}>
      {section === 'ip-info' && <ServerIpInfo serverId={id} />}
      {section === 'tcp-quality' && <TcpQuality serverId={id} />}
      {section === 'node-quality' && <><TierNotice tier="experimental">IP 质量与解锁以「IP 信息」为准；完整验机受安全门禁限制，暂不接收新任务。</TierNotice><NodeQuality serverId={id} /></>}
      {section === 'fleet' && <><TierNotice tier="experimental">终端、文件和服务操作还需要服务器本机的运维策略允许；通用运维更适合交给 SSH 工具。</TierNotice><FleetOperations serverId={id} embedded /></>}
    </Suspense>
    <DialogPresence>{editing && <ServerEdit key={id} server={entry} onClose={() => setEditing(false)} onSaved={() => { setEditing(false); server.reload() }} />}</DialogPresence>
    <DialogPresence>{installing && <ServerEnrollment key={id} server={entry} onClose={() => { setInstalling(false); server.reload() }} />}</DialogPresence>
  </>
}
