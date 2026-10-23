import { create } from 'zustand'

export type ThemePreference = 'system' | 'light' | 'dark'
export type Density = 'comfortable' | 'compact'

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
  theme: 'system',
  density: 'comfortable',
  setAppearance: ({ theme, density }) => set(state => ({ theme: theme ?? state.theme, density: density ?? state.density })),
}))

export function resolvedTheme(theme: ThemePreference): 'light' | 'dark' {
  if (theme !== 'system') return theme
  return typeof window !== 'undefined' && window.matchMedia?.('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
}
