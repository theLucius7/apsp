import { PageHeader } from '../components'
import { PluginSettings } from '.'
import PluginCenterTabs from './PluginCenterTabs'

export default function PluginEnablement() {
  return <>
    <PageHeader eyebrow="扩展" title="插件中心" description="按服务器查看和管理插件的启用状态。单台服务器的插件也可在服务器详情的“插件”中管理。" />
    <PluginCenterTabs active="enablement" />
    <PluginSettings key="all" />
  </>
}
