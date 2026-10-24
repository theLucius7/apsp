import { useEffect, useRef } from 'react'
import type { ReactNode } from 'react'
import { m } from 'motion/react'
import { LogOutIcon, MenuIcon, PanelLeftCloseIcon, PanelLeftOpenIcon, XIcon } from 'lucide-react'
import { useDefaultLayout, usePanelRef } from 'react-resizable-panels'
import { useResource } from '../hooks'
import CommandPalette from '../control-center/CommandPalette'
import Reauthentication from '../control-center/Reauthentication'
import Appearance from '../control-center/Appearance'
import { Brand, ErrorNotice } from '../components'
import { LanguageSelect, useI18n } from '../i18n'
import { navigation } from './navigation'
import type { NavigationItem } from './navigation'
import { cn } from '@/lib/utils'
import { pageMotion, transition } from '@/lib/motion'
import { layoutStorage } from '@/lib/layout-storage'
import { useMediaQuery } from '@/lib/use-media-query'
import { useShellStore } from '@/stores/shell'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import { ResizableHandle, ResizablePanel, ResizablePanelGroup } from '@/components/ui/resizable'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'

type Actor = { role: string; display_name: string; all_servers: boolean; capabilities: string[] }

type Props = {
  current?: NavigationItem
  busy: boolean
  error: string
  onLogout: () => void
  children: ReactNode
}

const features: Record<string, string[]> = { servers: ['servers'], fleet: ['servers'], statistics: ['monitoring'], dashboard: ['monitoring'], latency: ['monitoring'], notifications: ['monitoring'], 'alert-settings': ['monitoring'], 'network-workbench': ['diagnostics'], 'network-configuration': ['network'], operations: ['operations', 'recovery', 'cloud'], expenses: ['cloud'], commerce: ['proxy'], alicloud: ['cloud'], ddns: ['dns'], 'singbox-overview': ['proxy'], nodes: ['proxy'], 'proxy-users': ['proxy'], groups: ['proxy'] }
const globalPages = new Set(['statistics', 'dashboard', 'latency', 'notifications', 'alert-settings', 'expenses', 'commerce', 'alicloud', 'singbox-overview', 'nodes', 'proxy-users', 'groups'])

