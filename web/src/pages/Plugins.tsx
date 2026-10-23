import { PageHeader } from '../components'
import { PluginSettings } from '../plugins'
import ServerNavigation from './ServerNavigation'
import { buttonVariants } from '@/components/ui/button'

export default function Plugins({ serverId }: { serverId?: number }) {
  return <><PageHeader eyebrow={serverId ? `服务器 #${serverId}` : '服务器扩展'} title="服务器插件" description="按服务器管理插件的启用状态与配置，执行位置以各插件说明为准。"><a className={buttonVariants({ variant: 'outline' })} href="#/plugins/catalog">浏览插件目录</a></PageHeader>{serverId && <ServerNavigation id={serverId} active="plugins" />}<PluginSettings key={serverId ?? 'all'} serverId={serverId} /></>
}
