import { CalendarClockIcon, RssIcon } from 'lucide-react'
import { lazyPage } from '@/lib/pages'
import type { PageEntry } from '@/lib/pages'

// The ledger takes the opened entry from the address.
export const Expenses = lazyPage(() => import('./expenses/Expenses'), true)
const SubscriptionBoard = lazyPage(() => import('./subscriptions/SubscriptionBoard'), true)

export const pages = [
  // The administrator's own paid subscriptions (Wallos style) at a glance (ADR 0117).
  { page: 'subscription-board', path: '/subscriptions', label: '订阅看板', icon: RssIcon, workspace: 'overview', order: 40,
    parent: '/home', features: ['cloud'], allServers: true, preload: SubscriptionBoard.preload, render: () => <SubscriptionBoard /> },
  // ADR 0087: costs and renewals sit with the overview.
  { page: 'expenses', path: '/expenses', label: '费用与续费', icon: CalendarClockIcon, workspace: 'overview', order: 50,
    features: ['cloud'], allServers: true, preload: Expenses.preload },
] as const satisfies readonly PageEntry[]
