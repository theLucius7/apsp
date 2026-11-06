import { SegmentedLink, SegmentedNav } from '@/components/segmented'

// The plugin centre has two views: what each plugin does, and where sing-box is installed.
export default function PluginCenterTabs({ active }: { active: 'catalog' | 'enablement' }) {
  return <SegmentedNav className="mb-6" aria-label="插件中心">
    <SegmentedLink href="#/settings/plugins" aria-current={active === 'catalog' ? 'page' : undefined}>插件目录</SegmentedLink>
    <SegmentedLink href="#/settings/plugins/servers" aria-current={active === 'enablement' ? 'page' : undefined}>sing-box 安装</SegmentedLink>
  </SegmentedNav>
}