export default function AdminShell({ current, busy, error, onLogout, children }: Props) {
  const { t } = useI18n()
  const actor = useResource<Actor>('/api/control-center/me')
  const visibleNavigation = navigation.filter(item => item.page === 'control-center' || item.page === 'security' || actor.data?.role === 'owner' || ((!globalPages.has(item.page) || actor.data?.all_servers) && features[item.page]?.some(feature => actor.data?.capabilities.includes(`${feature}:read`))))
  const desktop = useMediaQuery('(min-width: 768px)')
  const collapsed = useShellStore(state => state.collapsed)
  const setCollapsed = useShellStore(state => state.setCollapsed)
  const mobileOpen = useShellStore(state => state.mobileOpen)
  const setMobileOpen = useShellStore(state => state.setMobileOpen)
  const sidebarPanel = usePanelRef()
  const layout = useDefaultLayout({ id: 'apsp-admin-shell', storage: layoutStorage, onlySaveAfterUserInteractions: true })

  // The compact menu closes after navigation and on Escape.
  useEffect(() => {
    const close = () => setMobileOpen(false)
    window.addEventListener('hashchange', close)
    return () => window.removeEventListener('hashchange', close)
  }, [setMobileOpen])
  // The sidebar panel follows the remembered collapsed state.
  useEffect(() => {
    const panel = sidebarPanel.current
    if (!desktop || !panel) return
    if (collapsed && !panel.isCollapsed()) panel.collapse()
    if (!collapsed && panel.isCollapsed()) panel.expand()
  }, [collapsed, desktop, sidebarPanel])

  const navigationList = <Navigation items={visibleNavigation} current={current} collapsed={desktop && collapsed} />
  const account = <AccountButton actor={actor.data} busy={busy} onLogout={onLogout} collapsed={desktop && collapsed} />
  const main = <MainColumn current={current} pages={visibleNavigation} error={error} mobile={!desktop}>{children}</MainColumn>

  return <TooltipProvider delayDuration={200}>
    <div data-slot="app-shell" className="bg-background text-foreground flex h-dvh min-h-0 flex-col overflow-hidden md:flex-row">
      <Reauthentication /><Appearance />
      {desktop ? <ResizablePanelGroup id="apsp-admin-shell" orientation="horizontal" defaultLayout={layout.defaultLayout} onLayoutChanged={layout.onLayoutChanged}>
        <ResizablePanel id="sidebar" panelRef={sidebarPanel} defaultSize={248} minSize={208} maxSize={360} collapsible collapsedSize={60}
          onResize={size => { const folded = size.inPixels < 120; if (folded !== useShellStore.getState().collapsed) setCollapsed(folded) }}>
          <aside data-slot="sidebar" data-collapsed={collapsed} className="bg-sidebar text-sidebar-foreground flex h-full min-h-0 flex-col border-r">
            <div className={cn('flex h-14 shrink-0 items-center gap-2 border-b px-3', collapsed ? 'justify-center px-2' : 'justify-between')}>
              {!collapsed && <a href="#/servers" aria-label={t('apsp首页')} className="min-w-0 rounded-md focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"><Brand /></a>}
              <Button variant="ghost" size="icon-sm" aria-label={collapsed ? t('展开侧栏') : t('折叠侧栏')} onClick={() => setCollapsed(!collapsed)}>
                {collapsed ? <PanelLeftOpenIcon /> : <PanelLeftCloseIcon />}
              </Button>
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto px-2 py-3 scrollbar-thin">{navigationList}</div>
            <div className="shrink-0 border-t p-2">{account}</div>
          </aside>
        </ResizablePanel>
        <ResizableHandle withHandle aria-label={t('调整侧栏宽度')} />
        <ResizablePanel id="main" minSize="50">{main}</ResizablePanel>
      </ResizablePanelGroup> : <>
        <aside data-slot="sidebar" className="bg-sidebar text-sidebar-foreground shrink-0 border-b">
          <div className="flex h-14 items-center gap-2 px-4">
            <a href="#/servers" aria-label={t('apsp首页')} className="mr-auto min-w-0 rounded-md focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"><Brand /></a>
            <AccountButton actor={actor.data} busy={busy} onLogout={onLogout} collapsed={false} compact />
            <Button variant="outline" aria-expanded={mobileOpen} aria-controls="admin-navigation" onClick={() => setMobileOpen(!mobileOpen)}>
              {mobileOpen ? <XIcon /> : <MenuIcon />}<span>{t('菜单')}</span>
            </Button>
          </div>
          <m.div id="admin-navigation" initial={false}
            animate={mobileOpen ? { height: 'auto', opacity: 1, display: 'block' } : { height: 0, opacity: 0, transitionEnd: { display: 'none' } }}
            transition={transition.base} className="overflow-hidden">
            <div className="max-h-[70dvh] overflow-y-auto border-t px-3 py-3">
              {navigationList}
              <p className="text-muted-foreground mt-3 border-t px-2 pt-3 text-xs">{t('当前账号')}：{actor.data?.display_name ?? t('管理员')}</p>
            </div>
          </m.div>
        </aside>
        {main}
      </>}
    </div>
  </TooltipProvider>
}

function Navigation({ items, current, collapsed }: { items: readonly NavigationItem[]; current?: NavigationItem; collapsed: boolean }) {
  const { t } = useI18n()
  return <nav aria-label={t('主导航')} className="grid gap-0.5 max-md:grid-cols-2 max-[360px]:grid-cols-1">
    {items.map((item, index) => {
      const active = current?.path === item.path
      const Icon = item.icon
      const link = <a href={`#${item.path}`} aria-current={active ? 'page' : undefined}
        className={cn(
          'text-sidebar-foreground/80 hover:bg-sidebar-accent hover:text-sidebar-accent-foreground focus-visible:ring-sidebar-ring/50 relative flex h-9 min-w-0 items-center gap-2.5 rounded-lg px-2.5 text-sm font-medium transition-colors outline-none focus-visible:ring-3 pointer-coarse:h-10',
          active && 'bg-sidebar-accent text-sidebar-accent-foreground',
          collapsed && 'justify-center px-0',
        )}>
        {active && <m.span layoutId="sidebar-active" transition={transition.spring} className="bg-sidebar-primary absolute inset-y-1.5 left-0 w-0.5 rounded-full" aria-hidden="true" />}
        <Icon className="size-4 shrink-0" aria-hidden="true" />
        <span className={cn('truncate', collapsed && 'sr-only')}>{t(item.label)}</span>
      </a>
      return <div key={item.path} className="contents">
        {items[index - 1]?.group !== item.group && (collapsed
          ? index > 0 && <div className="bg-sidebar-border mx-2 my-2 h-px" aria-hidden="true" />
          : <div className="text-muted-foreground px-2.5 pt-4 pb-1 text-xs font-medium first:pt-1 max-md:col-span-full">{t(item.group)}</div>)}
        {collapsed ? <Tooltip><TooltipTrigger asChild>{link}</TooltipTrigger><TooltipContent side="right">{t(item.label)}</TooltipContent></Tooltip> : link}
      </div>
    })}
  </nav>
}

