import { useEffect, useRef, useState } from 'react'
import type { ReactNode } from 'react'
import { LogOutIcon, MenuIcon, PanelLeftCloseIcon, PanelLeftOpenIcon, XIcon } from 'lucide-react'
import { useResource } from '../hooks'
import CommandPalette from '../control-center/CommandPalette'
import Reauthentication from '../control-center/Reauthentication'
import Appearance from '../control-center/Appearance'
import { Brand, ErrorNotice, TierBadge } from '../components'
import { useI18n } from '../i18n'
import { navigation, sectionTabs, sidebarEntries } from './navigation'
import type { NavigationItem } from './navigation'
import AccountMenu from './AccountMenu'
import { preloadPages, preloadRoute } from './AdminPage'
import { resolveRoute } from './routes'
import { ConnectionNotice, ConnectionStatus } from './ConnectionStatus'
import { SectionContext } from './section'
import { cn } from '@/lib/utils'
import { readShellActor, writeShellActor } from '@/lib/shell-actor'
import type { ShellActor } from '@/lib/shell-actor'
import { useMediaQuery } from '@/lib/use-media-query'
import { useShellStore } from '@/stores/shell'
import { useFeatureStore } from '@/stores/features'
import { Button } from '@/components/ui/button'
import { Skeleton } from '@/components/ui/skeleton'
import { Tooltip, TooltipContent, TooltipProvider, TooltipTrigger } from '@/components/ui/tooltip'

type Props = {
  current?: NavigationItem
  /** Whether the route is this page itself rather than something inside it, such as one server. */
  exact?: boolean
  busy: boolean
  error: string
  onLogout: () => void
  children: ReactNode
}

const features: Record<string, string[]> = { servers: ['servers'], fleet: ['servers'], 'batch-enrollment': ['servers'], statistics: ['monitoring'], dashboard: ['monitoring'], latency: ['monitoring'], notifications: ['monitoring'], 'alert-settings': ['monitoring'], 'network-workbench': ['diagnostics'], 'network-configuration': ['network'], operations: ['operations', 'cloud'], backup: ['recovery'], expenses: ['cloud'], alicloud: ['cloud'], ddns: ['dns'], 'singbox-overview': ['proxy'], nodes: ['proxy'], 'proxy-users': ['proxy'], 'subscription-board': ['cloud'], groups: ['proxy'] }
const globalPages = new Set(['backup', 'statistics', 'dashboard', 'latency', 'notifications', 'alert-settings', 'expenses', 'alicloud', 'singbox-overview', 'nodes', 'proxy-users', 'subscription-board', 'groups'])

