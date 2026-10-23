import type { Server } from '../types'
import ServerBusiness from './singbox/ServerBusiness'
import SingboxSettings from './singbox/Settings'
import { buttonVariants } from '@/components/ui/button'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

export function ServerPlugins({ server }: { server: Server }) { return <ServerBusiness server={server} /> }
export function PluginSettings({ serverId }: { serverId?: number }) { return <><SingboxSettings serverId={serverId} /><Panel><PanelHeader><PanelTitle>DDNS 动态域名解析</PanelTitle><a className={buttonVariants({ variant: 'outline' })} href={serverId ? `#/servers/${serverId}/ddns` : '#/plugins/ddns'}>管理 DDNS 插件</a></PanelHeader><div className="p-4"><p className="text-muted-foreground mt-3 text-xs leading-relaxed">按服务器启用 Cloudflare 动态解析，复用 Agent 的 IP 上报，由面板插件同步 DNS。</p></div></Panel></> }
