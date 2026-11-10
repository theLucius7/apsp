import { CalendarClockIcon, RssIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

// The ledger takes the opened entry from the address.
export const Expenses = lazyPage(() => import('./expenses/Expenses'), true)
const SubscriptionBoard = lazyPage(() => import('./subscriptions/SubscriptionBoard'), true)

export const pages = [
  // The administrator's own paid subscriptions (Wallos style) at a glance (ADR 0117).
  { page: 'subscription-board', path: '/subscriptions', label: '付费服务', icon: RssIcon, workspace: 'costs', order: 11,
    parent: '/expenses', features: ['cloud'], allServers: true, preload: SubscriptionBoard.preload, render: () => <SubscriptionBoard /> },
  { page: 'expenses', path: '/expenses', label: '续费项目', icon: CalendarClockIcon, workspace: 'costs', order: 10,
    features: ['cloud'], allServers: true, preload: Expenses.preload },
] as const satisfies readonly PageEntry[]