export default function AdminShell({ current, exact = true, busy, error, onLogout, children }: Props) {
  const { t } = useI18n()
  const resource = useResource<ShellActor>('/api/control-center/me', 30_000)
  const [remembered] = useState(readShellActor)
  // The navigation is drawn from the remembered actor until the panel confirms it,
  // so it never grows from two links to all of them after a reload.
  const actor = resource.data ?? remembered
  useEffect(() => { if (resource.data) writeShellActor(resource.data) }, [resource.data])
  useEffect(preloadPages, [])
  const visibleNavigation = navigation.filter(item => item.page === 'control-center' || item.page === 'security' || actor?.role === 'owner' || ((!globalPages.has(item.page) || actor?.all_servers) && features[item.page]?.some(feature => actor?.capabilities.includes(`${feature}:read`))))
  const desktop = useMediaQuery('(min-width: 768px)')
  const collapsed = useShellStore(state => state.collapsed)
  const setCollapsed = useShellStore(state => state.setCollapsed)
  const mobileOpen = useShellStore(state => state.mobileOpen)
  const setMobileOpen = useShellStore(state => state.setMobileOpen)
  // The brand returns to the first page this administrator may open.
  const home = `#${visibleNavigation[0]?.path ?? '/system/control-center'}`

  // Frozen features stay hidden unless the panel settings show them; the actor
  // response carries that panel-wide choice.
  const showFrozen = actor?.show_frozen_features
  useEffect(() => { if (showFrozen !== undefined) useFeatureStore.getState().setShowFrozen(showFrozen) }, [showFrozen])

  // The compact menu closes after navigation, on Escape and on a press outside the
  // phone header. Escape returns focus to the menu button when focus was in the menu;
  // an Escape a dialog already handled leaves the menu alone.
  const phoneHeader = useRef<HTMLElement>(null)
  const menuButton = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    const close = () => setMobileOpen(false)
    window.addEventListener('hashchange', close)
    return () => window.removeEventListener('hashchange', close)
  }, [setMobileOpen])
  useEffect(() => {
    if (desktop || !mobileOpen) return
    const keydown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape' || event.defaultPrevented) return
      const inMenu = phoneHeader.current?.contains(document.activeElement)
      setMobileOpen(false)
      if (inMenu) menuButton.current?.focus()
    }
    const pointerdown = (event: PointerEvent) => {
      if (!(event.target instanceof Node) || !phoneHeader.current?.contains(event.target)) setMobileOpen(false)
    }
    window.addEventListener('keydown', keydown)
    document.addEventListener('pointerdown', pointerdown)
    return () => { window.removeEventListener('keydown', keydown); document.removeEventListener('pointerdown', pointerdown) }
  }, [desktop, mobileOpen, setMobileOpen])

  const folded = desktop && collapsed
  const navigationList = actor
    ? <Navigation items={visibleNavigation} current={current} collapsed={folded} />
    : <NavigationSkeleton collapsed={folded} />

  // One layout tree for every width. Crossing the breakpoint only adds or removes the
  // sidebar and the phone header, so the page, its open dialogs and unsaved input stay
  // mounted when a tablet rotates or a window is resized.
  return <TooltipProvider delayDuration={200}>
    <div data-slot="app-shell" className="text-foreground flex h-dvh min-h-0 overflow-hidden">
      <Reauthentication /><Appearance />
      {desktop && <aside data-slot="sidebar" data-collapsed={collapsed} className={cn('bg-sidebar text-sidebar-foreground border-sidebar-border flex h-full min-h-0 shrink-0 flex-col border-r transition-[width] duration-150', collapsed ? 'w-14' : 'w-58')}>
        <div className={cn('flex h-16 shrink-0 items-center gap-2 px-3.5', collapsed ? 'justify-center px-2' : 'justify-between')}>
          {!collapsed && <a href={home} aria-label={t('apsp首页')} className="min-w-0 rounded-md focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"><Brand compact /></a>}
          <Button variant="ghost" size="icon-sm" aria-label={collapsed ? t('展开侧栏') : t('折叠侧栏')} onClick={() => setCollapsed(!collapsed)}>
            {collapsed ? <PanelLeftOpenIcon /> : <PanelLeftCloseIcon />}
          </Button>
        </div>
        <div className={cn('shrink-0 pb-2', collapsed ? 'px-2' : 'px-3')}><CommandPalette pages={visibleNavigation} compact={collapsed} /></div>
        <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2 scrollbar-thin">{navigationList}</div>
        {/* Who is signed in and whether the panel answers: one card at the foot of the sidebar. */}
        <div className={cn('shrink-0 pt-1', collapsed ? 'p-2' : 'px-2.5 pb-3')}>
          <AccountMenu actor={actor} busy={busy} onLogout={onLogout} collapsed={collapsed} />
        </div>
      </aside>}
      <div className="relative flex min-h-0 min-w-0 flex-1 flex-col">
        {!desktop && <aside ref={phoneHeader} data-slot="sidebar" className="bg-sidebar text-sidebar-foreground border-sidebar-border relative z-30 shrink-0 border-b">
          <div className="flex h-12 items-center gap-1.5 px-3">
            <Button ref={menuButton} variant="ghost" size="icon" aria-label={t('菜单')} aria-expanded={mobileOpen} aria-controls="admin-navigation" onClick={() => setMobileOpen(!mobileOpen)}>
              {mobileOpen ? <XIcon aria-hidden="true" /> : <MenuIcon aria-hidden="true" />}
            </Button>
            <a href={home} aria-label={t('apsp首页')} className="mr-auto min-w-0 rounded-md focus-visible:ring-3 focus-visible:ring-ring/50 focus-visible:outline-none"><Brand compact /></a>
            <ConnectionStatus compact />
            <CommandPalette pages={visibleNavigation} compact />
          </div>
          {/* The menu covers the page instead of pushing it, so opening it moves nothing. */}
          <div id="admin-navigation" hidden={!mobileOpen} className="bg-popover shadow-lifted absolute inset-x-0 top-full border-b">
            <div className="max-h-[calc(100dvh-3rem)] overflow-y-auto px-3 pt-1 pb-3">
              {navigationList}
              <div className="mt-2 border-t pt-2">
                <AccountMenu actor={actor} busy={busy} onLogout={onLogout} collapsed={false} />
                <Button data-slot="logout-button" variant="ghost" className="text-muted-foreground mt-1 w-full justify-start" disabled={busy} onClick={onLogout}><LogOutIcon aria-hidden="true" />{t('退出登录')}</Button>
              </div>
            </div>
          </div>
        </aside>}
        <ConnectionNotice />
        <SectionContext.Provider value={exact ? sectionTabs(current, visibleNavigation) : []}>
          <MainColumn current={current} error={error}>{children}</MainColumn>
        </SectionContext.Provider>
      </div>
    </div>
  </TooltipProvider>
}

