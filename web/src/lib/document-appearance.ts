import { useEffect } from 'react'
import type { Density } from '@/stores/appearance'

/**
 * Each entry point (the console, the public dashboard, sign-in and the proxy-user
 * portal) owns the document's theme and density while it is shown. Colour
 * variables and `dark:` styles read them from <html>, so a theme set on a nested
 * element cannot override them and a theme chosen in one entry never carries
 * over into the next.
 */
export function useDocumentAppearance(theme: 'light' | 'dark', density: Density = 'comfortable') {
  useEffect(() => {
    document.documentElement.dataset.theme = theme
    document.documentElement.dataset.density = density
  }, [theme, density])
}
