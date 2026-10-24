import { useEffect, useRef } from 'react'
import { usePreference } from './preferences'
import type { Locale } from '../i18n'
import { useI18n } from '../i18n'
import { resolvedTheme, useAppearanceStore } from '@/stores/appearance'
import type { Density, ThemePreference } from '@/stores/appearance'
import { useLocaleStore } from '@/stores/locale'

type Saved = { theme: ThemePreference; density: Density; locale?: Locale }

/** Sent by the console header when the administrator picks a language there. */
export const localeChosenEvent = 'apsp:locale-chosen'

/**
 * Applies the administrator's saved theme, density and language to the document.
 * The saved preference applies when it is first read and whenever it changes
 * (saved here, in settings or in another window). A language chosen in the page
 * header is the administrator's new preference: it takes effect at once and is
 * saved, instead of being undone by the older saved value.
 */
export default function Appearance() {
  const preference = usePreference<Saved>('appearance')
  const { setLocale } = useI18n()
  const setAppearance = useAppearanceStore(state => state.setAppearance)
  const theme = useAppearanceStore(state => state.theme)
  const density = useAppearanceStore(state => state.density)
  const applied = useRef<string | undefined>(undefined)
  useEffect(() => {
    if (!preference.ready) return
    const saved = JSON.stringify(preference.value)
    if (applied.current === saved) return
    applied.current = saved
    setAppearance({ theme: preference.value?.theme ?? 'system', density: preference.value?.density ?? 'comfortable' })
    const preferredLocale = preference.value?.locale
    if (preferredLocale && preferredLocale !== useLocaleStore.getState().locale) setLocale(preferredLocale)
  }, [preference.ready, preference.value, setAppearance, setLocale])
  const latest = useRef(preference)
  latest.current = preference
  useEffect(() => {
    const chosen = (event: Event) => {
      const current = latest.current, next = (event as CustomEvent<Locale>).detail
      if (!current.ready || applied.current === undefined || current.value?.locale === next) return
      // A failed save keeps the choice for this session; the stored value applies next time.
      void current.save({ theme: current.value?.theme ?? 'system', density: current.value?.density ?? 'comfortable', locale: next }).catch(() => {})
    }
    window.addEventListener(localeChosenEvent, chosen)
    return () => window.removeEventListener(localeChosenEvent, chosen)
  }, [])
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
