import { useEffect, useState } from 'react'
import { CircleUserIcon, MoonIcon, SunIcon } from 'lucide-react'
import { Brand } from '../components'
import { LanguageSelect, useI18n } from '../i18n'
import Overview from './Overview'
import ServerView from './ServerView'
import { dashboardHome } from './dashboard'
import { CurrencyProvider } from './CurrencyContext'
import { useDocumentAppearance } from '@/lib/document-appearance'
import { Button, buttonVariants } from '@/components/ui/button'

function useTheme() {
  const [theme, setTheme] = useState<'light' | 'dark'>(() => {
    try { const saved = localStorage.getItem('apsp-display-theme'); if (saved === 'light' || saved === 'dark') return saved } catch { /* Storage may be disabled. */ }
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light'
  })
  const toggle = () => setTheme(current => {
    const next = current === 'light' ? 'dark' : 'light'
    try { localStorage.setItem('apsp-display-theme', next) } catch { /* The in-memory choice still works. */ }
    return next
  })
  return { theme, toggle }
}

export default function ServerDisplay({ serverId }: { serverId?: number }) {
  const { t } = useI18n()
  const { theme, toggle } = useTheme()
  // The dashboard's own light/dark choice applies to the whole document, not only
  // this wrapper, so a dark console theme left on <html> cannot bleed into it.
  useDocumentAppearance(theme)
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    const timer = window.setInterval(() => { if (document.visibilityState === 'visible') setNow(Date.now()) }, 5000)
    const visible = () => { if (document.visibilityState === 'visible') setNow(Date.now()) }
    document.addEventListener('visibilitychange', visible)
    return () => { window.clearInterval(timer); document.removeEventListener('visibilitychange', visible) }
  }, [])
  return <CurrencyProvider><div data-slot="server-display" className="bg-background text-foreground flex min-h-dvh flex-col">
    <a className="bg-card text-foreground sr-only z-30 rounded-md p-2.5 focus:not-sr-only focus:absolute focus:top-2 focus:left-2" href="#display-main" onClick={event => { event.preventDefault(); document.getElementById('display-main')?.focus() }}>{t('跳到服务器信息')}</a>
    <header className="bg-background/80 supports-backdrop-filter:backdrop-blur-md sticky top-0 z-20 border-b">
      <div className="mx-auto flex min-h-16 w-full max-w-[1440px] items-center justify-between gap-3 px-4 sm:px-6 lg:px-8">
        <a href={dashboardHome} aria-label={t('apsp服务器看板')} className="rounded-md focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"><Brand className="[&_small]:hidden" /></a>
        <div className="flex items-center gap-1.5">
          <LanguageSelect />
          <Button variant="ghost" size="icon" aria-label={theme === 'dark' ? t('切换浅色主题') : t('切换深色主题')} title={theme === 'dark' ? t('浅色主题') : t('深色主题')} onClick={toggle}>{theme === 'dark' ? <SunIcon /> : <MoonIcon />}</Button>
          <a className={buttonVariants({ variant: 'ghost', size: 'icon' })} aria-label={t('进入后台')} title={t('进入后台')} href={serverId ? `#/servers/${serverId}` : '#/servers'}><CircleUserIcon /></a>
        </div>
      </div>
    </header>
    <main className="mx-auto w-full max-w-[1440px] flex-1 px-4 pt-5 pb-7 outline-none sm:px-6 lg:px-8" id="display-main" tabIndex={-1}>{serverId ? <ServerView key={serverId} id={serverId} now={now} /> : <Overview now={now} />}</main>
  </div></CurrencyProvider>
}
