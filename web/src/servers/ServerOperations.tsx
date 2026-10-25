import { ErrorNotice, Loading, PageHeader, TierNotice } from '../components'
import { useResource } from '../hooks'
import type { Server } from '../types'
import FleetOperations from '../fleet/FleetOperations'
import ServerNavigation from './ServerNavigation'
import { buttonVariants } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { ArrowLeftIcon } from 'lucide-react'

// The experimental operations tools of one server live in its detail page,
// like every other per-server task; the tools still need local grants.
export default function ServerOperations({ id }: { id: number }) {
  const server = useResource<Server>(`/api/servers/${id}`)
  const entry = server.data
  return <>
    <a className={cn(buttonVariants({ variant: 'ghost', size: 'sm' }), 'mb-4 -ml-2')} href="#/servers"><ArrowLeftIcon className="size-4" aria-hidden="true" />返回服务器列表</a>
    <ErrorNotice message={server.error} retry={server.reload} />
    {!entry ? server.loading && <Loading /> : <>
      <PageHeader eyebrow={`服务器 #${entry.id}`} title={entry.name} description="这台服务器的资源与能力、服务日志与文件、交互式终端、历史比较，以及采购与生命周期记录。" />
      <ServerNavigation id={id} active="fleet" />
      <TierNotice tier="experimental">终端、文件和服务操作还需要服务器本机的运维策略允许；通用运维更适合交给 SSH 工具。</TierNotice>
      <FleetOperations serverId={id} embedded />
    </>}
  </>
}
