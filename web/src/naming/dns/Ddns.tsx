import { useState } from 'react'
import { api } from '../../api'
import { Badge, Confirm, Empty, ErrorNotice, Loading, PageHeader, Refresh, DialogPresence } from '../../components'
import { ActionMenu } from '../../components/action-menu'
import { ago } from '../../format'
import { ddnsWrite, providers, ruleState } from './types'
import type { DdnsRule } from './types'
import { resourceWriteError, useAction, useResource } from '../../hooks'
import { ddnsCurrentError, ruleError } from './guards'
import type { DdnsCurrent, DdnsServer as Server } from './guards'
import Lifecycle from './Lifecycle'
import Migration from './Migration'
import RuleEditor from './RuleEditor'
import DnsAccounts, { useAccountConnect } from './DnsAccounts'
import type { DnsAccount } from './DnsAccounts'

import { Button } from '@/components/ui/button'
import { GlobeIcon, PlusIcon, RefreshCwIcon } from 'lucide-react'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

const MAX_RULES = 32

/** Where the record points now, and what the panel would write next. */
function Address({ rule, known }: { rule: DdnsRule; known: boolean }) {
  const next = known ? rule.candidate_ip : null
  const shown = next ?? rule.last_ip
  const checked = rule.last_success_at === null ? null : new Date(rule.last_success_at * 1000).toLocaleString('zh-CN', { hour12: false })
  return <div className="min-w-0">
    <div className="font-mono text-xs wrap-anywhere">{shown ?? '—'}</div>
    <small className="text-muted-foreground block text-xs" title={checked ?? undefined}>{next && rule.last_ip && next !== rule.last_ip ? `DNS 现为 ${rule.last_ip}` : rule.last_success_at === null ? '尚未同步' : rule.last_success_at > 0 ? `${ago(rule.last_success_at)}核对` : `${checked} 核对`}</small>
  </div>
}

