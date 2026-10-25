import { lazy, Suspense, useEffect, useRef, useState } from 'react'
import { api, errorMessage } from './api'
import { Brand, Loading } from './components'
import { useAction } from './hooks'
import AdminPage from './app/AdminPage'
import AdminShell from './app/AdminShell'
import Login from './app/Login'
import { currentNavigation, homePath } from './app/navigation'
import { resolveRoute } from './app/routes'
import { useI18n } from './i18n'
import { useSessionStore } from './stores/session'
import { acceptHashChange, guardLinks } from '@/lib/leave-guard'

const ServerDisplay = lazy(() => import('./display/ServerDisplay'))
const MemberLogin = lazy(() => import('./plugins/singbox/commerce/MemberLogin'))
const MemberJoin = lazy(() => import('./plugins/singbox/commerce/Join'))
const UserPortal = lazy(() => import('./plugins/singbox/UserPortal'))

function Boot() {
  return <div className="flex min-h-dvh flex-col items-center justify-center gap-2"><Brand /><Loading /></div>
}

export default function App() {
  const { t } = useI18n()
  const session = useSessionStore(state => state.authenticated)
  const publicDashboard = useSessionStore(state => state.publicDashboard)
  const notice = useSessionStore(state => state.notice)
  const accessRevision = useSessionStore(state => state.revision)
  const [path, setPath] = useState(window.location.hash.slice(1) || '/dashboard')
  const shown = useRef(path)
  shown.current = path
  const action = useAction()

  useEffect(() => guardLinks(), [])

  useEffect(() => {
    const store = useSessionStore.getState()
    const controller = new AbortController()
    let active = true
    void api<{ authenticated: boolean; public_dashboard: boolean }>(
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
      if (active) store.failed(errorMessage(error))
    })
    const unauthorized = () => {
      useSessionStore.getState().signedOut(useSessionStore.getState().authenticated ? t('登录已过期，请重新登录。') : undefined)
    }
    // Back, forward and direct hash writes reach the router here; unsaved input decides.
    const hash = () => { if (acceptHashChange(`#${shown.current}`)) setPath(window.location.hash.slice(1) || '/dashboard') }
    window.addEventListener('apsp:unauthorized', unauthorized)
    window.addEventListener('hashchange', hash)
    return () => {
      active = false
      controller.abort()
      window.removeEventListener('apsp:unauthorized', unauthorized)
      window.removeEventListener('hashchange', hash)
    }
  }, [accessRevision, t])

  const route = resolveRoute(path)
  const current = currentNavigation(route)
  const title = t(current?.label ?? '控制面板')
  useEffect(() => { document.title = `${title} · ${t('apsp')}` }, [title, t])

  if (route.page === 'member-login') return <Suspense fallback={<Boot />}><MemberLogin /></Suspense>
  if (route.page === 'member-join') return <Suspense fallback={<Boot />}><MemberJoin key={route.token} token={route.token} /></Suspense>
  if (route.page === 'proxy-portal') return <Suspense fallback={<Boot />}><UserPortal key={route.account} account={route.account} activation={route.activation} /></Suspense>
  if (session === null) return <Boot />
  if (!session && !(route.page === 'dashboard' && publicDashboard)) {
    return <Login notice={notice} onLogin={() => useSessionStore.getState().signedIn()} />
  }
  // The overview replaces the bare landing path as soon as the actor is known.
  if (session && route.page === 'dashboard' && (window.location.hash === '' || window.location.hash === '#/')) return <Boot />
  if (route.page === 'dashboard') {
    return <Suspense fallback={<Boot />}>
      <ServerDisplay key={session ? 'admin' : 'public'} serverId={route.serverId} />
    </Suspense>
  }

  const logout = () => void action.run(() => api('/api/logout', 'POST'), () => useSessionStore.getState().signedOut(''))
  return <AdminShell current={current} busy={action.busy} error={action.error} onLogout={logout}>
    <AdminPage route={route} />
  </AdminShell>
}
