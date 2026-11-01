import { TierBadge } from '../components'
import type { Tier } from '../app/tiers'
import { SegmentedGroup, SegmentedLink, SegmentedNav } from '@/components/segmented'
import { buttonVariants } from '@/components/ui/button'
import { useFeatureStore } from '@/stores/features'

export type ServerSectionKey = 'overview' | 'assets' | 'agent' | 'plugins' | 'ip-info' | 'node-quality' | 'tcp-quality' | 'fleet'
type DiagnosticKey = Extract<ServerSectionKey, 'ip-info' | 'node-quality' | 'tcp-quality'>

// Views of this one server, in the order an administrator usually needs them.
// The three line diagnostics share one tab and keep their own addresses.
const sections: readonly { key: ServerSectionKey | 'diagnostics'; path: string; label: string; tier?: Tier }[] = [
  { key: 'overview', path: '', label: '概况' },
  { key: 'assets', path: '/assets', label: '资产与流量' },
  { key: 'agent', path: '/agent', label: 'Agent 与任务' },
  { key: 'plugins', path: '/plugins', label: '插件' },
  { key: 'diagnostics', path: '/ip-info', label: '线路诊断' },
  { key: 'fleet', path: '/fleet', label: '运维', tier: 'experimental' },
]

export const diagnostics: readonly { key: DiagnosticKey; path: string; label: string; tier?: Tier }[] = [
  { key: 'ip-info', path: '/ip-info', label: 'IP 信息' },
  { key: 'tcp-quality', path: '/tcp-quality', label: 'TCP 诊断' },
  { key: 'node-quality', path: '/node-quality', label: '节点验机', tier: 'experimental' },
]

// Cross-server workspaces keep their own sidebar entry; these links only open
// them with this server preselected.
const workspaces: readonly { path: string; label: string; tier?: Tier }[] = [
  { path: '/network-workbench', label: '网络检测' },
  { path: '/network-configuration', label: '网络配置', tier: 'experimental' },
  { path: '/operations', label: '自动化与维护', tier: 'experimental' },
]

const isDiagnostic = (key: ServerSectionKey): key is DiagnosticKey => diagnostics.some(entry => entry.key === key)

export default function ServerNavigation({ id, active }: { id: number; active: ServerSectionKey }) {
  const current = isDiagnostic(active) ? 'diagnostics' : active
  const showExperimental = useFeatureStore(state => state.showFrozen)
  return <div data-slot="server-navigation" className="mb-4 grid min-w-0 gap-2">
    <div className="flex min-w-0 flex-wrap items-end justify-between gap-x-4 border-b">
      <SegmentedNav aria-label="服务器导航" className="min-w-0 border-b-0">
        {sections.map(section => <SegmentedLink key={section.key} href={`#/servers/${id}${section.path}`}
          aria-current={current === section.key ? 'page' : undefined}>{section.label}{section.tier && <TierBadge tier={section.tier} />}</SegmentedLink>)}
      </SegmentedNav>
      <nav data-slot="server-workspaces" className="text-muted-foreground flex min-w-0 flex-wrap items-center gap-0.5 pb-1 text-xs" aria-label="带此服务器打开">
        <span className="mr-1">带此服务器打开</span>
        {workspaces.filter(workspace => !workspace.tier || showExperimental).map(workspace => <a key={workspace.path} className={buttonVariants({ variant: 'ghost', size: 'xs' })} href={`#/servers/${id}${workspace.path}`}>{workspace.label}{workspace.tier && <TierBadge tier={workspace.tier} />}</a>)}
      </nav>
    </div>
    {isDiagnostic(active) && <SegmentedGroup role="navigation" aria-label="线路诊断" className="justify-self-start">
      {diagnostics.map(entry => <SegmentedLink key={entry.key} href={`#/servers/${id}${entry.path}`}
        aria-current={active === entry.key ? 'page' : undefined}>{entry.label}{entry.tier && <TierBadge tier={entry.tier} />}</SegmentedLink>)}
    </SegmentedGroup>}
  </div>
}
