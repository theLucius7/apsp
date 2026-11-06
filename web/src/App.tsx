import { lazy, Suspense, useEffect, useRef, useState } from 'react'
import { api, errorMessage } from './api'
import { BrandMark, PageSkeleton } from './components'
import { useAction } from './hooks'
import AdminPage, { preloadRoute } from './app/AdminPage'
import PageBoundary from './app/PageBoundary'
import AdminShell from './app/AdminShell'
import Login from './app/Login'
import { currentNavigation, homePath } from './app/navigation'
import { resolveRoute } from './app/routes'
import { useI18n } from './i18n'
import { useSessionStore } from './stores/session'
import { acceptHashChange, guardLinks } from '@/lib/leave-guard'
import { Button } from '@/components/ui/button'

const ServerDisplay = lazy(() => import('./display/ServerDisplay'))
const MemberLogin = lazy(() => import('./plugins/singbox/accounts/MemberLogin'))
const MemberJoin = lazy(() => import('./plugins/singbox/accounts/Join'))
const UserPortal = lazy(() => import('./plugins/singbox/UserPortal'))

/**
 * The first screen, identical to the splash index.html draws before any script runs,
 * so loading looks like one screen from the first frame. A lost connection stays
 * here and keeps trying: it says nothing about the session.
 */
function Boot({ message, onRetry }: { message?: string; onRetry?: () => void }) {
  const { t } = useI18n()
  return <div data-slot="boot" className="boot-splash">
    <BrandMark />
    {message ? <div role="status" className="grid max-w-sm justify-items-center gap-3 text-sm">
      <p className="text-foreground font-medium">{t('暂时连不上面板')}</p>
      <p className="text-muted-foreground">{message} {t('正在自动重试，恢复后会直接进入。')}</p>
      {onRetry && <Button variant="outline" size="sm" onClick={onRetry}>{t('立即重试')}</Button>}
    </div> : <span className="boot-bar" role="status" aria-label={t('正在加载…')} />}
  </div>
}

