import { useEffect } from 'react'
import { usePreference } from './preferences'
import type { Locale } from '../i18n'
import { useI18n } from '../i18n'
import { resolvedTheme, useAppearanceStore } from '@/stores/appearance'
import type { Density, ThemePreference } from '@/stores/appearance'

/** Applies the administrator's saved theme, density and language to the document. */
export default function Appearance() {
  const preference = usePreference<{ theme: ThemePreference; density: Density; locale?: Locale }>('appearance')
  const { locale, setLocale } = useI18n()
  const setAppearance = useAppearanceStore(state => state.setAppearance)
  const theme = useAppearanceStore(state => state.theme)
  const density = useAppearanceStore(state => state.density)
  useEffect(() => {
    setAppearance({ theme: preference.value?.theme ?? 'system', density: preference.value?.density ?? 'comfortable' })
    const preferredLocale = preference.value?.locale
    if (preferredLocale && preferredLocale !== locale) setLocale(preferredLocale)
  }, [locale, preference.value, setAppearance, setLocale])
  useEffect(() => {
    const media = window.matchMedia('(prefers-color-scheme: dark)')
    const apply = () => {
      document.documentElement.dataset.theme = resolvedTheme(theme)
      document.documentElement.dataset.density = density
    }
    apply(); media.addEventListener('change', apply)
    const update = () => { void preference.reload() }
    window.addEventListener('apsp:appearance-updated', update)
    return () => { media.removeEventListener('change', apply); window.removeEventListener('apsp:appearance-updated', update) }
  }, [density, theme, preference.reload])
  return null
}
