import { SegmentedLink, SegmentedNav } from '@/components/segmented'

// The plugin centre has two views: what each plugin does, and where it is enabled.
export default function PluginCenterTabs({ active }: { active: 'catalog' | 'enablement' }) {
  return <SegmentedNav className="mb-6" aria-label="插件中心">
    <SegmentedLink href="#/plugins/catalog" aria-current={active === 'catalog' ? 'page' : undefined}>插件目录</SegmentedLink>
    <SegmentedLink href="#/system/plugins" aria-current={active === 'enablement' ? 'page' : undefined}>服务器启用</SegmentedLink>
  </SegmentedNav>
}
