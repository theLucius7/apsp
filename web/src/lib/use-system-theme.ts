import { useEffect, useState } from 'react'
import { useDocumentAppearance } from './document-appearance'

/**
 * Pages outside the administrator console (sign-in, member pages, proxy-user
 * portal) follow the visitor's system theme, since no saved preference applies
 * to them.
 */
export function useSystemTheme() {
  const [dark, setDark] = useState(() => window.matchMedia('(prefers-color-scheme: dark)').matches)
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const change = () => setDark(media.matches)
    media.addEventListener('change', change)
    return () => media.removeEventListener('change', change)
  }, [])
  useDocumentAppearance(dark ? 'dark' : 'light')
}
