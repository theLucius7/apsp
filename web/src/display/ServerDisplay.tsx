import { useState } from 'react'
import { useNow } from './useNow'
import { CircleUserIcon, MoonIcon, SunIcon } from 'lucide-react'
import { Brand, PageHeader } from '../components'
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

/** The live clock every card measures freshness against, and which view is open. */
function DisplayContent({ serverId, embedded = false }: { serverId?: number; embedded?: boolean }) {
  const now = useNow(5000)
  return serverId ? <ServerView key={serverId} id={serverId} now={now} embedded={embedded} /> : <Overview now={now} />
}

/** The standalone page visitors see when the panel makes the dashboard public. */
function PublicDisplay({ serverId }: { serverId?: number }) {
  const { t } = useI18n()
  const { theme, toggle } = useTheme()
  // The dashboard's own light/dark choice applies to the whole document, not only
  // this wrapper, so a dark console theme left on <html> cannot bleed into it.
  useDocumentAppearance(theme)
  return <CurrencyProvider><div data-slot="server-display" className="text-foreground flex min-h-dvh flex-col">
    <a className="bg-card text-foreground sr-only z-30 rounded-md p-2.5 focus:not-sr-only focus:absolute focus:top-2 focus:left-2" href="#display-main" onClick={event => { event.preventDefault(); document.getElementById('display-main')?.focus() }}>{t('跳到服务器信息')}</a>
    <header className="bg-background/95 supports-backdrop-filter:backdrop-blur-sm sticky top-0 z-20 border-b">
      <div className="mx-auto flex min-h-12 w-full max-w-[1600px] items-center justify-between gap-3 px-4 md:px-6">
        <a href={dashboardHome} aria-label={t('apsp服务器看板')} className="rounded-md focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"><Brand compact /></a>
        <div className="flex items-center gap-1.5">
          <LanguageSelect />
          <Button variant="ghost" size="icon" aria-label={theme === 'dark' ? t('切换浅色主题') : t('切换深色主题')} title={theme === 'dark' ? t('浅色主题') : t('深色主题')} onClick={toggle}>{theme === 'dark' ? <SunIcon /> : <MoonIcon />}</Button>
          <a className={buttonVariants({ variant: 'ghost', size: 'icon' })} aria-label={t('进入后台')} title={t('进入后台')} href={serverId ? `#/servers/${serverId}` : '#/servers'}><CircleUserIcon /></a>
        </div>
      </div>
    </header>
    <main className="mx-auto w-full max-w-[1600px] flex-1 px-4 py-4 outline-none md:px-6" id="display-main" tabIndex={-1}><DisplayContent serverId={serverId} /></main>
  </div></CurrencyProvider>
}

/** Signed in, the board is one more console page: same shell, same theme. */
export default function ServerDisplay({ serverId, embedded = false }: { serverId?: number; embedded?: boolean }) {
  if (!embedded) return <PublicDisplay serverId={serverId} />
  return <CurrencyProvider><div data-slot="server-display" className="min-w-0">
    {!serverId && <PageHeader title="服务器看板" description="各服务器的实时状态与资源占用；可在「面板设置」公开给访客。" />}
    <DisplayContent serverId={serverId} embedded />
  </div></CurrencyProvider>
}
