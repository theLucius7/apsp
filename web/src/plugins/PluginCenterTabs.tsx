// The plugin centre has two views: what each plugin does, and where it is enabled.
export default function PluginCenterTabs({ active }: { active: 'catalog' | 'enablement' }) {
  return <nav className="ui-tab-list section-tabs" aria-label="插件中心">
    <a href="#/plugins/catalog" aria-current={active === 'catalog' ? 'page' : undefined}>插件目录</a>
    <a href="#/system/plugins" aria-current={active === 'enablement' ? 'page' : undefined}>服务器启用</a>
  </nav>
}
