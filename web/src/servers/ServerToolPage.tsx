import { ErrorNotice, Icon, Loading, PageHeader } from '../components'
import { useResource } from '../hooks'
import type { Server } from '../types'
import NodeQuality from '../server-diagnostics/NodeQuality'
import TcpQuality from '../server-diagnostics/TcpQuality'
import ServerIpInfo from '../server-diagnostics/ServerIpInfo'
import ServerNavigation from './ServerNavigation'

type ToolSection = 'ip-info' | 'node-quality' | 'tcp-quality'

const descriptions: Record<ToolSection, string> = {
  'ip-info': '这台服务器出口 IP 的查询结果、逐源状态和历史记录。',
  'node-quality': '从这台服务器运行节点验机，查看分段报告和历史结果。',
  'tcp-quality': '从这台服务器向已授权目标发起有限的 TCP 连接诊断，保留参数和报告。',
}

export default function ServerToolPage({ id, section }: { id: number; section: ToolSection }) {
  const server = useResource<Server>(`/api/servers/${id}`)
  const entry = server.data
  return <>
    <a className="back-link" href="#/servers"><Icon name="back" size={16} />返回服务器列表</a>
    <ErrorNotice message={server.error} retry={server.reload} />
    {!entry ? server.loading && <Loading /> : <>
      <PageHeader eyebrow={`服务器 #${entry.id}`} title={entry.name} description={descriptions[section]} />
      <ServerNavigation id={id} active={section} />
      {section === 'ip-info' ? <ServerIpInfo serverId={id} /> : section === 'tcp-quality' ? <TcpQuality serverId={id} /> : <NodeQuality serverId={id} />}
    </>}
  </>
}
