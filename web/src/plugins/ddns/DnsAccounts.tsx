import { useState } from 'react'
import { api } from '../../api'
import { Badge, ErrorNotice, Field, FormDialog, Modal, DialogPresence } from '../../components'
import { useAction, useResource } from '../../hooks'
import type { DdnsServer } from './guards'
import { providers } from './types'
import DnsRecords from './DnsRecords'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

export type DnsAccount = { id: string; revision: number; config: { name: string; provider: keyof typeof providers; credential_id: string; zone_ids: string[]; zone_names?: Record<string, string>; server_ids: number[]; enabled: boolean }; record_management_available: boolean; unavailable_reason: string | null; checked_at: number | null; error_code: string | null }

function Editor({ account, servers, close, saved }: { account?: DnsAccount; servers: DdnsServer[]; close: () => void; saved: () => void }) {
  const action = useAction()
  const [config, setConfig] = useState(account?.config ?? { name: '', provider: 'cloudflare' as keyof typeof providers, credential_id: '', zone_ids: [] as string[], server_ids: [] as number[], enabled: true })
  const [zones, setZones] = useState(config.zone_ids.join('\n'))
  const change = (part: Partial<typeof config>) => setConfig(value => ({ ...value, ...part }))
  return <Modal title={account ? '编辑 DNS 账号' : '添加 DNS 账号'} busy={action.busy} onClose={close}><form onSubmit={event => { event.preventDefault(); void action.run(() => api(account ? `/api/plugins/ddns/accounts/${account.id}` : '/api/plugins/ddns/accounts', account ? 'PUT' : 'POST', { config: { ...config, zone_ids: zones.split(/[\s,]+/).filter(Boolean) }, revision: account?.revision }), saved) }}><div data-slot="modal-body" className="p-4"><ErrorNotice message={action.error} />
    <Field label="账号名称"><Input required value={config.name} maxLength={128} onChange={event => change({ name: event.target.value })} /></Field>
    <Field label="DNS 提供方"><NativeSelect className="w-full" value={config.provider} onChange={event => change({ provider: event.target.value as typeof config.provider })}>{Object.entries(providers).map(([id, name]) => <option key={id} value={id}>{name}</option>)}</NativeSelect></Field>
    <Field label="凭据中心 DNS 凭据标识" hint="仅保存加密凭据引用。四个提供方分别使用官方接口；区域、线路、结构参数和异步确认按实际提供方能力显示。"><Input required value={config.credential_id} maxLength={36} onChange={event => change({ credential_id: event.target.value.trim() })} placeholder="DNS 凭据 UUID" /></Field>
    <Field label="明确授权的 DNS 区域" hint="每行一个区域。Cloudflare / 华为云填写 Zone ID，腾讯云 / 阿里云填写托管根域名，最多 32 个。"><Textarea required rows={4} value={zones} onChange={event => setZones(event.target.value)} /></Field>
    <Field label="账号授权关联的服务器" hint="所有关联服务器均须在管理员授权范围内；未选择时需要全服务器授权。">{servers.map(server => <label className="my-5 flex items-start gap-3 text-sm" key={server.id}><Checkbox checked={config.server_ids.includes(server.id)} onCheckedChange={checked => change({ server_ids: checked === true ? [...config.server_ids, server.id] : config.server_ids.filter(id => id !== server.id) })} className="mt-0.5" /><span>{server.name}</span></label>)}</Field>
    <label className="my-5 flex items-start gap-3 text-sm"><Checkbox checked={config.enabled} onCheckedChange={checked => change({ enabled: checked === true })} className="mt-0.5" /><span>启用此账号；停用保留记录与操作历史。</span></label>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">保存账号范围和凭据引用需要管理员再次验证；连接检查只查询提供方，不写入 DNS。</p>
  </div><footer><Button type="button" variant="outline" disabled={action.busy} onClick={close}>取消</Button><Button disabled={action.busy}>保存账号</Button></footer></form></Modal>
}

