import { TierBadge } from '../components'
import type { Tier } from '../app/tiers'
import { SegmentedGroup, SegmentedLink, SegmentedNav } from '@/components/segmented'
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

const isDiagnostic = (key: ServerSectionKey): key is DiagnosticKey => diagnostics.some(entry => entry.key === key)

export default function ServerNavigation({ id, active }: { id: number; active: ServerSectionKey }) {
  const current = isDiagnostic(active) ? 'diagnostics' : active
  // The experimental operations tab appears with the other experimental features, or while it is open.
  const showExperimental = useFeatureStore(state => state.showFrozen) || active === 'fleet'
  return <div data-slot="server-navigation" className="mb-4 grid min-w-0 gap-3">
    <SegmentedNav aria-label="服务器导航" className="min-w-0">
      {sections.filter(section => !section.tier || showExperimental).map(section => <SegmentedLink key={section.key} href={`#/servers/${id}${section.path}`}
        aria-current={current === section.key ? 'page' : undefined}>{section.label}{section.tier && <TierBadge tier={section.tier} />}</SegmentedLink>)}
    </SegmentedNav>
    {isDiagnostic(active) && <SegmentedGroup role="navigation" aria-label="线路诊断" className="justify-self-start">
      {diagnostics.map(entry => <SegmentedLink key={entry.key} href={`#/servers/${id}${entry.path}`}
        aria-current={active === entry.key ? 'page' : undefined}>{entry.label}{entry.tier && <TierBadge tier={entry.tier} />}</SegmentedLink>)}
    </SegmentedGroup>}
  </div>
}
