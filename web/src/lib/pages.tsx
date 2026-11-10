import { useEffect, useState } from 'react'
import type { ComponentType, ReactNode } from 'react'
import type { LucideIcon } from 'lucide-react'
import { ErrorNotice, PageSkeleton } from '../components'
import { reloadForNewRelease } from './new-release'
import type { Tier } from './tiers'

/**
 * The sidebar section a page appears in. The application decides their names and order;
 * each domain only says where its pages belong, so moving a page between sections never
 * moves its code.
 */
export type Workspace = 'overview' | 'proxy' | 'servers' | 'settings' | 'experimental'

/**
 * One administrator page, declared by the domain that owns it (`<domain>/pages.ts`). The
 * application collects these declarations into the sidebar, the address table and the
 * page loader, so adding a page only changes its domain's directory.
 */
export type PageEntry = {
  page: string
  path: string
  /** The page's own name: its title and its tab. */
  label: string
  icon: LucideIcon
  workspace: Workspace
  /** Position in the workspace, ascending. */
  order: number
  /** The sidebar entry this page is a tab of; pages without one are sidebar entries. */
  parent?: string
  /** The sidebar's name for an entry whose tabs go by more specific names. */
  navLabel?: string
  /** Set for downgraded features, which stay usable but are labelled. */
  tier?: Tier
  /** Earlier addresses that still open this page. */
  aliases?: readonly string[]
  /** Not in the sidebar: the page is reached from another one, whose entry stays marked. */
  within?: string
  /** Any of these `<feature>:read` capabilities lets an administrator open the page; without it only owners may. */
  features?: readonly string[]
  /** Every administrator may open the page, whatever their capabilities. */
  everyAdministrator?: boolean
  /** The page shows data of every server, so the administrator must see all servers. */
  allServers?: boolean
  /** Starts fetching the page's code ahead of a visit. */
  preload?: () => void
  /** Draws the page for its plain address; pages that take parameters from the address are drawn by the application. */
  render?: () => ReactNode
}

export type LazyPage<P extends object> = ComponentType<P> & { preload: () => void }

const preloads: (() => void)[] = []

/**
 * Loads a page on its first visit. Unlike React.lazy with Suspense, completion
 * is a plain state update, so revealing the page never waits on timers; a
 * page loaded once renders synchronously afterwards. A failed load usually
 * means a new release replaced the old files, so it recovers by itself.
 */
export function lazyPage<P extends object>(load: () => Promise<{ default: ComponentType<P> }>, preload = false): LazyPage<P> {
  let loaded: ComponentType<P> | undefined
  let pending: Promise<ComponentType<P>> | undefined
  const start = () => (pending ??= load().then(module => (loaded = module.default)))
  if (preload) preloads.push(() => { void start().catch(() => { pending = undefined }) })
  function LazyPage(props: P) {
    const [page, setPage] = useState<ComponentType<P> | undefined>(() => loaded)
    const [failed, setFailed] = useState(false)
    useEffect(() => {
      if (page) return
      let active = true
      start().then(component => { if (active) setPage(() => component) }, () => {
        pending = undefined
        if (!active) return
        void reloadForNewRelease().then(reloading => { if (active && !reloading) setFailed(true) })
      })
      return () => { active = false }
    }, [page])
    const Page = page
    if (Page) return <Page {...props} />
    return failed ? <ErrorNotice message="页面加载失败，请检查网络后刷新重试。" retry={() => window.location.reload()} /> : <PageSkeleton />
  }
  return Object.assign(LazyPage, { preload: () => void start().catch(() => { pending = undefined }) })
}

/** Fetches the everyday pages while idle, so moving between them never waits on the network. */
export function preloadPages() {
  const connection = (navigator as Navigator & { connection?: { saveData?: boolean } }).connection
  if (connection?.saveData) return
  const run = () => { for (const page of preloads) page() }
  if (typeof window.requestIdleCallback === 'function') window.requestIdleCallback(run, { timeout: 4000 })
  else setTimeout(run, 1500)
}