/** One form for the common case: everything else is worked out from the token. */
function CloudflareConnect({ close, saved }: { close: () => void; saved: () => void }) {
  const action = useAction()
  const [name, setName] = useState('')
  const [token, setToken] = useState('')
  return <FormDialog title="添加 Cloudflare 账号" submitLabel="验证并保存" busy={action.busy} error={action.error} onClose={close} onSubmit={() => void action.run(() => api('/api/plugins/ddns/accounts/cloudflare', 'POST', { name: name.trim(), api_token: token.trim() }), saved)}>
    <Field label="API 令牌" hint="只需要这一个值。在 Cloudflare 的「账户 API 令牌」创建，权限选 DNS 编辑；保存时会校验并自动找出它能管理的域名，之后不再显示。"><Input type="password" autoComplete="off" required value={token} onChange={event => setToken(event.target.value)} placeholder="cfat_…" /></Field>
    <Field label="名称" hint="留空时按第一个域名命名。"><Input value={name} maxLength={128} onChange={event => setName(event.target.value)} /></Field>
  </FormDialog>
}

export default function DnsAccounts({ servers }: { servers: DdnsServer[] }) {
  const accounts = useResource<DnsAccount[]>('/api/plugins/ddns/accounts')
  const action = useAction()
  const [editing, setEditing] = useState<DnsAccount | 'new' | null>(null)
  const [connecting, setConnecting] = useState(false)
  const [records, setRecords] = useState<DnsAccount | null>(null)
  const [check, setCheck] = useState<{ zones: { id: string; name: string | null; available: boolean; error_code: string | null }[]; error_code: string | null; checked_at: number } | null>(null)
  return <Panel><PanelHeader><PanelTitle>DNS 账号</PanelTitle><div className="flex items-center gap-1"><Button variant="ghost" size="sm" onClick={() => setEditing('new')}>其他提供方</Button><Button size="sm" onClick={() => setConnecting(true)}>添加 Cloudflare 账号</Button></div></PanelHeader><div className="p-4"><ErrorNotice message={accounts.error || action.error} retry={accounts.reload} />
    <p className="text-muted-foreground text-xs leading-relaxed">保存一次，动态解析规则和用户专属地址都从这里选。令牌加密保存，不再显示。</p>
    {accounts.data?.map(account => <article data-slot="ddns-rule" className="border-t py-5 first:border-t-0" key={account.id}><div className="flex flex-wrap items-center justify-between gap-4"><strong>{account.config.name} · {providers[account.config.provider]}</strong><Badge>{account.config.enabled ? '已启用' : '已停用'}</Badge></div><p className="text-muted-foreground mt-3 text-xs leading-relaxed">账号标识 {account.id} · {account.config.zone_ids.length} 个授权区域 · {account.config.server_ids.length ? `${account.config.server_ids.length} 台服务器范围` : '全服务器权限范围'} · {account.record_management_available ? '官方记录管理已接入' : account.unavailable_reason}</p><div className="flex flex-wrap gap-4"><Button variant="ghost" size="sm" disabled={action.busy || !account.config.enabled || !account.record_management_available} onClick={() => void action.run(async () => { setCheck(await api(`/api/plugins/ddns/accounts/${account.id}/check`, 'POST')); accounts.reload() })}>检查连接与区域</Button><Button variant="ghost" size="sm" disabled={!account.config.enabled || !account.record_management_available} onClick={() => setRecords(account)}>记录、传播观测与历史</Button><Button variant="ghost" size="sm" onClick={() => setEditing(account)}>编辑授权与启停</Button></div></article>)}
    {!accounts.data?.length && !accounts.loading && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">尚未登记 DNS 账号。</p>}
    {check && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{new Date(check.checked_at * 1000).toLocaleString('zh-CN')} · {check.zones.length ? check.zones.map(zone => `${zone.name ?? zone.id}：${zone.available ? '官方接口已确认' : `未确认（${zone.error_code ?? '未知'}）`}`).join('；') : '连接或授权未确认，请检查凭据与区域范围。'}</p>}
  </div><DialogPresence>{connecting && <CloudflareConnect close={() => setConnecting(false)} saved={() => { setConnecting(false); accounts.reload() }} />}</DialogPresence><DialogPresence>{editing && <Editor account={editing === 'new' ? undefined : editing} servers={servers} close={() => setEditing(null)} saved={() => { setEditing(null); accounts.reload() }} />}</DialogPresence><DialogPresence>{records && <DnsRecords account={records} close={() => setRecords(null)} />}</DialogPresence></Panel>
}
