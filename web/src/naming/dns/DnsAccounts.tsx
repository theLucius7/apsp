import { useState } from 'react'
import { api } from '../../api'
import { Badge, ErrorNotice, Field, FormDialog, DialogPresence, InfoTip } from '../../components'
import { ActionMenu } from '../../components/action-menu'
import { useAction } from '../../hooks'
import type { ResourceState } from '../../hooks'
import type { DdnsServer } from './guards'
import { providers, zoneName } from './types'
import DnsRecords from './DnsRecords'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Textarea } from '@/components/ui/textarea'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { PlusIcon } from 'lucide-react'

export type DnsAccount = { id: string; revision: number; config: { name: string; provider: keyof typeof providers; credential_id: string; zone_ids: string[]; zone_names?: Record<string, string>; server_ids: number[]; enabled: boolean }; record_management_available: boolean; unavailable_reason: string | null; checked_at: number | null; error_code: string | null }
type Check = { zones: { id: string; name: string | null; available: boolean; error_code: string | null }[]; error_code: string | null; checked_at: number }

const zoneLabel = (account: DnsAccount, zone: string) => zoneName(account.config.provider, zone, account.config.zone_names) ?? zone

/** Edits what an account may manage; adding one goes through ConnectAccount. */
function Editor({ account, servers, close, saved }: { account: DnsAccount; servers: DdnsServer[]; close: () => void; saved: () => void }) {
  const action = useAction()
  const [config, setConfig] = useState(account.config)
  const [zones, setZones] = useState(config.zone_ids.join('\n'))
  const change = (part: Partial<typeof config>) => setConfig(value => ({ ...value, ...part }))
  return <FormDialog title={`编辑账号 · ${account.config.name}`} busy={action.busy} error={action.error} onClose={close} onSubmit={() => void action.run(() => api(`/api/plugins/ddns/accounts/${account.id}`, 'PUT', { config: { ...config, zone_ids: zones.split(/[\s,]+/).filter(Boolean) }, revision: account.revision }), saved)}>
    <Field label="名称"><Input required value={config.name} maxLength={128} onChange={event => change({ name: event.target.value })} /></Field>
    <Field label="可管理的域名" hint={config.provider === 'cloudflare' || config.provider === 'huawei' ? '每行一个 Zone ID，最多 32 个。' : '每行一个托管的根域名，最多 32 个。'}><Textarea required rows={4} value={zones} onChange={event => setZones(event.target.value)} /></Field>
    <Field label="可用于哪些服务器" hint="都不勾选表示所有服务器。">
      <div className="grid gap-x-4 sm:grid-cols-2">{servers.map(server => <label className="flex items-center gap-2 py-1 text-sm" key={server.id}><Checkbox checked={config.server_ids.includes(server.id)} onCheckedChange={checked => change({ server_ids: checked === true ? [...config.server_ids, server.id] : config.server_ids.filter(id => id !== server.id) })} /><span className="truncate">{server.name}</span></label>)}</div>
    </Field>
    <Field label="凭据引用" hint="凭据中心里用途为 DNS 的凭据；一般不需要修改。"><Input required value={config.credential_id} maxLength={36} spellCheck={false} onChange={event => change({ credential_id: event.target.value.trim() })} /></Field>
    <label className="flex items-center gap-2 text-sm"><Checkbox checked={config.enabled} onCheckedChange={checked => change({ enabled: checked === true })} />启用此账号<small className="text-muted-foreground text-xs">（停用后保留记录与历史）</small></label>
  </FormDialog>
}

/** One form for every provider: a Cloudflare token finds its own domains; key pairs list theirs. */
function ConnectAccount({ close, saved }: { close: () => void; saved: (account: DnsAccount) => void }) {
  const action = useAction()
  const [provider, setProvider] = useState<keyof typeof providers>('cloudflare')
  const [name, setName] = useState(''), [token, setToken] = useState('')
  const [key, setKey] = useState(''), [secret, setSecret] = useState(''), [zones, setZones] = useState('')
  const cloudflare = provider === 'cloudflare', zoneIds = provider === 'huawei'
  return <FormDialog title="添加 DNS 账号" submitLabel={cloudflare ? '验证并保存' : '保存'} busy={action.busy} error={action.error} onClose={close} onSubmit={() => void action.run(() => cloudflare
    ? api<DnsAccount>('/api/plugins/ddns/accounts/cloudflare', 'POST', { name: name.trim(), api_token: token.trim() })
    : api<DnsAccount>('/api/plugins/ddns/accounts/access-key', 'POST', { provider, name: name.trim(), access_key_id: key.trim(), access_key_secret: secret.trim(), zone_ids: zones.split(/[\s,]+/).filter(Boolean) }), saved)}>
    <Field label="DNS 提供方"><NativeSelect className="w-full" value={provider} onChange={event => setProvider(event.target.value as keyof typeof providers)}>{Object.entries(providers).map(([id, label]) => <option key={id} value={id}>{label}</option>)}</NativeSelect></Field>
    {cloudflare
      ? <Field label="API 令牌" hint="在 Cloudflare「账户 API 令牌」创建，权限选 DNS 编辑；保存时自动识别可管理的域名，之后不再显示。"><Input type="password" autoComplete="off" required value={token} onChange={event => setToken(event.target.value)} placeholder="cfat_…" /></Field>
      : <>
        <div className="grid items-start gap-x-4 sm:grid-cols-2">
          <Field label="访问密钥 ID"><Input type="password" autoComplete="off" required minLength={8} maxLength={256} value={key} onChange={event => setKey(event.target.value)} /></Field>
          <Field label="访问密钥 Secret"><Input type="password" autoComplete="off" required minLength={8} maxLength={256} value={secret} onChange={event => setSecret(event.target.value)} /></Field>
        </div>
        <Field label={zoneIds ? '可管理的 Zone ID' : '可管理的域名'} hint={zoneIds ? '每行一个公网 Zone ID，在华为云 DNS 控制台复制。' : '每行一个在该云服务托管的根域名，例如 example.com。'}><Textarea required rows={3} value={zones} onChange={event => setZones(event.target.value)} placeholder={zoneIds ? '32 位区域标识' : 'example.com'} /></Field>
        <p className="text-muted-foreground mb-4 text-xs leading-relaxed">建议使用只有 DNS 权限的子账号密钥。保存后会加密保存，不再显示。</p>
      </>}
    <Field label="名称" hint="留空时按提供方和第一个域名命名。"><Input value={name} maxLength={128} onChange={event => setName(event.target.value)} /></Field>
  </FormDialog>
}

