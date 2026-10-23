import { SegmentedLink, SegmentedNav } from '@/components/segmented'
import { buttonVariants } from '@/components/ui/button'

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
  return <div data-slot="server-navigation" className="mb-6 grid min-w-0 gap-2.5">
    <SegmentedNav aria-label="服务器导航">
      {sections.map(section => <SegmentedLink key={section.key} href={`#/servers/${id}${section.path}`}
        aria-current={active === section.key ? 'page' : undefined}>{section.label}</SegmentedLink>)}
    </SegmentedNav>
    <nav data-slot="server-workspaces" className="text-muted-foreground flex min-w-0 flex-wrap items-center gap-1 text-xs" aria-label="带此服务器打开">
      <span className="mr-1">带此服务器打开</span>
      {workspaces.map(workspace => <a key={workspace.path} className={buttonVariants({ variant: 'ghost', size: 'xs' })} href={`#/servers/${id}${workspace.path}`}>{workspace.label}</a>)}
    </nav>
  </div>
}
