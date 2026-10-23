import { create } from 'zustand'

export type Locale = 'zh-CN' | 'en-US'

const STORAGE_KEY = 'apsp-locale'

function readInitialLocale(): Locale {
  try {
    const value = localStorage.getItem(STORAGE_KEY)
    if (value === 'en-US' || value === 'zh-CN') return value
  } catch { /* Storage may be unavailable. */ }
  return 'zh-CN'
}

type LocaleState = { locale: Locale; setLocale: (locale: Locale) => void }

/** Interface language shared by every entry point; the choice survives reloads. */
export const useLocaleStore = create<LocaleState>()(set => ({
  locale: typeof window === 'undefined' ? 'zh-CN' : readInitialLocale(),
  setLocale: locale => {
    try { localStorage.setItem(STORAGE_KEY, locale) } catch { /* Keep the in-memory selection. */ }
    set({ locale })
  },
}))
