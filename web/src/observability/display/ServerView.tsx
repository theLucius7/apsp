import { useEffect } from 'react'
import { ArrowLeftIcon, RefreshCwIcon } from 'lucide-react'
import { status } from './data'
import { OSIcon, Region } from './Icon'
import { useDashboardServers } from './useDashboardServers'
import ServerMonitor from './ServerMonitor'
import { snapshotUnavailable } from './dashboard'
import { DisplayNotice, StatusPill, surface } from './ui'
import { cn } from '@/lib/utils'
import { Button, buttonVariants } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'

/**
 * One server on the board. Visitors and monitoring-only administrators see it
 * here; anyone who may read the server itself is sent to its console page,
 * which shows the same monitoring panel beside the server's settings.
 */
export default function ServerView({ id, now, embedded = false }: { id: number; now: number; embedded?: boolean }) {
  const resource = useDashboardServers(false, id)
  const server = resource.data?.[0]
  const privateView = embedded && server !== undefined && !server.public_view
  useEffect(() => { if (privateView) window.location.replace(`#/servers/${id}`) }, [privateView, id])
  if (!server || privateView) return <>
    <a className={cn(buttonVariants(), 'mb-4')} href="#/dashboard"><ArrowLeftIcon aria-hidden="true" />返回看板</a>
    {resource.error ? <DisplayNotice tone="error" onRetry={resource.reload}>{resource.error}</DisplayNotice> : <div className="text-muted-foreground flex min-h-80 items-center justify-center gap-2 text-sm" role="status"><Spinner role="presentation" aria-hidden="true" aria-label={undefined} />正在读取服务器…</div>}
  </>
  const unavailable = snapshotUnavailable(resource.updatedAt, now, false, resource.error)
  const info = server.static_info, state = status(server, unavailable)
  return <div data-slot="server-detail" className="flex flex-col gap-4">
    <section data-slot="detail-hero" className={cn(surface, 'flex min-w-0 items-center gap-3 px-5 py-3.5 max-sm:gap-2 max-sm:px-3')}>
      <a href="#/dashboard" className={buttonVariants({ variant: 'ghost', size: 'icon' })} aria-label="返回服务器看板"><ArrowLeftIcon /></a>
      <div className="min-w-0 flex-1">
        <h1 className="flex flex-wrap items-center gap-2 text-xl font-semibold wrap-anywhere max-sm:text-lg">{server.name}<StatusPill tone={state.tone}>{state.label}</StatusPill></h1>
        <p className="text-muted-foreground mt-1 flex flex-wrap items-center gap-1.5 text-sm"><Region region={server.asset_settings?.region} /><OSIcon system={info.system} />{info.system ?? '系统尚未上报'}<span>·</span>{info.arch ?? '架构未知'}</p>
      </div>
      <Button variant="ghost" size="icon" aria-label="刷新服务器详情" onClick={resource.reload}><RefreshCwIcon /></Button>
    </section>
    {resource.error && <DisplayNotice tone="error">{resource.error} 以下保留最近一次读取的信息。</DisplayNotice>}
    <ServerMonitor server={server} base={`/api/dashboard/servers/${id}`} now={now} unavailable={unavailable} modern={resource.modern} />
  </div>
}
