import { BellRingIcon, ChartColumnIcon, GaugeIcon, LayoutDashboardIcon, RadarIcon, SlidersHorizontalIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

const Statistics = lazyPage(() => import('./statistics/Statistics'), true)
const Notifications = lazyPage(() => import('./monitoring/Notifications'), true)
const AlertSettings = lazyPage(() => import('./monitoring/AlertSettings'))
const LineQuality = lazyPage(() => import('./monitoring/LineQuality'))
const LineQualitySettings = lazyPage(() => import('./monitoring/LineQualitySettings'))
// Also opened from a server's page with that server preselected.
export const NetworkWorkbench = lazyPage(() => import('./network-workbench/NetworkWorkbench'))

export const pages = [
  { page: 'statistics', path: '/statistics', label: '趋势与报表', icon: ChartColumnIcon, workspace: 'monitoring', order: 30,
    features: ['monitoring'], allServers: true, preload: Statistics.preload, render: () => <Statistics /> },
  // The server dashboard is a view of the server list; the application draws it, also for
  // visitors of the public dashboard.
  { page: 'dashboard', path: '/dashboard', label: '服务器看板', icon: LayoutDashboardIcon, workspace: 'servers', order: 15,
    parent: '/servers', features: ['monitoring'], allServers: true },
  { page: 'notifications', path: '/alerts', label: '告警事件', navLabel: '告警', icon: BellRingIcon, workspace: 'monitoring', order: 10,
    features: ['monitoring'], allServers: true, preload: Notifications.preload, render: () => <Notifications /> },
  { page: 'alert-settings', path: '/alerts/settings', label: '告警设置', icon: SlidersHorizontalIcon, workspace: 'monitoring', order: 11,
    parent: '/alerts', features: ['monitoring'], allServers: true, preload: AlertSettings.preload, render: () => <AlertSettings /> },
  // ADR 0130: latency and China reachability answer one question and share the overview;
  // the checks are configured in one tab, and ad-hoc network tests sit beside them.
  { page: 'latency', path: '/latency', label: '质量概况', navLabel: '线路质量', icon: GaugeIcon, workspace: 'monitoring', order: 20,
    aliases: ['/latency/reachability'], features: ['monitoring'], allServers: true, preload: LineQuality.preload, render: () => <LineQuality /> },
  { page: 'latency-settings', path: '/latency/settings', label: '检测设置', icon: SlidersHorizontalIcon, workspace: 'monitoring', order: 21,
    parent: '/latency', preload: LineQualitySettings.preload, render: () => <LineQualitySettings /> },
  { page: 'network-workbench', path: '/network-workbench', label: '网络检测', icon: RadarIcon, workspace: 'monitoring', order: 22,
    parent: '/latency', features: ['diagnostics'], preload: NetworkWorkbench.preload, render: () => <NetworkWorkbench /> },
] as const satisfies readonly PageEntry[]
