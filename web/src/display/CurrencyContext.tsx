import { createContext, useContext, useEffect, useMemo, useState } from 'react'
import type { ReactNode } from 'react'
import { create } from 'zustand'
import { currencyCode } from './currency'
import type { ExchangeRates } from './currency'
import { useDashboardPoll } from './useDashboardPoll'

type Rates = { quote?: ExchangeRates; error: string; loading: boolean; reload: () => void }

function readCurrency() {
  try { return currencyCode(localStorage.getItem('apsp-display-currency')) } catch { return 'CNY' }
}

/** The visitor's display currency: interface state shared by every dashboard card. */
const useCurrencyStore = create<{ currency: string }>()(() => ({ currency: 'CNY' }))

/** Reference rates come from the dashboard's own read, not from a store. */
const RatesContext = createContext<Rates>({ error: '', loading: false, reload: () => {} })

// The last rates the read returned. It belongs to the read, not to interface
// state: it only bridges the first read after the dashboard mounts again, so
// cards do not flash "missing rate" on every visit.
let lastQuote: ExchangeRates | undefined

export const useCurrency = () => ({ currency: useCurrencyStore(state => state.currency), ...useContext(RatesContext) })

/** Reads the visitor's currency once and keeps the reference rates current. */
export function CurrencyProvider({ children }: { children: ReactNode }) {
  const rates = useDashboardPoll<ExchangeRates>('/api/dashboard/exchange-rates', 300_000, false)
  // The preference may have changed in the console since the last visit, so it is
  // read again whenever the dashboard mounts, before any card renders.
  useState(() => useCurrencyStore.setState({ currency: readCurrency() }))
  // Reference rates are global: keep the last ones while a remount's first read
  // is pending, and drop them once a read fails or access is withdrawn.
  const quote = rates.data ?? (rates.loading && !rates.error ? lastQuote : undefined)
  useEffect(() => { lastQuote = quote }, [quote])
  const value = useMemo(() => ({ quote, error: rates.error, loading: rates.loading, reload: rates.reload }), [quote, rates.error, rates.loading, rates.reload])
  return <RatesContext.Provider value={value}>{children}</RatesContext.Provider>
}
