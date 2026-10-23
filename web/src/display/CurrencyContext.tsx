import { useEffect, useState } from 'react'
import type { ReactNode } from 'react'
import { create } from 'zustand'
import { useShallow } from 'zustand/react/shallow'
import { currencyCode } from './currency'
import type { ExchangeRates } from './currency'
import { useDashboardPoll } from './useDashboardPoll'

type CurrencyState = { currency: string; quote?: ExchangeRates; error: string; loading: boolean; reload: () => void }

function readCurrency() {
  try { return currencyCode(localStorage.getItem('apsp-display-currency')) } catch { return 'CNY' }
}

/** Display currency and the latest reference rates, shared by every dashboard card. */
const useCurrencyStore = create<CurrencyState>()(() => ({ currency: 'CNY', error: '', loading: false, reload: () => {} }))

export const useCurrency = () => useCurrencyStore(useShallow(state => state))

/** Reads the visitor's currency once and keeps the reference rates current. */
export function CurrencyProvider({ children }: { children: ReactNode }) {
  const rates = useDashboardPoll<ExchangeRates>('/api/dashboard/exchange-rates', 300_000, false)
  // The preference may have changed in the console since the last visit, so it is
  // read again whenever the dashboard mounts, before any card renders.
  useState(() => useCurrencyStore.setState({ currency: readCurrency() }))
  useEffect(() => {
    useCurrencyStore.setState(state => ({
      // Reference rates are global: keep the last ones while a remount's first read
      // is pending, and drop them once a read fails or access is withdrawn.
      quote: rates.data ?? (rates.loading && !rates.error ? state.quote : undefined),
      error: rates.error, loading: rates.loading, reload: rates.reload,
    }))
  }, [rates.data, rates.error, rates.loading, rates.reload])
  return children
}
