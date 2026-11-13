import { useEffect, useState } from 'react'
import { api } from '../../api'
import { ErrorNotice, Field, Modal } from '../../components'
import { useAction } from '../../hooks'
import { ddnsWrite, matchZone, normalizeDomain, providers } from './types'
import type { DdnsConfig, DdnsRule } from './types'
import { editorError } from './guards'
import type { DdnsCurrent, DdnsServer } from './guards'
import type { DnsAccount } from './DnsAccounts'
import { Notice } from '@/components/panel'
import { Button } from '@/components/ui/button'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { ChevronRightIcon } from 'lucide-react'

type Provider = NonNullable<DdnsConfig['provider']>
type CredentialMode = 'account' | 'center' | 'legacy'
const empty: DdnsConfig = { provider: 'cloudflare', line: '', name: '', server_id: 0, zone_id: '', record_name: '', record_type: 'A', ttl: 1, proxied: false, interval_secs: 300, enabled: true, adopt_existing: false }
const defaultLine = (provider: Provider) => provider === 'tencent' ? '0' : provider === 'aliyun' ? 'default' : ''

function Toggle({ label, description, checked, onChange }: { label: string; description: string; checked: boolean; onChange: (value: boolean) => void }) {
  return <label className="flex items-start gap-3 py-2 text-sm"><Checkbox checked={checked} onCheckedChange={value => onChange(value === true)} className="mt-0.5" /><span><span className="font-medium">{label}</span><small className="text-muted-foreground mt-0.5 block text-xs leading-relaxed">{description}</small></span></label>
}

/**
 * Adds or edits one rule. With a saved DNS account the domain is all that is
 * needed: the account and zone are found from it. Everything else has a
 * sensible default and sits under 更多设置.
 */
