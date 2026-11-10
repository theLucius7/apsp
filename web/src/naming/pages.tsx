import { GlobeIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

// Also opened from a server's page with that server preselected.
export const Ddns = lazyPage(() => import('./dns/Ddns'))

export const pages = [
  { page: 'ddns', path: '/dns', label: '域名解析', icon: GlobeIcon, workspace: 'servers', order: 20,
    features: ['dns'], preload: Ddns.preload, render: () => <Ddns /> },
] as const satisfies readonly PageEntry[]
