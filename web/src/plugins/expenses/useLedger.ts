import { useMemo } from 'react'
import { useResource, useResourceGroup } from '../../hooks'
import { itemRow, ledgerTotals, serverRow, todayUtc, validItems, validLabels, validServers, validSummary } from './model'
import type { ItemPayment, LedgerRow, ServerPayment } from './model'
import { root } from './types'

/** Everything the ledger's views read, in one round, with the rows they all show. */
export function useLedger() {
  const data = useResourceGroup<{ items: unknown; labels: unknown; servers: unknown; summary: unknown }>({
    items: `${root}/subscriptions`, labels: `${root}/labels`, servers: `${root}/servers`, summary: `${root}/summary`,
  }, 60_000, { keep: true })
  const payments = useResource<{ items: ItemPayment[]; servers: ServerPayment[] }>(`${root}/payments`, 120_000, { keep: true })
  const items = validItems(data.items.data) ? data.items.data : undefined
  const labels = validLabels(data.labels.data) ? data.labels.data.entries : undefined
  const servers = validServers(data.servers.data) ? data.servers.data.entries : undefined
  const summary = validSummary(data.summary.data) ? data.summary.data : undefined
  // The ledger's own calendar day (its time zone) decides what is due; UTC until it is known.
  const today = summary?.from ?? todayUtc()
  const base = summary?.base_currency, rates = summary?.exchange
  const rows = useMemo<LedgerRow[]>(() => {
    const context = { today, base, rates, labels: labels ?? [] }
    return [...(items ?? []).map(item => itemRow(item, context)), ...(servers ?? []).map(server => serverRow(server, context))]
  }, [items, servers, labels, today, base, rates])
  const totals = useMemo(() => ledgerTotals(rows), [rows])
  const malformed = (['items', 'labels', 'servers', 'summary'] as const).some(key => data[key].data !== undefined && !{ items, labels, servers, summary }[key])
  const reload = () => { data.items.reload(); data.labels.reload(); data.servers.reload(); data.summary.reload(); payments.reload() }
  return {
    items, labels: labels ?? [], servers, summary, rows, totals, today, base, rates,
    budget: summary?.budget_units ? Number(summary.budget_units) / 10000 : null,
    payments: payments.data ? [...payments.data.items, ...payments.data.servers] : undefined,
    loaded: items !== undefined && servers !== undefined,
    error: data.items.error || data.servers.error || data.labels.error || data.summary.error,
    paymentsError: payments.error,
    malformed,
    /** Item writes wait until the list and labels they were made from are current. */
    stale: !data.items.isCurrent() || !data.labels.isCurrent() || Boolean(data.items.error || data.labels.error),
    reload,
  }
}
export type Ledger = ReturnType<typeof useLedger>