export default function Ddns({ serverId }: { serverId?: number }) {
  const [migrating, setMigrating] = useState(false)
  const [lifecycle, setLifecycle] = useState<DdnsRule | null>(null)
  const [editing, setEditing] = useState<DdnsRule | 'new' | null>(null), [removing, setRemoving] = useState<DdnsRule | null>(null)
  const rules = useResource<DdnsRule[]>('/api/plugins/ddns/rules'), servers = useResource<Server[]>('/api/plugins/ddns/servers')
  const accounts = useResource<DnsAccount[]>('/api/plugins/ddns/accounts', 30_000)
  const connect = useAccountConnect(accounts.reload)
  const action = useAction()
  const available = Boolean(rules.data && servers.data && !resourceWriteError(rules, servers))
  const scope = servers.data?.filter(server => !serverId || server.id === serverId) ?? []
  const visibleRules = rules.data?.filter(rule => !serverId || rule.config.server_id === serverId)
  const full = (rules.data?.length ?? MAX_RULES) >= MAX_RULES
  const refresh = () => { rules.reload(); servers.reload(); accounts.reload() }
  const current: DdnsCurrent = () => {
    if (resourceWriteError(rules, servers)) return undefined
    return { rules: rules.getCurrent()!, servers: servers.getCurrent()!.filter(server => !serverId || server.id === serverId) }
  }
  const perform = (error: () => string, request: () => Promise<unknown>, saved = rules.reload) => void action.run(() => { const reason = error(); if (reason) throw new Error(reason); return request() }, saved)
  const open = (rule: DdnsRule | 'new') => { if (rule === 'new' ? ddnsCurrentError(current) : ruleError(current, rule)) return; action.clearError(); setEditing(rule) }
  const toggle = (rule: DdnsRule) => perform(() => ruleError(current, rule), () => api(`/api/plugins/ddns/rules/${rule.id}`, 'PATCH', ddnsWrite({ ...rule.config, enabled: !rule.config.enabled }, '', rule.revision)))
  const sync = (rule: DdnsRule) => perform(() => ruleError(current, rule, true), () => api(`/api/plugins/ddns/rules/${rule.id}/sync`, 'POST'))
  const mixed = new Set(visibleRules?.map(rule => rule.config.provider ?? 'cloudflare')).size > 1
  const noAccounts = accounts.data !== undefined && !accounts.data.some(account => account.config.enabled)
  const addDisabled = action.busy || !available || !scope.length || full

  return <div>
    <PageHeader title="地址与域名"><Refresh onClick={refresh} /><Button disabled={addDisabled} onClick={() => open('new')}><PlusIcon aria-hidden="true" />添加解析</Button></PageHeader>
    <ErrorNotice message={rules.error || servers.error || action.error} retry={refresh} />
    <Panel>
      <PanelHeader>
        <PanelTitle>解析记录<span className="text-muted-foreground text-xs font-normal tabular-nums">{rules.error || !rules.data ? '—' : rules.data.length} / {MAX_RULES}</span></PanelTitle>
        {Boolean(visibleRules?.length) && <Button variant="ghost" size="sm" disabled={action.busy || !available || !scope.length} onClick={() => setMigrating(true)}>批量修改</Button>}
      </PanelHeader>
      {!rules.data && rules.loading ? <Loading variant="table" rows={3} />
        : !rules.data || (rules.error && !visibleRules?.length) ? <Empty icon={GlobeIcon} title="解析状态暂不可用" description="请刷新读取规则和服务器状态后再操作。" />
        : !visibleRules?.length ? !scope.length
          ? <Empty icon={GlobeIcon} title={serverId ? '这台服务器不可用' : '还没有服务器'} description={servers.error || !servers.data ? '服务器状态暂不可用。' : serverId ? '指定服务器不存在或不可用。' : '先添加服务器并接入 Agent，域名才有 IP 可以指向。'} />
          : noAccounts
            ? <Empty icon={GlobeIcon} title="先连接 DNS 账号" description="连接 Cloudflare、腾讯云、阿里云或华为云账号后，添加解析只需要填写域名和服务器。"><Button onClick={connect.open}>连接 DNS 账号</Button></Empty>
            : <Empty icon={GlobeIcon} title="还没有解析" description="添加后，域名会指向所选服务器的公网 IP；服务器换 IP 时自动更新。"><Button disabled={addDisabled} onClick={() => open('new')}><PlusIcon aria-hidden="true" />添加解析</Button></Empty>
        : <ul className="md:grid md:grid-cols-[minmax(0,1.6fr)_minmax(0,1fr)_minmax(0,1.1fr)_auto]">
          <li aria-hidden="true" className="text-muted-foreground hidden border-b px-4 py-2 text-xs md:col-span-4 md:grid md:grid-cols-subgrid md:gap-x-4"><span>域名</span><span>服务器</span><span>解析到</span><span className="text-right">状态</span></li>
          {visibleRules.map(rule => {
            const state = ruleState(rule, available)
            const idle = action.busy || !available || rule.busy
            const note = [rule.config.name !== rule.config.record_name && rule.config.name, mixed && providers[rule.config.provider ?? 'cloudflare'], rule.config.proxied && '已开启 Cloudflare 代理'].filter(Boolean).join(' · ')
            // Phones: domain with its state on top, server and address below. Wider
            // screens: one row aligned to the header columns.
            return <li key={rule.id} data-slot="ddns-rule" className="grid gap-y-2 border-b px-4 py-3 last:border-b-0 md:col-span-4 md:grid-cols-subgrid md:items-center md:gap-x-4">
              <div className="flex items-start justify-between gap-2 md:contents">
                <div className="min-w-0 md:col-start-1 md:row-start-1">
                  <div className="flex min-w-0 items-center gap-2"><h3 className="m-0 min-w-0 font-mono text-sm font-medium wrap-anywhere">{rule.config.record_name}</h3><span className="bg-muted text-muted-foreground shrink-0 rounded px-1 text-[11px] font-medium">{rule.config.record_type === 'A' ? 'IPv4' : 'IPv6'}</span></div>
                  {note && <small className="text-muted-foreground block truncate text-xs">{note}</small>}
                </div>
                <div className="flex shrink-0 items-center justify-end gap-1 md:col-start-4 md:row-start-1">
                  <Badge tone={state.tone}>{state.label}</Badge>
                  <Button variant="ghost" size="icon-sm" className="max-md:hidden" aria-label="立即同步" title="立即同步" disabled={idle || !rule.config.enabled} onClick={() => sync(rule)}><RefreshCwIcon aria-hidden="true" /></Button>
                  <ActionMenu label="解析操作" groups={[
                    [{ label: '立即同步', disabled: idle || !rule.config.enabled, onSelect: () => sync(rule) }, { label: '编辑', disabled: idle, onSelect: () => open(rule) }, { label: rule.config.enabled ? '暂停' : '恢复同步', disabled: idle, onSelect: () => toggle(rule) }, { label: '核对与历史', disabled: idle, onSelect: () => setLifecycle(rule) }],
                    [{ label: '删除', destructive: true, disabled: idle, onSelect: () => { action.clearError(); setRemoving(rule) } }],
                  ]} />
                </div>
              </div>
              <div className="grid grid-cols-2 gap-x-4 md:contents">
                <div className="min-w-0 self-center text-sm wrap-anywhere md:col-start-2 md:row-start-1">{rule.server_name}</div>
                <div className="min-w-0 md:col-start-3 md:row-start-1"><Address rule={rule} known={available} /></div>
              </div>
              {state.detail && <p className="text-warning m-0 text-xs leading-relaxed wrap-anywhere md:col-span-4 md:row-start-2" role="status">{state.detail}</p>}
            </li>
          })}
        </ul>}
    </Panel>
    {servers.data && <DnsAccounts accounts={accounts} servers={servers.data} connect={connect.open} />}
    {connect.dialog}
    <DialogPresence>{migrating && rules.data && servers.data && <Migration rules={visibleRules ?? []} servers={servers.data} accounts={accounts.data ?? []} close={() => setMigrating(false)} saved={() => { setMigrating(false); refresh() }} />}</DialogPresence>
    <DialogPresence>{lifecycle && <Lifecycle rule={rules.data?.find(rule => rule.id === lifecycle.id) ?? lifecycle} close={() => setLifecycle(null)} changed={refresh} />}</DialogPresence>
    <DialogPresence>{editing && servers.data && <RuleEditor key={editing === 'new' ? 'new' : editing.id} rule={editing === 'new' ? undefined : editing} servers={scope} accounts={accounts.data ?? []} current={current} close={() => setEditing(null)} saved={() => { setEditing(null); refresh() }} connect={() => { setEditing(null); connect.open() }} />}</DialogPresence>
    <DialogPresence>{removing && <Confirm title="删除解析" busy={action.busy} error={action.error} confirmDisabled={Boolean(ruleError(current, removing))} onClose={() => setRemoving(null)} onConfirm={() => perform(() => ruleError(current, removing), () => api(`/api/plugins/ddns/rules/${removing.id}`, 'DELETE'), () => { setRemoving(null); rules.reload() })}>确认删除“{removing.config.record_name}”的自动解析？DNS 提供方中的记录会保留，只是之后不再自动更新。</Confirm>}</DialogPresence>
  </div>
}
