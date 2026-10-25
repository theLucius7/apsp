import { useEffect, useState } from 'react'
import type { ComponentType, ReactNode } from 'react'
import { ErrorNotice, Loading } from '../components'
import type { AppRoute } from './routes'
import { homePath } from './navigation'
import { buttonVariants } from '@/components/ui/button'

/**
 * Loads a page on its first visit. Unlike React.lazy with Suspense, completion
 * is a plain state update, so revealing the page never waits on timers; a
 * page loaded once renders synchronously afterwards.
 */
function lazyPage<P extends object>(load: () => Promise<{ default: ComponentType<P> }>) {
  let loaded: ComponentType<P> | undefined
  let pending: Promise<ComponentType<P>> | undefined
  return function LazyPage(props: P) {
    const [page, setPage] = useState<ComponentType<P> | undefined>(() => loaded)
    const [failed, setFailed] = useState(false)
    useEffect(() => {
      if (page) return
      let active = true
      pending ??= load().then(module => (loaded = module.default))
      pending.then(component => { if (active) setPage(() => component) }, () => {
        pending = undefined
        if (active) setFailed(true)
      })
      return () => { active = false }
    }, [page])
    const Page = page
    if (Page) return <Page {...props} />
    return failed ? <ErrorNotice message="页面加载失败，请检查网络后刷新重试。" /> : <Loading />
  }
}

// Each administrator page loads on first visit, keeping the initial bundle small.
const Servers = lazyPage(() => import('../servers/Servers'))
const ServerDetail = lazyPage(() => import('../servers/ServerDetail'))
const ServerToolPage = lazyPage(() => import('../servers/ServerToolPage'))
const ServerOperations = lazyPage(() => import('../servers/ServerOperations'))
const BatchEnrollment = lazyPage(() => import('../servers/BatchEnrollmentPage'))
const PluginEnablement = lazyPage(() => import('../plugins/PluginEnablement'))
const PluginCatalog = lazyPage(() => import('../plugins/PluginCatalog'))
const Security = lazyPage(() => import('../system/Security'))
const Settings = lazyPage(() => import('../system/Settings'))
const Backup = lazyPage(() => import('../system/Backup'))
const AlertSettings = lazyPage(() => import('../monitoring/AlertSettings'))
const Notifications = lazyPage(() => import('../monitoring/Notifications'))
const LatencyTasks = lazyPage(() => import('../monitoring/LatencyTasks'))
const Statistics = lazyPage(() => import('../statistics/Statistics'))
const SingboxOverview = lazyPage(() => import('../plugins/singbox/Overview'))
const Nodes = lazyPage(() => import('../plugins/singbox/Nodes'))
const ProxyUsers = lazyPage(() => import('../plugins/singbox/ProxyUsers'))
const Groups = lazyPage(() => import('../plugins/singbox/Groups'))
const Commerce = lazyPage(() => import('../plugins/singbox/commerce/Commerce'))
const Expenses = lazyPage(() => import('../plugins/expenses/Expenses'))
const Ddns = lazyPage(() => import('../plugins/ddns/Ddns'))
const Alicloud = lazyPage(() => import('../plugins/alicloud/Alicloud'))
const FleetOperations = lazyPage(() => import('../fleet/FleetOperations'))
const NetworkWorkbench = lazyPage(() => import('../network-workbench/NetworkWorkbench'))
const NetworkConfiguration = lazyPage(() => import('../network-configuration/NetworkConfiguration'))
const OperationsPage = lazyPage(() => import('../operations/OperationsPage'))
const ControlCenter = lazyPage(() => import('../control-center/ControlCenter'))

export default function AdminPage({ route }: { route: Exclude<AppRoute, { page: 'dashboard' | 'proxy-portal' | 'member-join' | 'member-login' }> }) {
  return page(route)
}

function page(route: Exclude<AppRoute, { page: 'dashboard' | 'proxy-portal' | 'member-join' | 'member-login' }>): ReactNode {
  switch (route.page) {
    case 'server': {
      const { serverId, section } = route
      if (section === 'fleet') return <ServerOperations key={serverId} id={serverId} />
      if (section === 'network-workbench') return <NetworkWorkbench key={serverId} initialServerId={serverId} />
      if (section === 'network-configuration') return <NetworkConfiguration key={serverId} serverId={serverId} />
      if (section === 'operations') return <OperationsPage key={serverId} selectedServerId={serverId} />
      if (section === 'ddns') return <Ddns key={serverId} serverId={serverId} />
      if (section === 'ip-info' || section === 'node-quality' || section === 'tcp-quality') return <ServerToolPage key={`${serverId}/${section}`} id={serverId} section={section} />
      return <ServerDetail key={serverId} id={serverId} section={section} />
    }
    case 'fleet': return <FleetOperations />
    case 'batch-enrollment': return <BatchEnrollment />
    case 'network-workbench': return <NetworkWorkbench />
    case 'network-configuration': return <NetworkConfiguration />
    case 'operations': return <OperationsPage />
    case 'control-center': return <ControlCenter />
    case 'servers': return <Servers />
    case 'singbox-overview': return <SingboxOverview />
    case 'statistics': return <Statistics />
    case 'latency': return <LatencyTasks />
    case 'alicloud': return <Alicloud />
    case 'ddns': return <Ddns />
    case 'nodes': return <Nodes
      key="nodes"
      serverId={route.serverId}
      chainsOnly={route.chains}
      view={route.view}
      selected={route.selected}
      initialKind={route.kind}
      initialServerRole={route.serverRole}
    />
    case 'expenses': return <Expenses />
    case 'commerce': return <Commerce />
    case 'proxy-users': return <ProxyUsers />
    case 'groups': return <Groups />
    case 'plugins': return <PluginEnablement />
    case 'catalog': return <PluginCatalog />
    case 'settings': return <Settings />
    case 'backup': return <Backup />
    case 'alert-settings': return <AlertSettings />
    case 'notifications': return <Notifications />
    case 'security': return <Security />
    case 'not-found': return <div className="grid justify-items-center gap-4 px-5 py-16 text-center">
      <h1 className="text-xl font-semibold tracking-tight">这个页面不存在</h1>
      <p className="text-muted-foreground text-sm">链接可能已失效。从左侧导航选择一个页面，或回到概览。</p>
      <a className={buttonVariants()} href={`#${homePath}`}>返回概览</a>
    </div>
  }
}
