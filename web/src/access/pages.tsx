import { LayersIcon, UsersIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

// The user pages take the selected user from the address.
export const ProxyUsers = lazyPage(() => import('./ProxyUsers'), true)
const Groups = lazyPage(() => import('./Groups'), true)

export const pages = [
  { page: 'proxy-users', path: '/users', label: '用户', icon: UsersIcon, workspace: 'network', order: 20,
    features: ['proxy'], allServers: true, preload: ProxyUsers.preload },
  { page: 'groups', path: '/groups', label: '策略与套餐', icon: LayersIcon, workspace: 'network', order: 30,
    features: ['proxy'], allServers: true, preload: Groups.preload, render: () => <Groups /> },
] as const satisfies readonly PageEntry[]