export default function App() {
  const { t } = useI18n()
  const session = useSessionStore(state => state.authenticated)
  const publicDashboard = useSessionStore(state => state.publicDashboard)
  const notice = useSessionStore(state => state.notice)
  const accessRevision = useSessionStore(state => state.revision)
  const bootError = useSessionStore(state => state.bootError)
  const [bootAttempt, setBootAttempt] = useState(0)
  const [path, setPath] = useState(window.location.hash.slice(1) || '/dashboard')
  const shown = useRef(path)
  shown.current = path
  const action = useAction()
  const translate = useRef(t)
  translate.current = t
  const bindRouter = () => {
    const unauthorized = () => {
      useSessionStore.getState().signedOut(useSessionStore.getState().authenticated ? translate.current('登录已过期，请重新登录。') : undefined)
    }
    // Back, forward and direct hash writes reach the router here; unsaved input decides.
    const hash = () => { if (acceptHashChange(`#${shown.current}`)) setPath(window.location.hash.slice(1) || '/dashboard') }
    window.addEventListener('apsp:unauthorized', unauthorized)
    window.addEventListener('hashchange', hash)
    return () => {
      window.removeEventListener('apsp:unauthorized', unauthorized)
      window.removeEventListener('hashchange', hash)
    }
  }

  useEffect(() => guardLinks(), [])
  // The page's code downloads while the session is checked; a bare address lands on the overview.
  useEffect(() => { preloadRoute(resolveRoute(window.location.hash.length > 2 ? shown.current : homePath)) }, [])

  useEffect(() => {
    // Proxy users and members have their own sign-in; on a user's own hostname the
    // administrator's endpoints do not exist at all.
    if (['proxy-portal', 'member-login', 'member-join'].includes(resolveRoute(shown.current).page)) return bindRouter()
    const store = useSessionStore.getState()
    const controller = new AbortController()
    let active = true
    let attempts = 0
    let retry: number | undefined
    const check = () => void api<{ authenticated: boolean; public_dashboard: boolean }>(
      '/api/dashboard/access', 'GET', undefined, controller.signal,
    ).then(access => {
      if (active) {
        store.setAccess({ authenticated: access.authenticated, publicDashboard: access.public_dashboard })
        // Signed-in administrators land on the overview of the proxy network;
        // without access to it they start at the servers or their own settings.
        // An explicit #/dashboard keeps the server board for those who may see it.
        const landing = window.location.hash === '' || window.location.hash === '#/'
        if (access.authenticated && (landing || window.location.hash === '#/dashboard')) {
          void api<{ role: string; all_servers: boolean; capabilities: string[] }>('/api/control-center/me').then(actor => {
            if (!active) return
            const owner = actor.role === 'owner'
            const fallback = actor.capabilities.includes('servers:read') ? '/servers' : '/system/control-center'
            if (landing) window.location.hash = owner || (actor.all_servers && actor.capabilities.includes('proxy:read')) ? homePath : fallback
            else if (!owner && (!actor.all_servers || !actor.capabilities.includes('monitoring:read'))) window.location.hash = fallback
          }).catch(() => { if (active && landing) window.location.hash = homePath })
        }
      }
    }).catch(error => {
      // No answer says nothing about the session: never send anyone to sign-in for it.
      if (!active || (error instanceof DOMException && error.name === 'AbortError')) return
      store.failed(errorMessage(error))
      retry = window.setTimeout(check, Math.min(1500 * 2 ** attempts++, 15_000))
    })
    check()
    const unbind = bindRouter()
    return () => {
      active = false
      controller.abort()
      window.clearTimeout(retry)
      unbind()
    }
  }, [accessRevision, bootAttempt])

  const route = resolveRoute(path)
  const current = currentNavigation(route)
  const title = t(current?.label ?? '控制面板')
  useEffect(() => { document.title = `${title} · ${t('apsp')}` }, [title, t])

  if (route.page === 'member-login') return <PageBoundary resetKey={path}><Suspense fallback={<Boot />}><MemberLogin /></Suspense></PageBoundary>
  if (route.page === 'member-join') return <PageBoundary resetKey={path}><Suspense fallback={<Boot />}><MemberJoin key={route.token} token={route.token} /></Suspense></PageBoundary>
  if (route.page === 'proxy-portal') return <PageBoundary resetKey={path}><Suspense fallback={<Boot />}><UserPortal key={route.account} account={route.account} activation={route.activation} /></Suspense></PageBoundary>
  if (session === null) return <Boot message={bootError || undefined} onRetry={bootError ? () => setBootAttempt(value => value + 1) : undefined} />
  if (!session && !(route.page === 'dashboard' && publicDashboard)) {
    return <Login notice={notice} onLogin={() => useSessionStore.getState().signedIn()} />
  }
  // The overview replaces the bare landing path as soon as the actor is known.
  if (session && route.page === 'dashboard' && (window.location.hash === '' || window.location.hash === '#/')) return <Boot />
  // Without a session the board stands alone; signed in, it is one more console page.
  if (route.page === 'dashboard' && !session) {
    return <PageBoundary resetKey={path}><Suspense fallback={<Boot />}>
      <ServerDisplay key="public" serverId={route.serverId} />
    </Suspense></PageBoundary>
  }

  const logout = () => void action.run(() => api('/api/logout', 'POST'), () => useSessionStore.getState().signedOut(''))
  return <AdminShell current={current} exact={route.page === current?.page && !(route.page === 'dashboard' && route.serverId)} busy={action.busy} error={action.error} onLogout={logout}>
    <PageBoundary resetKey={path}>
      {route.page === 'dashboard'
        ? <Suspense fallback={<PageSkeleton />}><ServerDisplay key="admin" embedded serverId={route.serverId} /></Suspense>
        : <AdminPage route={route} />}
    </PageBoundary>
  </AdminShell>
}
