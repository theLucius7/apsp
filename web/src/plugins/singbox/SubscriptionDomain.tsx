import { useEffect, useState } from 'react'
import { api } from '../../api'
import { ErrorNotice, Field } from '../../components'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import type { DnsAccount } from '../ddns/DnsAccounts'
import { Checkbox } from '@/components/ui/checkbox'
import { Input } from '@/components/ui/input'
import { NativeSelect } from '@/components/ui/native-select'
import { Notice } from '@/components/panel'
import { Button, buttonVariants } from '@/components/ui/button'

type DomainView = {
  configured: boolean; enabled: boolean; credential_id: string | null; zone_id: string; zone_name: string; host_prefix: string; target: string; legacy_paths: boolean; revision: number
  hosts: { total: number; ready: number; failed: number; last_error: string | null }
}
const path = '/api/plugins/sing-box/subscription-domain'

/** Panel-wide switch for each user having their own hostname for subscriptions and sign-in; a tab of 门户设置. */
export default function SubscriptionDomain({ onSaved }: { onSaved: () => void }) {
  const view = useResource<DomainView>(path, 5000)
  const accounts = useResource<DnsAccount[]>('/api/plugins/ddns/accounts', 0)
  const action = useAction()
  const [saved, setSaved] = useState(false)
  const usable = accounts.data?.filter(account => account.config.provider === 'cloudflare' && account.config.enabled) ?? []
  const [loaded, setLoaded] = useState(false)
  const [enabled, setEnabled] = useState(true)
  const [accountId, setAccountId] = useState('')
  const [zoneId, setZoneId] = useState('')
  const [prefix, setPrefix] = useState('apsp-')
  const [target, setTarget] = useState('')
  const [legacy, setLegacy] = useState(true)
  const account = usable.find(entry => entry.id === accountId)
  // Fill the form once, from what is saved or from the only sensible choice.
  useEffect(() => {
    if (loaded || !view.data || !accounts.data) return
    const saved = view.data, current = usable.find(entry => entry.config.credential_id === saved.credential_id) ?? usable[0]
    setLoaded(true)
    setAccountId(current?.id ?? '')
    setZoneId(saved.zone_id || (current?.config.zone_ids[0] ?? ''))
    if (saved.configured) { setEnabled(saved.enabled); setPrefix(saved.host_prefix); setTarget(saved.target); setLegacy(saved.legacy_paths) }
  }, [loaded, view.data, accounts.data])
  const zoneName = account?.config.zone_names?.[zoneId] ?? view.data?.zone_name ?? ''
  const error = resourceWriteError(view) || (loaded && !usable.length ? '还没有可用的 Cloudflare 账号，请先在「域名解析」添加。' : '')
  const submit = () => {
    const current = view.getCurrent()
    if (!current || !account) return
    void action.run(() => api(path, 'PUT', { enabled, credential_id: account.config.credential_id, zone_id: zoneId, host_prefix: prefix.trim(), target: target.trim(), legacy_paths: legacy, revision: current.revision }), () => { setSaved(true); view.reload(); onSaved() })
  }
  const hosts = view.data?.hosts
  return <form data-slot="subscription-domain" onSubmit={event => { event.preventDefault(); submit() }} onChange={() => setSaved(false)}>
    <p className="text-muted-foreground mb-3 text-[13px] leading-relaxed">每个用户的订阅和登录使用各自的随机地址；重置后旧地址立即失效。</p>
    <code className="bg-muted/50 mb-4 block rounded-md border px-2.5 py-2 text-xs wrap-anywhere">https://{prefix || 'apsp-'}随机.{zoneName || '你的域名'}/随机</code>
    <label className="mb-4 flex items-center gap-2.5 text-[13px]"><Checkbox checked={enabled} onCheckedChange={checked => setEnabled(checked === true)} />启用专属地址</label>
    {loaded && !usable.length && <a className={buttonVariants({ variant: 'outline', size: 'sm' }) + ' mb-4'} href="#/dns">前往添加 Cloudflare 账号</a>}
    {usable.length > 0 && <>
      <Field label="Cloudflare 账号"><NativeSelect className="w-full" value={accountId} onChange={event => { setAccountId(event.target.value); setZoneId(usable.find(entry => entry.id === event.target.value)?.config.zone_ids[0] ?? '') }}>{usable.map(entry => <option key={entry.id} value={entry.id}>{entry.config.name}</option>)}</NativeSelect></Field>
      <Field label="域名"><NativeSelect className="w-full" value={zoneId} onChange={event => setZoneId(event.target.value)}>{account?.config.zone_ids.map(id => <option key={id} value={id}>{account.config.zone_names?.[id] ?? id}</option>)}</NativeSelect></Field>
    </>}
    <div className="grid gap-x-3 sm:grid-cols-2">
      <Field label="地址前缀"><Input required maxLength={20} value={prefix} onChange={event => setPrefix(event.target.value)} spellCheck={false} /></Field>
      <Field label="解析目标" hint="专属地址都解析到这里，通常是 Cloudflare Tunnel 的地址。"><Input required maxLength={253} value={target} onChange={event => setTarget(event.target.value)} spellCheck={false} placeholder="隧道 ID.cfargotunnel.com" /></Field>
    </div>
    <label className="flex items-start gap-2.5 text-[13px]"><Checkbox className="mt-0.5" checked={legacy} onCheckedChange={checked => setLegacy(checked === true)} /><span>旧地址继续可用<small className="text-muted-foreground block text-xs leading-relaxed">已经在用 /sub/… 和面板域名登录的客户端不受影响。都换成专属地址后，关掉这项，旧地址就不再响应。</small></span></label>
    {view.data?.enabled && hosts && <Notice className="mt-4 mb-0" tone={hosts.failed ? 'error' : 'quiet'} role="status">已创建 {hosts.ready} / {hosts.total} 个专属地址{hosts.failed ? `，${hosts.failed} 个失败：${hosts.last_error ?? '原因未知'}` : ''}</Notice>}
    <div className="mt-4"><ErrorNotice message={error || action.error} /></div>
    <div className="flex flex-wrap items-center gap-3">
      <Button disabled={action.busy || Boolean(error) || !account || !zoneId}>{action.busy ? '正在保存…' : '保存专属地址'}</Button>
      {saved && <span role="status" className="text-muted-foreground text-xs">已保存</span>}
    </div>
  </form>
}
