import { useEffect } from 'react'

/**
 * Pages outside the administrator console (sign-in, proxy-user portal) follow the
 * visitor's system theme, since no saved preference applies to them.
 */
export function useSystemTheme() {
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () => { document.documentElement.dataset.theme = media.matches ? 'dark' : 'light' }
    apply()
    media.addEventListener('change', apply)
    return () => media.removeEventListener('change', apply)
  }, [])
}
