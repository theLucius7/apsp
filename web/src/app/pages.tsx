import { HouseIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

// The application's own pages: compositions of several domains.
const Overview = lazyPage(() => import('./Overview'), true)
export const ServerDetail = lazyPage(() => import('./ServerDetail'), true)

export const pages = [
  { page: 'singbox-overview', path: '/home', label: '概览', icon: HouseIcon, workspace: 'overview', order: 10,
    features: ['proxy'], allServers: true, preload: Overview.preload, render: () => <Overview /> },
] as const satisfies readonly PageEntry[]
