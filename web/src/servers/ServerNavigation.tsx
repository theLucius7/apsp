export type ServerSectionKey = 'overview' | 'assets' | 'agent' | 'plugins' | 'ip-info' | 'node-quality' | 'tcp-quality'

// Views of this one server, in the order an administrator usually needs them.
const sections: readonly { key: ServerSectionKey; path: string; label: string }[] = [
  { key: 'overview', path: '', label: '概况' },
  { key: 'assets', path: '/assets', label: '资产与流量' },
  { key: 'agent', path: '/agent', label: 'Agent 与任务' },
  { key: 'plugins', path: '/plugins', label: '插件' },
  { key: 'ip-info', path: '/ip-info', label: 'IP 信息' },
  { key: 'node-quality', path: '/node-quality', label: '节点验机' },
  { key: 'tcp-quality', path: '/tcp-quality', label: 'TCP 诊断' },
]

// Cross-server workspaces keep their own sidebar entry; these links only open
// them with this server preselected.
const workspaces: readonly { path: string; label: string }[] = [
  { path: '/fleet', label: '运维工具' },
  { path: '/network-workbench', label: '网络检测' },
  { path: '/network-configuration', label: '网络配置' },
  { path: '/operations', label: '自动化与恢复' },
]

export default function ServerNavigation({ id, active }: { id: number; active: ServerSectionKey }) {
  return <div className="server-navigation">
    <nav className="ui-tab-list" aria-label="服务器导航">
      {sections.map(section => <a key={section.key} href={`#/servers/${id}${section.path}`}
        aria-current={active === section.key ? 'page' : undefined}>{section.label}</a>)}
    </nav>
    <nav className="server-workspaces" aria-label="带此服务器打开">
      <span>带此服务器打开</span>
      {workspaces.map(workspace => <a key={workspace.path} className="text-button" href={`#/servers/${id}${workspace.path}`}>{workspace.label}</a>)}
    </nav>
  </div>
}
