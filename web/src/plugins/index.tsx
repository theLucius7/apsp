import type { Server } from '../types'
import ServerBusiness from './singbox/ServerBusiness'
import SingboxSettings from './singbox/Settings'
import { buttonVariants } from '@/components/ui/button'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

export function ServerPlugins({ server }: { server: Server }) { return <ServerBusiness server={server} /> }
export function PluginSettings({ serverId }: { serverId?: number }) { return <><SingboxSettings serverId={serverId} /><Panel><PanelHeader><PanelTitle>域名解析</PanelTitle><a className={buttonVariants({ variant: 'outline' })} href={serverId ? `#/servers/${serverId}/ddns` : '#/plugins/ddns'}>管理域名解析</a></PanelHeader><div className="p-4"><p className="text-muted-foreground text-xs leading-relaxed">让域名自动指向服务器的公网 IP。由面板执行，所有接入 Agent 的服务器都可以直接使用，无需单独启用。</p></div></Panel></> }
