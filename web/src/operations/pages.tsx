import { ServerIcon, WorkflowIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

// Also opened from a server's page with that server preselected.
export const OperationsPage = lazyPage(() => import('./OperationsPage'))
const FleetOperations = lazyPage(() => import('./fleet/FleetOperations'))
const BatchEnrollment = lazyPage(async () => ({ default: (await import('./fleet/FleetOperations')).BatchEnrollment }))

export const pages = [
  { page: 'operations', path: '/operations', label: '自动化与维护', icon: WorkflowIcon, workspace: 'servers', order: 40, tier: 'experimental',
    features: ['operations', 'cloud'], preload: OperationsPage.preload, render: () => <OperationsPage /> },
  // Batch enrollment opens from the server list, whose entry stays marked; its form lives with the fleet tools.
  { page: 'batch-enrollment', path: '/servers/batch', label: '批量接入', icon: ServerIcon, workspace: 'servers', order: 11,
    within: '/servers', features: ['servers'], preload: BatchEnrollment.preload, render: () => <BatchEnrollment /> },
  // The fleet tools moved into the server pages; their old address opens from there.
  { page: 'fleet', path: '/fleet', label: '运维', icon: WorkflowIcon, workspace: 'servers', order: 12,
    within: '/servers', features: ['servers'], preload: FleetOperations.preload, render: () => <FleetOperations /> },
] as const satisfies readonly PageEntry[]
