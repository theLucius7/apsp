import { useRef, useState } from 'react'
import { Badge, Empty, ErrorNotice, Loading, Modal } from '../../components'
import { time } from '../../format'
import type { ResourceState } from '../../hooks'
import { validatedSnapshot } from './groupTypes'
import { filterNexusNodes, nexusEndpoint, nexusProtocol, validNexusNodes } from './nexusNodeTypes'
import type { NexusNode } from './nexusNodeTypes'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'

const protocolLabels: Record<string, string> = { vless: 'VLESS', hysteria: 'Hysteria2', shadowsocks: 'Shadowsocks' }
const fields: Record<string, string> = { network: '传输方式', flow: '流控', tls: 'TLS 模式', reality_sni: 'Reality 目标域名', reality_port: 'Reality 目标端口', public_key: 'Reality 公钥', short_id: 'Short ID', tls_sni: '证书域名', tls_settings_sni: 'TLS 域名', cipher: '加密方法', version: '协议版本', hop_interval: '跳端口间隔（秒）', multiplex: '多路复用', multiplex_protocol: '多路复用协议', encryption_enabled: '原配置加密开关', fingerprint: '客户端指纹', obfs: '混淆', allow_insecure: '允许不安全证书' }

export default function NexusNodes({ query, server, onReload }: { query: ResourceState<unknown>; server: string; onReload: () => void }) {
  const history = useRef<NexusNode[] | undefined>(undefined)
  const view = validatedSnapshot(query, validNexusNodes, history.current)
  if (view.fresh) history.current = view.data
  const [search, setSearch] = useState(''), [protocol, setProtocol] = useState(''), [host, setHost] = useState('')
  const [page, setPage] = useState(1), [selected, setSelected] = useState<number | null>(null)
  const all = view.data ?? [], filtered = filterNexusNodes(all, search, protocol, server || host)
  const pages = Math.max(1, Math.ceil(filtered.length / 20)), current = Math.min(page, pages)
  const node = all.find(node => node.id === selected)
  const hosts = [...new Map(all.map(node => [node.server_id, node.server_name])).entries()]
  if (view.fresh && !all.length) return null
  return <Panel aria-label="Nexus 节点" className="mb-5">
    <PanelHeader><PanelTitle>Nexus 节点 <span className="bg-muted text-muted-foreground rounded px-1.5 text-xs tabular-nums">{view.data ? all.length : '—'}</span></PanelTitle><Badge tone="warm">待 Agent 接管</Badge></PanelHeader>
    <div className="border-b px-5 py-3 text-sm text-muted-foreground">已导入原节点信息和主机关系。当前服务由 Nexus 运行；原订阅链接继续使用。</div>
    <div className="flex flex-wrap gap-2 border-b px-5 py-3">
      <Input className="min-w-40 flex-1" aria-label="搜索 Nexus 节点" placeholder="搜索名称、地址、主机、标签或原编号" value={search} onChange={e => { setSearch(e.target.value); setPage(1) }} />
      <NativeSelect className="w-auto" aria-label="Nexus 节点协议" value={protocol} onChange={e => { setProtocol(e.target.value); setPage(1) }}><option value="">全部协议</option>{[...new Set(all.map(n => n.protocol))].map(p => <option key={p} value={p}>{protocolLabels[p] ?? p}</option>)}</NativeSelect>
      {!server && <NativeSelect className="w-auto max-w-52" aria-label="Nexus 节点服务器" value={host} onChange={e => { setHost(e.target.value); setPage(1) }}><option value="">全部服务器</option>{hosts.map(([id, name]) => <option key={id} value={id}>{name}</option>)}</NativeSelect>}
    </div>
    <div className="px-5 pt-3"><ErrorNotice message={view.error} retry={onReload} /></div>
    {!view.data ? <Loading variant="table" /> : !filtered.length ? <Empty title="没有匹配的 Nexus 节点" description="调整搜索条件或服务器筛选。" /> : <>
      <Table><TableHeader><TableRow><TableHead>节点</TableHead><TableHead>服务器 / 原实例</TableHead><TableHead>公开入口</TableHead><TableHead>流量倍率</TableHead><TableHead>原配置状态</TableHead><TableHead><span className="sr-only">操作</span></TableHead></TableRow></TableHeader>
        <TableBody>{filtered.slice((current - 1) * 20, current * 20).map(node => <TableRow key={node.id}>
          <TableCell className="max-w-64 whitespace-normal wrap-anywhere"><strong>{node.name}</strong><small className="text-muted-foreground block">{nexusProtocol(node)} · 原编号 {node.legacy_node_id}</small><div className="mt-1 flex flex-wrap gap-1">{node.tags.map((tag, index) => <Badge key={index}>{tag}</Badge>)}</div></TableCell>
          <TableCell className="max-w-48 whitespace-normal wrap-anywhere"><a className="text-primary hover:underline" href={`#/servers/${node.server_id}`}>{node.server_name}</a><small className="text-muted-foreground block">{node.machine_name} · #{node.legacy_machine_id}</small></TableCell>
          <TableCell className="max-w-64 whitespace-normal wrap-anywhere"><code>{nexusEndpoint(node)}</code><small className="text-muted-foreground block">监听端口 {node.listen_port}</small></TableCell>
          <TableCell>{node.rate}×{node.timed_rate_enabled && <small className="block">分时倍率</small>}</TableCell>
          <TableCell><Badge tone={node.enabled ? 'neutral' : 'warm'}>{node.enabled ? '已启用' : '已停用'}</Badge><small className="text-muted-foreground mt-1 block">{node.visible ? '订阅可见' : '订阅隐藏'}</small></TableCell>
          <TableCell><Button variant="ghost" size="sm" onClick={() => setSelected(node.id)}>详情</Button></TableCell>
        </TableRow>)}</TableBody>
      </Table>
      <div className="flex flex-wrap items-center justify-between gap-2 border-t px-5 py-3 text-sm"><span className="text-muted-foreground">共 {filtered.length} 个 · 第 {current} / {pages} 页</span><div className="flex gap-2"><Button variant="outline" size="sm" disabled={current <= 1} onClick={() => setPage(current - 1)}>上一页</Button><Button variant="outline" size="sm" disabled={current >= pages} onClick={() => setPage(current + 1)}>下一页</Button></div></div>
    </>}
    {node && <Modal title={node.name} onClose={() => setSelected(null)} wide>
      <Badge tone="warm">节点信息已迁入 · 待 Agent 接管</Badge>
      <dl className="my-4 grid gap-4 sm:grid-cols-2 [&_dt]:text-xs [&_dt]:text-muted-foreground [&_dd]:mt-1 [&_dd]:wrap-anywhere">
        {Object.entries({ '原节点编号': node.legacy_node_id, '来源': node.source_id, '协议': nexusProtocol(node), '主机': `${node.server_name}（原主机 #${node.legacy_host_id}）`, '原实例': `${node.machine_name}（#${node.legacy_machine_id}）`, '原实例设置': node.machine_active ? '启用' : '停用', '公开入口': nexusEndpoint(node), '监听端口': node.listen_port, '流量倍率': `${node.rate}×`, '分时倍率': node.timed_rate_enabled ? `${node.timed_rate_count} 条` : '关闭', '权限组': node.groups.map(g => `${g.name}（#${g.id}）`).join('、') || '无', '原父节点编号': node.parent_id ?? '无', '路由组': node.routes.map(r => `${r.name}（#${r.id}）`).join('、') || '无', '自定义路由': `${node.custom_route_count} 条`, '自定义出口': `${node.custom_outbound_count} 个`, '证书配置': node.certificate_saved ? '已保存' : '未设置', '快照时间': time(node.exported_at), '导入时间': time(node.imported_at) }).map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}
      </dl>
      <h3 className="border-t pt-4 font-medium">协议参数</h3>
      <dl className="my-3 grid gap-3 sm:grid-cols-2 [&_dt]:text-xs [&_dt]:text-muted-foreground [&_dd]:mt-1 [&_dd]:wrap-anywhere">{Object.entries(node.protocol_summary).filter(([key]) => key in fields).map(([key, value]) => <div key={key}><dt>{fields[key]}</dt><dd>{typeof value === 'boolean' ? value ? '启用' : '关闭' : String(value)}</dd></div>)}</dl>
      <p className="text-sm text-muted-foreground">原始密钥、证书及完整路由配置已加密保存。此处展示导入时的信息，实时状态在 Agent 接管后更新。</p>
    </Modal>}
  </Panel>
}
