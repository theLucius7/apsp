import { Badge, Empty, ErrorNotice, Loading, PageHeader, Refresh } from '../../components'
import { useResource } from '../../hooks'
import type { PluginServer } from '../../types'
import { installationView } from './Settings'

const steps = [
  { title: '接入服务器', description: '安装 Agent 并确认服务器在线。监控、资产和日常运维都在「服务器」中管理。', href: '#/servers', action: '管理服务器' },
  { title: '启用 sing-box', description: '为需要提供代理的服务器启用插件，等待设备确认安装和运行状态。', href: '#/system/plugins', action: '启用插件' },
  { title: '创建节点与链路', description: '节点是某台服务器上的代理入口；链路把多台服务器或订阅中的节点串成一条线路。', href: '#/plugins/sing-box/nodes', action: '创建代理节点' },
  { title: '创建用户并授权', description: '用策略组决定代理用户能用哪些节点；需要流量和有效期限制时，再分配套餐。', href: '#/plugins/sing-box/users', action: '管理代理用户' },
]

export default function Overview() {
  const servers = useResource<PluginServer[]>('/api/plugins/sing-box/servers')
  return <>
    <PageHeader eyebrow="代理服务" title="服务概览" description="由 sing-box 插件在你的服务器上提供代理节点，并管理代理用户、授权与订阅。首次使用按下面的顺序完成配置。"><Refresh onClick={servers.reload} /><a className="button button-primary" href="#/system/plugins">启用插件</a></PageHeader>
    <section className="panel"><div className="panel-heading"><h2>使用顺序</h2></div><div className="panel-body"><ol className="plugin-workflow">{steps.map(step => <li key={step.title}><div><strong>{step.title}</strong><p>{step.description}</p></div><a className="text-button" href={step.href}>{step.action}</a></li>)}</ol><p className="helper">面板保存配置和授权，Agent 将配置应用到对应服务器并回报结果。创建成功后，还需等待设备应用；订阅只提供已应用的可用节点。</p></div></section>
    <ErrorNotice message={servers.error} retry={servers.reload} />
    <section className="panel"><div className="panel-heading"><h2>各服务器的 sing-box 状态</h2></div>{servers.loading && !servers.data ? <Loading /> : !servers.data?.length ? <Empty icon="server" title="先接入服务器" description="服务器在线后，再为需要代理服务的设备安装 sing-box。"><a className="button button-primary" href="#/servers">添加服务器</a></Empty> : <div className="table-wrap"><table><thead><tr><th>服务器</th><th>安装与运行</th><th>下一步</th></tr></thead><tbody>{servers.data.map(server => {
      const state = installationView(server)
      return <tr key={server.id}><td><a className="text-button" href={`#/servers/${server.id}`}>{server.name}</a><small>{server.online ? 'Agent 在线' : 'Agent 离线'} · {server.agent_supported ? '支持 sing-box' : '尚未声明支持'}</small></td><td><Badge tone={state.tone}>{state.label}</Badge><small>{state.reason}</small></td><td>{server.enabled ? <div className="row-actions"><a className="text-button" href={`#/plugins/sing-box/nodes?server=${server.id}`}>管理此服务器节点</a><a className="text-button" href={`#/servers/${server.id}`}>查看部署</a></div> : <a className="text-button" href="#/system/plugins">启用并安装</a>}</td></tr>
    })}</tbody></table></div>}</section>
    <section className="panel"><div className="panel-heading"><h2>代理业务</h2></div><div className="panel-body plugin-business-links"><a className="button button-secondary" href="#/plugins/sing-box/nodes">代理节点</a><a className="button button-secondary" href="#/plugins/sing-box/nodes?kind=chains">链路</a><a className="button button-secondary" href="#/plugins/sing-box/groups">策略与套餐</a><a className="button button-secondary" href="#/plugins/sing-box/users">代理用户与订阅</a></div></section>
  </>
}
