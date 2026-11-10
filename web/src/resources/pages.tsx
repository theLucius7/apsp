import { CloudCogIcon, ServerIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

const Servers = lazyPage(() => import('./servers/Servers'), true)
const BatchEnrollment = lazyPage(() => import('./servers/BatchEnrollmentPage'))
const Alicloud = lazyPage(() => import('./cloud/Alicloud'))

export const pages = [
  { page: 'servers', path: '/servers', label: '服务器列表', navLabel: '服务器', icon: ServerIcon, workspace: 'servers', order: 10,
    features: ['servers'], preload: Servers.preload, render: () => <Servers /> },
  // Batch enrollment opens from the server list, whose entry stays marked.
  { page: 'batch-enrollment', path: '/servers/batch', label: '批量接入', icon: ServerIcon, workspace: 'servers', order: 11,
    within: '/servers', features: ['servers'], preload: BatchEnrollment.preload, render: () => <BatchEnrollment /> },
  { page: 'alicloud', path: '/alicloud', label: '阿里云 CDT', icon: CloudCogIcon, workspace: 'servers', order: 30,
    features: ['cloud'], allServers: true, preload: Alicloud.preload, render: () => <Alicloud /> },
] as const satisfies readonly PageEntry[]