export default function RuleEditor({ rule, servers, accounts, current, close, saved, connect }: { rule?: DdnsRule; servers: DdnsServer[]; accounts: DnsAccount[]; current: DdnsCurrent; close: () => void; saved: () => void; connect: () => void }) {
  const usable = accounts.filter(account => account.config.enabled)
  const [config, setConfig] = useState<DdnsConfig>(rule?.config ?? { ...empty, server_id: servers[0]?.id ?? 0 })
  const [addressMode, setAddressMode] = useState<'A' | 'AAAA' | 'dual'>(rule?.config.record_type ?? 'A')
  const [credentialMode, setCredentialMode] = useState<CredentialMode>(rule ? rule.config.account_id ? 'account' : rule.config.credential_id ? 'center' : 'legacy' : usable.length ? 'account' : 'legacy')
  const [token, setToken] = useState(''), [key, setKey] = useState(''), [secret, setSecret] = useState('')
  const [ttlTouched, setTtlTouched] = useState(Boolean(rule))
  const [more, setMore] = useState(false)
  const action = useAction()
  const change = (part: Partial<DdnsConfig>) => setConfig(value => ({ ...value, ...part }))
  // A new rule on a saved account takes its provider and zone from the domain.
  const match = !rule && credentialMode === 'account' ? matchZone(usable, config.record_name) : null
  const ruleAccount = rule?.config.account_id ? accounts.find(account => account.id === rule.config.account_id) : undefined
  const provider: Provider = match?.account.config.provider ?? config.provider ?? 'cloudflare'
  const cloudflare = provider === 'cloudflare', namedZone = provider === 'tencent' || provider === 'aliyun'
  useEffect(() => {
    if (ttlTouched) return
    setConfig(value => ({ ...value, ttl: provider === 'cloudflare' ? 1 : 600 }))
  }, [provider, ttlTouched])
  const slots = !rule && addressMode === 'dual' ? 2 : 1
  const draft = (): DdnsConfig => {
    const name = config.name.trim() || normalizeDomain(config.record_name)
    if (!match) return { ...config, name, ...(cloudflare ? {} : { proxied: false }) }
    return { ...config, name, provider, account_id: match.account.id, credential_id: null, zone_id: match.zoneId, line: config.line || defaultLine(provider), ...(cloudflare ? {} : { proxied: false }) }
  }
  const credentialError = () => {
    if (credentialMode === 'account') return rule ? config.account_id ? '' : '请选择 DNS 账号。' : !config.record_name.trim() ? '请填写域名。' : match ? '' : '已连接的 DNS 账号里没有这个域名。'
    if (credentialMode === 'center') return config.credential_id?.trim() ? '' : '请填写凭据中心的 DNS 凭据标识。'
    if (cloudflare) return !rule && !token.trim() ? '请填写 Cloudflare API Token。' : ''
    return Boolean(key.trim()) !== Boolean(secret.trim()) || (!rule && (!key.trim() || !secret.trim())) ? '请同时填写访问密钥 ID 与 Secret。' : ''
  }
  const writeError = () => (addressMode === 'dual' && config.address_source === 'manual' ? '双栈手工地址请分别创建 IPv4 与 IPv6 规则。' : '') || editorError(current, draft(), rule, slots) || credentialError()
  const submit = () => void action.run(() => {
    const reason = writeError()
    if (reason) throw new Error(reason)
    const path = rule ? `/api/naming/rules/${rule.id}` : addressMode === 'dual' ? '/api/naming/rules/dual-stack' : '/api/naming/rules'
    return api(path, rule ? 'PATCH' : 'POST', ddnsWrite(draft(), token, rule?.revision, key, secret))
  }, () => { setToken(''); setKey(''); setSecret(''); saved() })
  const domainHint = match ? `由「${match.account.config.name}」管理${match.account.config.name.includes(providers[provider]) ? '' : ` · ${providers[provider]}`}` : config.record_name.trim() && usable.length ? '已连接的 DNS 账号里没有这个域名。请检查拼写，或先添加管理它的账号。' : '例如 hk1.example.com，也支持 *.example.com。'
  const fixedDomain = rule && <div className="mb-4 grid gap-1"><span className="text-sm font-medium">域名</span><code className="bg-muted/50 rounded-md border px-3 py-2 text-sm wrap-anywhere">{rule.config.record_name} · {rule.config.record_type}</code><small className="text-muted-foreground text-xs">{ruleAccount ? `由「${ruleAccount.config.name}」管理` : providers[provider]} · 域名、类型和账号保存后不能修改，需要时请另建一条。</small></div>
  return <Modal title={rule ? `编辑解析 · ${rule.config.record_name}` : '添加解析'} busy={action.busy} onClose={close} wide>
    <form className="flex min-h-0 flex-col" onSubmit={event => { event.preventDefault(); submit() }}>
      <div data-slot="modal-body" className="p-4"><ErrorNotice message={action.error || editorError(current, draft(), rule, slots)} /><fieldset disabled={action.busy}>
        {!rule && !usable.length && <Notice className="items-center"><span>还没有连接 DNS 账号。连接后，添加解析只需要填写域名。</span><Button type="button" size="sm" variant="outline" onClick={connect}>连接 DNS 账号</Button></Notice>}
        {rule ? fixedDomain : credentialMode === 'account'
          ? <Field label="完整域名" hint={domainHint}><Input autoFocus value={config.record_name} required maxLength={253} autoCapitalize="none" spellCheck={false} onChange={event => change({ record_name: event.target.value })} placeholder="hk1.example.com" /></Field>
          : <div className="grid items-start gap-x-4 sm:grid-cols-2">
            <Field label="DNS 提供方"><NativeSelect className="w-full" value={provider} onChange={event => { const next = event.target.value as Provider; change({ provider: next, zone_id: '', line: '', proxied: false }); setToken(''); setKey(''); setSecret('') }}>{Object.entries(providers).map(([id, name]) => <option key={id} value={id}>{name}</option>)}</NativeSelect></Field>
            <Field label="完整域名" hint="例如 hk1.example.com，也支持 *.example.com。"><Input value={config.record_name} required maxLength={253} autoCapitalize="none" spellCheck={false} onChange={event => change({ record_name: event.target.value })} placeholder="hk1.example.com" /></Field>
            <Field label={namedZone ? '托管根域名' : 'Zone ID'} hint={namedZone ? '在该云服务托管的根域名，例如 example.com。' : `在 ${providers[provider]} 的域名概览页复制。`}><Input value={config.zone_id} required pattern={namedZone ? undefined : '[a-fA-F0-9]{32}'} maxLength={namedZone ? 253 : 32} spellCheck={false} autoCapitalize="none" onChange={event => change({ zone_id: event.target.value.trim() })} placeholder={namedZone ? 'example.com' : '32 位区域标识'} /></Field>
            {namedZone && <Field label="解析线路标识" hint={provider === 'tencent' ? '默认线路为 0。' : '默认线路为 default。'}><Input value={config.line ?? ''} maxLength={64} placeholder={defaultLine(provider)} onChange={event => change({ line: event.target.value })} /></Field>}
          </div>}
        <div className="grid items-start gap-x-4 sm:grid-cols-2">
          <Field label="服务器"><NativeSelect className="w-full" value={config.server_id} required onChange={event => change({ server_id: Number(event.target.value) })}>{!servers.some(server => server.id === config.server_id) && <option value={config.server_id}>{rule?.server_name ?? '请选择服务器'}（不可用）</option>}{servers.map(server => <option key={server.id} value={server.id}>{server.name}{!server.online ? '（离线）' : ''}</option>)}</NativeSelect></Field>
          <Field label="IP 类型" hint={addressMode === 'dual' ? '一次添加 A 和 AAAA 两条，之后分别管理。' : undefined}><NativeSelect className="w-full" value={addressMode} disabled={Boolean(rule)} onChange={event => { const mode = event.target.value as typeof addressMode; setAddressMode(mode); change({ record_type: mode === 'AAAA' ? 'AAAA' : 'A' }) }}><option value="A">仅 IPv4</option><option value="AAAA">仅 IPv6</option><option value="dual">IPv4 和 IPv6</option></NativeSelect></Field>
        </div>
        {credentialMode === 'legacy' && (cloudflare
          ? <Field label="API Token" hint={rule ? '已保存的令牌不显示；留空保留，填写则替换。' : '只授予这个域名 DNS 编辑权限的令牌。'}><Input type="password" value={token} autoComplete="new-password" required={!rule} minLength={16} maxLength={256} onChange={event => setToken(event.target.value)} placeholder={rule?.token_configured ? '已配置，留空保留' : '输入 Cloudflare API Token'} /></Field>
          : <div className="grid items-start gap-x-4 sm:grid-cols-2"><Field label="访问密钥 ID" hint={rule ? '两项同时留空则保留原密钥。' : '建议使用只有 DNS 权限的子账号密钥。'}><Input type="password" autoComplete="new-password" value={key} required={!rule || Boolean(secret)} minLength={8} maxLength={256} onChange={event => setKey(event.target.value)} /></Field><Field label="访问密钥 Secret"><Input type="password" autoComplete="new-password" value={secret} required={!rule || Boolean(key)} minLength={8} maxLength={256} onChange={event => setSecret(event.target.value)} /></Field></div>)}
        <Toggle label="接管已有的同名记录" description="域名已有解析时勾选，之后由这里接管；不勾选则遇到已有记录时停止。" checked={config.adopt_existing} onChange={value => change({ adopt_existing: value })} />

        <button type="button" aria-expanded={more} onClick={() => setMore(value => !value)} className="text-muted-foreground hover:text-foreground focus-visible:ring-ring/50 mt-2 flex items-center gap-1 rounded-sm text-sm outline-none focus-visible:ring-3"><ChevronRightIcon aria-hidden="true" className={`size-4 transition-transform ${more ? 'rotate-90' : ''}`} />更多设置</button>
        {more && <div className="mt-3 border-t pt-4">
          <div className="grid items-start gap-x-4 sm:grid-cols-2">
            <Field label="IP 从哪里来" hint={config.address_source === 'discovered' ? 'NAT 后的服务器选这个，需要在采样设置中开启公网 IP 发现。' : config.address_source === 'manual' ? '不依赖服务器在线，适合固定 IP。' : 'Agent 上报的公网地址，有多个时沿用上次成功的那个。'}><NativeSelect className="w-full" value={config.address_source ?? 'agent'} onChange={event => change({ address_source: event.target.value as NonNullable<DdnsConfig['address_source']>, manual_ip: null, interface_name: null })}><option value="agent">服务器上报的公网地址</option><option value="interface">指定网卡的地址</option><option value="discovered">实际出口公网 IP（NAT）</option><option value="manual">手动填写</option></NativeSelect></Field>
            {config.address_source === 'interface' && <Field label="网卡" hint="只使用这个网卡上的公网地址。"><Input required value={config.interface_name ?? ''} maxLength={128} list="ddns-address-interfaces" onChange={event => change({ interface_name: event.target.value })} /><datalist id="ddns-address-interfaces">{servers.find(server => server.id === config.server_id)?.interface_names?.map(name => <option key={name} value={name} />)}</datalist></Field>}
            {config.address_source === 'manual' && <Field label="公网地址" hint="与 IP 类型一致的公网地址。"><Input value={config.manual_ip ?? ''} required maxLength={64} spellCheck={false} onChange={event => change({ manual_ip: event.target.value })} placeholder={config.record_type === 'A' ? 'IPv4 公网地址' : 'IPv6 公网地址'} /></Field>}
            <Field label="TTL（秒）" hint={cloudflare ? '1 表示自动，或 60–86400。' : '60–86400，不能低于域名套餐的下限。'}><Input type="number" required min={cloudflare ? 1 : 60} max={86400} step={1} value={config.proxied ? 1 : config.ttl} disabled={config.proxied} onChange={event => { setTtlTouched(true); change({ ttl: Number(event.target.value) }) }} /></Field>
            <Field label="检查间隔（秒）" hint="60–86400；IP 没变时不会调用 DNS 接口。"><Input type="number" required min={60} max={86400} step={1} value={config.interval_secs} onChange={event => change({ interval_secs: Number(event.target.value) })} /></Field>
            <Field label="名称" hint="留空时使用域名。"><Input value={config.name} maxLength={128} onChange={event => change({ name: event.target.value })} placeholder={normalizeDomain(config.record_name) || '例如 香港入口 IPv4'} /></Field>
            <Field label="凭据来源"><NativeSelect className="w-full" value={credentialMode} disabled={Boolean(rule?.config.account_id)} onChange={event => { const mode = event.target.value as CredentialMode; setCredentialMode(mode); change({ credential_id: mode === 'center' ? '' : null, account_id: mode === 'account' ? config.account_id ?? null : null }); setToken(''); setKey(''); setSecret('') }}><option value="account" disabled={!usable.length && !rule?.config.account_id}>已连接的 DNS 账号{usable.length ? '' : '（尚未添加）'}</option>{credentialMode === 'center' && <option value="center">凭据中心引用</option>}<option value="legacy" disabled={Boolean(rule?.config.account_id)}>只给这条解析单独填写</option></NativeSelect></Field>
            {credentialMode === 'center' && <Field label="DNS 凭据标识" hint="凭据中心里用途为 DNS 的凭据。"><Input value={config.credential_id ?? ''} required maxLength={36} spellCheck={false} pattern="[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}" onChange={event => change({ credential_id: event.target.value.trim() })} placeholder="DNS 凭据的 UUID" /></Field>}
          </div>
          {cloudflare && <Toggle label="启用 Cloudflare 代理" description="流量经 Cloudflare 转发（橙色云朵），TTL 自动；代理协议节点通常应保持关闭。" checked={config.proxied} onChange={value => change({ proxied: value })} />}
          <Toggle label="自动同步" description="关闭后保留规则和现有解析，不再自动更新。" checked={config.enabled} onChange={value => change({ enabled: value })} />
        </div>}
      </fieldset></div>
      <footer><Button type="button" variant="outline" disabled={action.busy} onClick={close}>取消</Button><Button disabled={action.busy || Boolean(writeError())}>{action.busy ? '正在保存…' : rule ? '保存' : addressMode === 'dual' ? '添加两条解析' : '添加解析'}</Button></footer>
    </form>
  </Modal>
}