export function useAccountConnect(reload: () => void) {
  const [connecting, setConnecting] = useState(false)
  const dialog = <DialogPresence>{connecting && <ConnectAccount close={() => setConnecting(false)} saved={() => { setConnecting(false); reload() }} />}</DialogPresence>
  return { open: () => setConnecting(true), dialog }
}

/** The saved DNS accounts that rules and personal addresses pick from. */
export default function DnsAccounts({ accounts, servers, connect }: { accounts: ResourceState<DnsAccount[]>; servers: DdnsServer[]; connect: () => void }) {
  const action = useAction()
  const [editing, setEditing] = useState<DnsAccount | null>(null)
  const [records, setRecords] = useState<DnsAccount | null>(null)
  const [check, setCheck] = useState<{ account: string; result: Check } | null>(null)
  const list = accounts.data ?? []
  const runCheck = (account: DnsAccount) => void action.run(async () => { setCheck({ account: account.id, result: await api<Check>(`/api/plugins/ddns/accounts/${account.id}/check`, 'POST') }); accounts.reload() })
  return <Panel>
    <PanelHeader><span className="flex min-w-0 items-center gap-1.5"><PanelTitle>DNS 账号</PanelTitle><InfoTip label="账号说明"><p>令牌加密保存；解析规则和用户专属地址都从这里选。</p></InfoTip></span><Button size="sm" variant="outline" onClick={connect}><PlusIcon aria-hidden="true" />添加账号</Button></PanelHeader>
    {(accounts.error || action.error) && <div className="px-4 pt-4"><ErrorNotice message={accounts.error || action.error} retry={accounts.reload} /></div>}
    {!list.length ? !accounts.loading && <p className="text-muted-foreground p-4 text-sm">还没有 DNS 账号。添加一个 Cloudflare、腾讯云、阿里云或华为云账号后，添加解析只需要填写域名。</p>
      : <ul className="divide-y">{list.map(account => {
        const named = account.config.name.includes(providers[account.config.provider])
        const shown = account.config.zone_ids.slice(0, 6)
        const result = check?.account === account.id ? check.result : null
        return <li key={account.id} data-slot="dns-account" className="flex flex-wrap items-center gap-x-4 gap-y-2 px-4 py-3">
          <div className="min-w-0 flex-[1_1_16rem]">
            <div className="flex flex-wrap items-center gap-x-2 gap-y-1"><strong className="text-sm font-medium wrap-anywhere">{account.config.name}</strong>{!named && <span className="text-muted-foreground text-xs">{providers[account.config.provider]}</span>}{!account.config.enabled ? <Badge>已停用</Badge> : account.error_code && <Badge tone="warm">连接未确认</Badge>}</div>
            <div className="mt-1.5 flex flex-wrap gap-1.5">{shown.map(zone => <span key={zone} className="bg-muted text-muted-foreground rounded-md px-1.5 py-0.5 font-mono text-xs wrap-anywhere">{zoneLabel(account, zone)}</span>)}{account.config.zone_ids.length > shown.length && <span className="text-muted-foreground text-xs">等 {account.config.zone_ids.length} 个</span>}</div>
          </div>
          <div className="ml-auto flex items-center gap-1">
            <Button variant="ghost" size="sm" disabled={!account.config.enabled || !account.record_management_available} onClick={() => setRecords(account)}>管理记录</Button>
            <ActionMenu label="账号操作" groups={[[{ label: '检查连接', disabled: action.busy || !account.config.enabled || !account.record_management_available, onSelect: () => runCheck(account) }, { label: '编辑', onSelect: () => setEditing(account) }]]} />
          </div>
          {result && <Notice tone={result.error_code ? 'info' : 'success'} className="mb-0 basis-full" role="status"><span>{result.zones.length ? result.zones.map(zone => `${zone.name ?? zoneLabel(account, zone.id)}：${zone.available ? '可以管理' : `未确认（${zone.error_code ?? '未知'}）`}`).join('；') : '没有连上提供方，请检查凭据和域名范围。'}</span></Notice>}
        </li>
      })}</ul>}
    <DialogPresence>{editing && <Editor account={editing} servers={servers} close={() => setEditing(null)} saved={() => { setEditing(null); accounts.reload() }} />}</DialogPresence>
    <DialogPresence>{records && <DnsRecords account={records} close={() => setRecords(null)} />}</DialogPresence>
  </Panel>
}
