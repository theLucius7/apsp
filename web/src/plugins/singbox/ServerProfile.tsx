import { useEffect, useState } from 'react'
import { api } from '../../api'
import { ErrorNotice, Field } from '../../components'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

type Profile = { revision: number; definition: Record<string, unknown> }
const examples: Record<string, Record<string, unknown>> = {
  'DNS 与分流': { dns: { servers: [{ type: 'https', tag: 'extra-dns', server: 'dns.example.com', path: '/dns-query' }], final: 'extra-dns' }, outbounds: [{ type: 'socks', tag: 'extra-exit', server: 'proxy.example.com', server_port: 1080, version: '5', username: '', password: '' }], route: { rules: [{ domain_suffix: ['example.org'], action: 'route', outbound: 'extra-exit' }], final: 'direct' } },
  'Cloudflared 隧道': { cloudflared: [{ tag: 'extra-tunnel', token: '', origin_node_id: 1, protocol: 'auto' }] },
  'Hysteria Realm 服务': { services: [{ type: 'hysteria-realm', tag: 'extra-realm', listen: '::', listen_port: 8443, tls: { enabled: true, server_name: 'realm.example.com', certificate: '', key: '' }, users: [{ name: 'realm-client', token: '', max_realms: 10 }] }] },
  'Naive 出口': { outbounds: [{ type: 'naive', tag: 'extra-naive', server: 'proxy.example.com', server_port: 443, username: '', password: '', tls: { enabled: true, server_name: 'proxy.example.com' } }], route: { final: 'extra-naive' } },
  'OpenVPN 端点': { endpoints: [{ type: 'openvpn-client', tag: 'extra-vpn', server: 'vpn.example.com', server_port: 1194, username: '', password: '' }], route: { final: 'extra-vpn' } },
  'OpenConnect 端点': { endpoints: [{ type: 'openconnect', tag: 'extra-vpn', server: 'https://vpn.example.com', username: '', password: '' }], route: { final: 'extra-vpn' } },
}
export default function ServerProfile({ serverId }: { serverId: number }) {
  const path = `/api/plugins/sing-box/servers/${serverId}/profile`
  const resource = useResource<Profile>(path)
  const action = useAction()
  const [text, setText] = useState('')
  const [dirty, setDirty] = useState(false)
  useEffect(() => { if (resource.data && !dirty) setText(JSON.stringify(resource.data.definition, null, 2)) }, [resource.data, dirty])
  const save = () => {
    if (resourceWriteError(resource) || !resource.data) return
    void action.run(async () => { await api(path, 'PUT', { expected_revision: resource.data!.revision, definition: JSON.parse(text) }); setDirty(false) }, resource.reload)
  }
  return <Panel><PanelHeader><PanelTitle>服务端配置</PanelTitle><Button variant="outline" size="sm" onClick={() => { setDirty(false); resource.reload() }}>重新载入</Button></PanelHeader><div className="grid gap-3 p-4">
    <ErrorNotice message={resource.error || action.error} />
    <p className="text-muted-foreground text-sm">配置出口、端点、DNS、分流、隧道和 Realm 服务。保存后进入部署预检；失败时保留已应用配置。凭据不回显，保留占位符会沿用原值。</p>
    <details><summary className="cursor-pointer text-sm">填入配置示例</summary><div className="mt-2 flex flex-wrap gap-2">{Object.entries(examples).map(([name, example]) => <Button key={name} size="sm" variant="outline" onClick={() => { setText(JSON.stringify(example, null, 2)); setDirty(true) }}>{name}</Button>)}</div><p className="text-muted-foreground mt-2 text-xs">示例会替换编辑区，请填写自己的服务地址和凭据后保存。Cloudflared 源站编号须对应此服务器上的回环 VLESS 隧道节点。</p></details>
    <Field label="配置 JSON"><Textarea aria-label="服务端配置 JSON" rows={18} spellCheck={false} autoComplete="off" value={text} onChange={event => { setText(event.target.value); setDirty(true) }} className="font-mono text-xs" /></Field>
    <div><Button disabled={action.busy || !dirty || Boolean(resourceWriteError(resource))} onClick={save}>{action.busy ? '保存中…' : '保存并发布'}</Button></div>
  </div></Panel>
}
