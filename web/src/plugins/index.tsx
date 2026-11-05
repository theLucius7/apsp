import type { Server } from '../types'
import ServerBusiness from './singbox/ServerBusiness'
import SingboxSettings from './singbox/Settings'
import { buttonVariants } from '@/components/ui/button'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

function DdnsEntry({ href }: { href: string }) {
  return <Panel><PanelHeader><PanelTitle>域名解析</PanelTitle><a className={buttonVariants({ variant: 'outline', size: 'sm' })} href={href}>管理域名解析</a></PanelHeader>
    <p className="text-muted-foreground px-4 py-3 text-xs leading-relaxed">让域名自动指向服务器的公网 IP。由面板执行，所有接入 Agent 的服务器都可以直接使用，无需单独启用。</p></Panel>
}

/** The plugins tab of one server: sing-box in full, then the optional plugins. */
export function ServerPluginsSection({ serverId, server }: { serverId: number; server?: Server }) {
  return <><ServerBusiness serverId={serverId} runtimeVersion={server?.static_info.runtime_version} /><DdnsEntry href={`#/servers/${serverId}/ddns`} /></>
}

/** Every server's plugin enablement, for the plugin centre. */
export function PluginSettings() {
  return <><SingboxSettings /><DdnsEntry href="#/plugins/ddns" /></>
}
