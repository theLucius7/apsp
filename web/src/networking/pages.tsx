import { NetworkIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

// Also opened from a server's page with that server preselected.
export const NetworkConfiguration = lazyPage(() => import('./configuration/NetworkConfiguration'))

export const pages = [
  { page: 'network-configuration', path: '/network-configuration', label: '网络配置', icon: NetworkIcon, workspace: 'servers', order: 50, tier: 'experimental',
    features: ['network'], preload: NetworkConfiguration.preload, render: () => <NetworkConfiguration /> },
] as const satisfies readonly PageEntry[]
