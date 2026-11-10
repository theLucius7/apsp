import type { ReactNode } from 'react'
import type { AppRoute } from './routes'
import { homePath, pageEntries } from './navigation'
import { ServerDetail } from './pages'
import { ProxyUsers } from '../access/pages'
import { Expenses } from '../costs/pages'
import { Ddns } from '../naming/pages'
import { NetworkConfiguration } from '../networking/pages'
import { NetworkWorkbench } from '../observability/pages'
import { OperationsPage } from '../operations/pages'
import { Nodes } from '../traffic/pages'
import { buttonVariants } from '@/components/ui/button'

/**
 * Starts fetching the code of the page a route opens. The first screen calls it while
 * the session is still being checked, so the page is ready when the console appears.
 */
export function preloadRoute(route: AppRoute) {
  if (route.page === 'server') return ServerDetail.preload()
  const entry = pageEntries.find(item => item.page === route.page)
  if (entry && 'preload' in entry) entry.preload()
}

export default function AdminPage({ route }: { route: Exclude<AppRoute, { page: 'dashboard' | 'proxy-portal' | 'member-join' | 'member-login' }> }) {
  return page(route)
}

function page(route: Exclude<AppRoute, { page: 'dashboard' | 'proxy-portal' | 'member-join' | 'member-login' }>): ReactNode {
  switch (route.page) {
    case 'server': {
      const { serverId, section } = route
      if (section === 'network-workbench') return <NetworkWorkbench key={serverId} initialServerId={serverId} />
      if (section === 'network-configuration') return <NetworkConfiguration key={serverId} serverId={serverId} />
      if (section === 'operations') return <OperationsPage key={serverId} selectedServerId={serverId} />
      if (section === 'ddns') return <Ddns key={serverId} serverId={serverId} />
      return <ServerDetail key={serverId} id={serverId} section={section} />
    }
    case 'nodes': return <Nodes
      key="nodes"
      serverId={route.serverId}
      chainsOnly={route.chains}
      view={route.view}
      selected={route.selected}
      initialKind={route.kind}
      initialServerRole={route.serverRole}
    />
    case 'expenses': return <Expenses item={route.item} server={route.server} />
    case 'proxy-users': return <ProxyUsers key={route.userId ?? 'users'} initialUserId={route.userId} />
    default: {
      const entry = pageEntries.find(item => item.page === route.page)
      return entry && 'render' in entry ? entry.render() : notFound
    }
  }
}

const notFound = <div className="grid justify-items-center gap-4 px-5 py-16 text-center">
  <h1 className="text-base font-semibold">这个页面不存在</h1>
  <p className="text-muted-foreground text-sm">链接可能已失效。从左侧导航选择一个页面，或回到概览。</p>
  <a className={buttonVariants()} href={`#${homePath}`}>返回概览</a>
</div>
