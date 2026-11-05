import { PageHeader } from '../components'
import { PluginSettings } from '.'
import PluginCenterTabs from './PluginCenterTabs'

export default function PluginEnablement() {
  return <>
    <PageHeader title="插件中心" description="查看每台服务器的 sing-box 安装与运行状态。单台服务器也可在服务器详情的“插件”中管理。" />
    <PluginCenterTabs active="enablement" />
    <PluginSettings key="all" />
  </>
}
