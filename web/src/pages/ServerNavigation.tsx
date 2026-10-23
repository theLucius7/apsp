import { SegmentedLink, SegmentedNav } from '@/components/segmented'

export default function ServerNavigation({ id, active }: { id: number; active: 'overview' | 'ip-info' | 'node-quality' | 'tcp-quality' | 'plugins' | 'fleet' | 'network-workbench' | 'network-configuration' | 'operations' }) {
  const pages = [{ key: 'fleet', path: '/fleet', label: '日常运维' }, { key: 'network-workbench', path: '/network-workbench', label: '网络与验机' }, { key: 'network-configuration', path: '/network-configuration', label: '网络与证书' }, { key: 'operations', path: '/operations', label: '批量运维' }, { key: 'overview', path: '', label: '服务器概况' }, { key: 'plugins', path: '/plugins', label: '服务器插件' }, { key: 'ip-info', path: '/ip-info', label: 'IP 信息' }, { key: 'node-quality', path: '/node-quality', label: 'NodeQuality 验机' }, { key: 'tcp-quality', path: '/tcp-quality', label: 'TCP 诊断' }]
  return <SegmentedNav className="mb-6" aria-label="服务器导航">{pages.map(page => <SegmentedLink key={page.key} href={`#/servers/${id}${page.path}`} aria-current={active === page.key ? 'page' : undefined}>{page.label}</SegmentedLink>)}</SegmentedNav>
}
