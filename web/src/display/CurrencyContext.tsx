import { createContext, useContext, useState } from 'react'
import type { ReactNode } from 'react'
import { currencyCode } from './currency'
import type { ExchangeRates } from './currency'
import { useDashboardPoll } from './useDashboardPoll'

type CurrencyState = { currency: string; quote?: ExchangeRates; error: string; loading: boolean; reload: () => void }
const Context = createContext<CurrencyState>({ currency: 'CNY', error: '', loading: false, reload: () => {} })
export const useCurrency = () => useContext(Context)

export function CurrencyProvider({ children }: { children: ReactNode }) {
  const [currency] = useState(() => { try { return currencyCode(localStorage.getItem('apsp-display-currency')) } catch { return 'CNY' } })
  const rates = useDashboardPoll<ExchangeRates>('/api/dashboard/exchange-rates', 300_000, false)
  return <Context.Provider value={{ currency, quote: rates.data, error: rates.error, loading: rates.loading, reload: rates.reload }}>{children}</Context.Provider>
}