function Navigation({ items, current, collapsed }: { items: readonly NavigationItem[]; current?: NavigationItem; collapsed: boolean }) {
  const { t } = useI18n()
  const showExperimental = useFeatureStore(state => state.showFrozen)
  // Experimental workspaces stay out of the way until the panel settings show them;
  // anyone already on one still sees where they are.
  const shown = items.filter(item => !item.tier || showExperimental || current?.page === item.page)
  const entries = sidebarEntries(shown)
  const activePath = current?.parent ?? current?.path
  const groups = [...new Set(entries.map(entry => entry.root.group))]
  return <nav aria-label={t('主导航')} className="grid gap-px">
    {groups.map((group, index) => <div key={group} className="contents">
      {collapsed
        ? index > 0 && <div className="bg-sidebar-border mx-2 my-1.5 h-px" aria-hidden="true" />
        : <div className="text-muted-foreground flex items-center gap-1.5 px-2.5 pt-4 pb-1.5 text-[11px] font-medium tracking-[0.08em] first:pt-1"><span className="truncate">{t(group)}</span>{entries.find(entry => entry.root.group === group)?.root.tier && <TierBadge tier="experimental" className="ml-auto" />}</div>}
      {entries.filter(entry => entry.root.group === group).map(({ root, href, path }) => {
        const active = activePath === path
        const Icon = root.icon
        const label = t(root.navLabel ?? root.label)
        // Pointing at an entry starts fetching its page, so the click rarely waits.
        const link = <a href={`#${href}`} aria-current={active ? 'page' : undefined} onPointerEnter={() => preloadRoute(resolveRoute(href))} onFocus={() => preloadRoute(resolveRoute(href))}
          className={cn(
            'text-sidebar-foreground/85 hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground group/nav relative flex h-9 min-w-0 items-center gap-2.5 rounded-[10px] px-2.5 text-[13px] outline-none transition-[color,background-color,box-shadow] duration-150 focus-visible:shadow-focus pointer-coarse:h-11',
            active && 'bg-sidebar-accent text-sidebar-accent-foreground shadow-soft font-medium',
            collapsed && 'justify-center px-0',
          )}>
          <Icon className={cn('size-4 shrink-0 transition-colors', active ? 'text-sidebar-primary' : 'opacity-75 group-hover/nav:opacity-100')} aria-hidden="true" />
          <span className={cn('truncate', collapsed && 'sr-only')}>{label}</span>
        </a>
        return <div key={path} className="contents">
          {collapsed ? <Tooltip><TooltipTrigger asChild>{link}</TooltipTrigger><TooltipContent side="right">{label}{root.tier && ` · ${t('实验')}`}</TooltipContent></Tooltip> : link}
        </div>
      })}
    </div>)}
  </nav>
}

/** First sign-in on this browser: the navigation's shape until the actor is known. */
function NavigationSkeleton({ collapsed }: { collapsed: boolean }) {
  return <div aria-hidden="true" className="grid gap-px">
    {Array.from({ length: 12 }, (_, index) => <div key={index} className="flex h-8 items-center gap-2.5 px-2">
      <Skeleton className="size-4 shrink-0 rounded" />{!collapsed && <Skeleton className="h-3 flex-1" style={{ maxWidth: `${50 + (index * 13) % 40}%` }} />}
    </div>)}
  </div>
}

function MainColumn({ current, error, children }: { current?: NavigationItem; error: string; children: ReactNode }) {
  const scroller = useRef<HTMLDivElement>(null)
  // A new page starts at the top of the scrolling column.
  useEffect(() => { scroller.current?.scrollTo({ top: 0 }) }, [current?.path])
  // The shell is one screen tall at every width, so this column owns vertical
  // scrolling on phones as well as on desktops. Each page's own header is the only
  // bar at the top of it and stays there while the page scrolls.
  return <div ref={scroller} data-slot="main-scroller" className="flex min-h-0 min-w-0 flex-1 flex-col overflow-y-auto">
    {/* Focusable so a dialog whose opener has gone can hand focus back to the page. */}
    <main key={current?.path ?? 'missing'} tabIndex={-1} className="mx-auto w-full max-w-[1600px] flex-1 px-4 pt-4 pb-10 outline-none md:px-6">
      <ErrorNotice message={error} />{children}
    </main>
  </div>
}
