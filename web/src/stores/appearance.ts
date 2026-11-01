import { create } from 'zustand'

export type ThemePreference = 'system' | 'light' | 'dark'
export type Density = 'comfortable' | 'compact'

// index.html reads the same key before the first paint, so a reload never flashes
// the wrong theme while the saved preference is on its way from the panel.
const STORAGE_KEY = 'apsp-appearance'

function readSaved(): { theme: ThemePreference; density: Density } {
  try {
    const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? 'null') as { theme?: unknown; density?: unknown } | null
    return { theme: value?.theme === 'light' || value?.theme === 'dark' ? value.theme : 'system', density: value?.density === 'compact' ? 'compact' : 'comfortable' }
  } catch { return { theme: 'system', density: 'comfortable' } }
}

type AppearanceState = {
  theme: ThemePreference
  density: Density
  setAppearance: (appearance: { theme?: ThemePreference; density?: Density }) => void
}

/**
 * Theme and density applied to the document. The administrator preference is
 * read from the panel; the public dashboard and the proxy portal follow the
 * system theme unless a page sets its own.
 */
export const useAppearanceStore = create<AppearanceState>()(set => ({
  ...(typeof window === 'undefined' ? { theme: 'system' as const, density: 'comfortable' as const } : readSaved()),
  setAppearance: ({ theme, density }) => set(state => {
    const next = { theme: theme ?? state.theme, density: density ?? state.density }
    try { localStorage.setItem(STORAGE_KEY, JSON.stringify(next)) } catch { /* The in-memory choice still applies. */ }
    return next
  }),
}))

export function resolvedTheme(theme: ThemePreference): 'light' | 'dark' {
  if (theme !== 'system') return theme
  return typeof window !== 'undefined' && window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}
