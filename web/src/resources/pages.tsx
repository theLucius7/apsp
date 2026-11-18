import { CloudCogIcon, PackageIcon, ServerIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

const Servers = lazyPage(() => import('./servers/Servers'), true)
const Alicloud = lazyPage(() => import('./cloud/Alicloud'))
const AgentUpdates = lazyPage(() => import('./AgentUpdates'))

export const pages = [
  // The first entry keeps its own name, so the sidebar never reads 服务器 › 服务器.
  { page: 'servers', path: '/servers', label: '服务器列表', icon: ServerIcon, workspace: 'servers', order: 10,
    features: ['servers'], preload: Servers.preload, render: () => <Servers /> },
  { page: 'alicloud', path: '/alicloud', label: '云资源', icon: CloudCogIcon, workspace: 'servers', order: 30,
    features: ['cloud'], allServers: true, preload: Alicloud.preload, render: () => <Alicloud /> },
  { page: 'agent-updates', path: '/settings/agent-updates', label: 'Agent 版本', icon: PackageIcon, workspace: 'system', order: 11,
    parent: '/settings', preload: AgentUpdates.preload, render: () => <AgentUpdates /> },
] as const satisfies readonly PageEntry[]
