import { WaypointsIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

// The node pages take their view, server and selection from the address.
export const Nodes = lazyPage(() => import('./Nodes'), true)

export const pages = [
  { page: 'nodes', path: '/nodes', label: '节点', icon: WaypointsIcon, workspace: 'network', order: 10,
    features: ['proxy'], allServers: true, preload: Nodes.preload },
] as const satisfies readonly PageEntry[]
