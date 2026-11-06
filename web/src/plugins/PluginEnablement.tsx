import { PageHeader } from '../components'
import { PluginSettings } from '.'
import PluginCenterTabs from './PluginCenterTabs'

export default function PluginEnablement() {
  return <>
    <PageHeader title="插件中心" />
    <PluginCenterTabs active="enablement" />
    <PluginSettings key="all" />
  </>
}
