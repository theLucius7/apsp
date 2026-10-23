import { ErrorNotice, Loading, PageHeader } from '../components'
import { useResource } from '../hooks'
import type { Server } from '../types'
import NodeQuality from './NodeQuality'
import TcpQuality from './TcpQuality'
import ServerIpInfo from './ServerIpInfo'
import ServerNavigation from './ServerNavigation'
import { buttonVariants } from '@/components/ui/button'
import { cn } from '@/lib/utils'
import { ArrowLeftIcon } from 'lucide-react'

export default function ServerToolPage({ id, section }: { id: number; section: 'ip-info' | 'node-quality' | 'tcp-quality' }) {
  const server = useResource<Server>(`/api/servers/${id}`)
  const entry = server.data
  return <>
    <a className={cn(buttonVariants({ variant: 'ghost', size: 'sm' }), 'mb-4 -ml-2')} href="#/servers"><ArrowLeftIcon className="size-4" aria-hidden="true" />返回服务器</a>
    <ErrorNotice message={server.error} retry={server.reload} />
    {!entry ? server.loading && <Loading /> : <>
      <PageHeader eyebrow={`服务器 #${entry.id}`} title={entry.name} description={section === 'ip-info' ? '查看服务器 IP 查询结果、逐源状态与历史数据。' : section === 'tcp-quality' ? '对已配置目标做有限 TCP 连接诊断，保留参数和部分报告。' : '运行节点诊断并查看已保存的验机报告。'} />
      <ServerNavigation id={id} active={section} />
      {section === 'ip-info' ? <ServerIpInfo serverId={id} /> : section === 'tcp-quality' ? <TcpQuality serverId={id} /> : <NodeQuality serverId={id} />}
    </>}
  </>
}