function AccountButton({ actor, busy, onLogout, collapsed, compact = false }: { actor?: Actor; busy: boolean; onLogout: () => void; collapsed: boolean; compact?: boolean }) {
  const { t } = useI18n()
  const role = actor?.role === 'viewer' ? t('只读 · ') : actor?.role === 'operator' ? t('运维 · ') : ''
  const name = actor?.display_name ?? t('管理员')
  // Phones keep logout one tap away without spending header width on the name.
  if (compact) return <Button data-slot="logout-button" variant="ghost" size="icon" disabled={busy} onClick={onLogout} title={`${name} · ${t('退出登录')}`}>
    <LogOutIcon aria-hidden="true" /><span className="sr-only">{role}{name} · {t('退出登录')}</span>
  </Button>
  return <button type="button" data-slot="logout-button" disabled={busy} onClick={onLogout}
    className={cn('hover:bg-sidebar-accent focus-visible:ring-sidebar-ring/50 flex w-full min-w-0 items-center gap-2.5 rounded-lg p-1.5 text-left transition-colors outline-none focus-visible:ring-3 disabled:opacity-50', collapsed && 'justify-center')}>
    <span className="bg-primary/10 text-primary grid size-8 shrink-0 place-items-center rounded-full text-xs font-semibold max-md:hidden" aria-hidden="true">管</span>
    <span className={cn('grid min-w-0 flex-1 leading-tight', collapsed && 'sr-only')}>
      <strong className="truncate text-sm font-medium">{actor?.display_name ?? t('管理员')}</strong>
      <small className="text-muted-foreground truncate text-xs">{role}{t('退出登录')}</small>
    </span>
    <LogOutIcon className={cn('text-muted-foreground size-4 shrink-0', collapsed && 'hidden')} aria-hidden="true" />
  </button>
}

function MainColumn({ current, pages, error, mobile, children }: { current?: NavigationItem; pages: readonly NavigationItem[]; error: string; mobile: boolean; children: ReactNode }) {
  const { t } = useI18n()
  const scroller = useRef<HTMLDivElement>(null)
  // A new page starts at the top of the scrolling column.
  useEffect(() => { scroller.current?.scrollTo({ top: 0 }) }, [current?.path])
  return <div ref={scroller} className={cn('flex min-h-0 min-w-0 flex-1 flex-col', !mobile && 'h-full overflow-y-auto')}>
    {/* The bar sits in a fixed-height scrolling column, so it must not shrink below
        its content. At tablet widths the breadcrumb drops the group (the sidebar shows
        it) and the search button narrows, so the controls stay on one row. */}
    <div data-slot="topbar" className="bg-background/85 supports-backdrop-filter:backdrop-blur-sm sticky top-0 z-20 flex min-h-14 shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-b px-4 py-2 md:px-8">
      <nav aria-label={t('当前位置')} className="text-muted-foreground mr-auto flex min-w-0 items-center gap-2 text-sm">
        {current && <><span className="truncate max-lg:hidden">{t(current.group)}</span><span aria-hidden="true" className="max-lg:hidden">/</span></>}
        <strong aria-current="page" className="text-foreground truncate font-medium">{t(current?.label ?? '页面不存在')}</strong>
      </nav>
      <CommandPalette pages={pages} />
      <LanguageSelect className="shrink-0" />
      <Badge variant="success" className="max-xl:hidden"><span className="size-1.5 rounded-full bg-current" aria-hidden="true" />{t('管理员会话已登录')}</Badge>
    </div>
    <m.main key={current?.path ?? 'missing'} {...pageMotion} className="mx-auto w-full max-w-[1480px] flex-1 px-4 py-6 md:px-8 md:py-8">
      <ErrorNotice message={error} />{children}
    </m.main>
    <footer className="text-muted-foreground mx-4 flex justify-between gap-4 border-t py-4 text-xs md:mx-8">
      <span className="tracking-[0.2em]">{t('apsp')}</span><span>{t('清晰掌握，自在连接。')}</span>
    </footer>
  </div>
}
