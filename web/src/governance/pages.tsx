import { DatabaseBackupIcon, KeyRoundIcon, Settings2Icon, ShieldCheckIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

const Settings = lazyPage(() => import('./system/Settings'))
const Backup = lazyPage(() => import('./system/Backup'))
const ControlCenter = lazyPage(() => import('./control-center/ControlCenter'))
const Security = lazyPage(() => import('./system/Security'))

const settings = '/settings'
export const pages = [
  { page: 'settings', path: settings, label: '面板设置', navLabel: '设置', icon: Settings2Icon, workspace: 'system', order: 10,
    preload: Settings.preload, render: () => <Settings /> },
  { page: 'backup', path: '/settings/backup', label: '备份与恢复', icon: DatabaseBackupIcon, workspace: 'system', order: 12,
    parent: settings, features: ['recovery'], allServers: true, preload: Backup.preload, render: () => <Backup /> },
  { page: 'control-center', path: '/settings/admin', label: '管理与安全', icon: ShieldCheckIcon, workspace: 'system', order: 13,
    parent: settings, everyAdministrator: true, preload: ControlCenter.preload, render: () => <ControlCenter /> },
  { page: 'security', path: '/settings/account', label: '账号安全', icon: KeyRoundIcon, workspace: 'system', order: 14,
    parent: settings, everyAdministrator: true, preload: Security.preload, render: () => <Security /> },
] as const satisfies readonly PageEntry[]
