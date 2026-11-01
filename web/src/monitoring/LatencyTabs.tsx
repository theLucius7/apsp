import { useResource } from '../hooks'
import { SegmentedLink, SegmentedNav } from '@/components/segmented'

// Recurring line checks have two views: latency measured by the servers, and
// reachability measured from mainland China. The second is owner-only, so other
// administrators see a single view and no switcher.
export default function LatencyTabs({ active }: { active: 'latency' | 'reachability' }) {
  const actor = useResource<{ role: string }>('/api/control-center/me', 0)
  if (actor.data?.role !== 'owner') return null
  return <SegmentedNav className="mb-6" aria-label="延迟检测">
    <SegmentedLink href="#/latency" aria-current={active === 'latency' ? 'page' : undefined}>三网延迟</SegmentedLink>
    <SegmentedLink href="#/latency/reachability" aria-current={active === 'reachability' ? 'page' : undefined}>中国可达性</SegmentedLink>
  </SegmentedNav>
}
