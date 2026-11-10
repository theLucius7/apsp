import { useState } from 'react'
import type { ReactNode } from 'react'
import { TrafficHistory } from '../../costs/cloud/TrafficHistory'
import { readingStale } from '../../costs/cloud/usageHistory'
import { useNow } from '../../observability/display/useNow'
import { Badge, Empty, ErrorNotice } from '../../components'
import type { Tone } from '../../components'
import { ActionMenu } from '../../components/action-menu'
import { CdtGuardSummary, cdtGuardFacts } from '../../operations/cloud/CdtGuard'
import { cloudBusy } from '../../operations/cloud/guards'
import { billUsable, charge, exactUsage, gib, message, powerStatus, stopMode, time, updated, utcOffset } from './types'
import type { Account, Overview, Resource } from './types'
import { Button } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { ServerIcon } from 'lucide-react'

/** What the page lets an administrator do; each opens the existing guarded dialog or request. */
export type CloudActions = {
  available: boolean
  busy: boolean
  full: boolean
  refreshAccount: (account: Account) => void
  editAccount: (account: Account) => void
  addResource: (account: Account) => void
  removeAccount: (account: Account) => void
  refreshResource: (resource: Resource, account?: Account) => void
  editResource: (resource: Resource) => void
  bandwidth: (resource: Resource) => void
  powerPolicy: (resource: Resource) => void
  power: (resource: Resource, verb: 'start' | 'stop') => void
  rotateEip: (resource: Resource) => void
  resume: (resource: Resource, account?: Account) => void
  removeResource: (resource: Resource, account?: Account) => void
}

const Figure = ({ label, children }: { label: string; children: ReactNode }) => <div className="min-w-0">
  <dt className="text-muted-foreground text-xs">{label}</dt>
  <dd className="mt-0.5 text-lg leading-tight font-semibold tabular-nums wrap-anywhere">{children}</dd>
</div>

/** The account's CDT pools, and its bill and balance only where the account queries them. */
function AccountUsage({ account }: { account: Account }) {
  const traffic = account.traffic, billing = account.billing_enabled !== false, balance = account.balance_enabled !== false
  const usage = exactUsage(account.bill?.usage_micro_gb)
  const now = useNow() / 1000
  const [history, setHistory] = useState(false)
  const errors = [account.traffic_error && `用量：${message(account.traffic_error)}`, account.error_code && `${billing ? '账单' : '账号'}：${message(account.error_code)}`, balance && account.balance_error && `余额：${message(account.balance_error)}`].filter(Boolean).join('；')
  return <div data-slot="cloud-account-usage" className="grid gap-3 border-b px-4 py-3.5">
    <dl className="grid grid-cols-2 gap-x-6 gap-y-3 sm:flex sm:flex-wrap">
      <Figure label="中国内地 CDT">{traffic ? `${gib(traffic.mainland_bytes)} GiB` : '—'}</Figure>
      <Figure label="海外 CDT">{traffic ? `${gib(traffic.overseas_bytes)} GiB` : '—'}</Figure>
      {billing && <Figure label={`${account.bill?.month ?? '当月'} 已出账`}>{usage !== null ? `${usage.toLocaleString('zh-CN', { maximumFractionDigits: 3 })} GB` : '—'}</Figure>}
      {balance && <Figure label="可用额度">{account.balance ? `${account.balance.available} ${account.balance.currency}` : '尚未查询'}</Figure>}
    </dl>
    <div className="text-muted-foreground flex flex-wrap gap-x-4 gap-y-1 text-xs" aria-label="分项采集时间">
      <span>用量采集于 {updated(traffic?.queried_at ?? null)}{readingStale(traffic?.queried_at, now, 900, account.traffic_error, true) ? ' · 待更新' : ''}</span>
      {billing && <span>账单采集于 {updated(account.bill?.queried_at ?? null)}{readingStale(account.bill?.queried_at, now, 900, account.error_code, true) ? ' · 待更新' : ''}</span>}
      {balance && <span>余额采集于 {updated(account.balance?.queried_at ?? null)}{readingStale(account.balance?.queried_at, now, 21600, account.balance_error) ? ' · 待更新' : ''}</span>}
      <Button variant="link" size="sm" onClick={() => setHistory(true)}>用量历史</Button>
    </div>
    {history && <TrafficHistory account={account} close={() => setHistory(false)} />}
    {billing && account.auto_enabled && account.enabled && <p className="m-0 text-[13px]">自动降速：已出账达到 {account.limit_gb} GB 后降低所选资源带宽{billUsable(account) ? '' : ' · 等待当月完整账单'}</p>}
    {errors && <ErrorNotice message={errors} />}
    {(traffic?.regions.length || billing) && <div className="flex flex-wrap gap-x-5 gap-y-2 text-[13px]">
      {!!traffic?.regions.length && <details><summary className="text-muted-foreground">地域明细</summary><ul className="mt-2 grid gap-1">{traffic.regions.map(row => <li key={row.region} className="tabular-nums">{row.region}：{gib(row.bytes)} GiB</li>)}</ul></details>}
      {billing && <details className="min-w-0 basis-full sm:basis-auto"><summary className="text-muted-foreground">账单项目（{account.bill?.rows.length ?? 0}）</summary><p className="text-muted-foreground mt-2 text-xs leading-relaxed">列出全部 CDT 项目，可能同时包含公网与跨地域流量，不按币种混加费用。无账单不代表未开通或没有用量。</p><div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>地域 / 实例</TableHead><TableHead>账单项目</TableHead><TableHead>用量</TableHead><TableHead>费用</TableHead></TableRow></TableHeader><TableBody>{account.bill?.rows.map((row, index) => <TableRow key={index}><TableCell>{row.region || '—'}<br /><code>{row.instance_id || '—'}</code></TableCell><TableCell>{row.billing_item || row.product_type || '—'}</TableCell><TableCell>{row.usage || '—'} {row.unit}</TableCell><TableCell>{row.amount || '—'} {row.currency || '币种未知'}</TableCell></TableRow>)}</TableBody></Table></div></details>}
    </div>}
  </div>
}

const powerTones: Record<string, Tone> = { Running: 'good', Stopped: 'neutral' }
const billItems: Record<string, string> = { SubscriptionOrder: '预付订单', PayAsYouGoBill: '后付账单', Refund: '退款', Adjustment: '调账' }

/** The automatic start/stop and bandwidth rules that apply, in one line each. */
function Automation({ resource, account }: { resource: Resource; account?: Account }) {
  const policy = resource.power_policy, attached = resource.snapshot?.attached_eip_id != null
  const rules: string[] = []
  if (resource.kind === 'ecs' && policy?.enabled) {
    if (policy.threshold_action !== 'off') rules.push(`已出账达到 ${policy.limit_gb} GB 的 ${policy.threshold_percent}% 时${policy.threshold_action === 'stop' ? '停机' : '通知'}`)
    if (policy.schedule_enabled && policy.schedule_mode === 'cron') rules.push(`Cron：${policy.start_cron} 开机、${policy.stop_cron} 停机（${utcOffset(policy.utc_offset_minutes)}）`)
    else if (policy.schedule_enabled) rules.push(`每日 ${policy.start_time} 开机、${policy.stop_time} 停机（${utcOffset(policy.utc_offset_minutes)}）`)
    if (policy.keepalive) rules.push('抢占式保活')
  }
  const downgrade = resource.auto_enabled && !attached ? `自动降速：降至 ${resource.cap_mbps} Mbps${account?.auto_enabled && account.enabled ? '' : '（账号未开启自动降速）'}` : ''
  if (!policy?.cdt_guard && !rules.length && !downgrade && !(resource.kind === 'ecs' && policy?.enabled)) return null
  return <div className="grid gap-2">
    {resource.kind === 'ecs' && <CdtGuardSummary resource={resource} />}
    {resource.kind === 'ecs' && policy?.enabled && rules.length > 0 && <p className="m-0 text-[13px]"><span className="font-medium">自动启停</span> · {rules.length ? rules.join(' · ') : '未设置动作'}</p>}
    {downgrade && <p className="m-0 text-[13px]">{downgrade}</p>}
  </div>
}

/** IDs, stop mode, spot details, guard state, query times and the instance bill: facts read rarely, folded. */
function ResourceDetails({ resource, account }: { resource: Resource; account?: Account }) {
  const state = resource.power_state, snapshot = resource.snapshot, ecs = resource.kind === 'ecs'
  const facts: [string, ReactNode][] = [
    [ecs ? 'ECS 实例' : 'EIP', <code key="id">{resource.cloud_id}</code>],
    ...(snapshot?.attached_eip_id ? [['公网来源', <>绑定 EIP <code>{snapshot.attached_eip_id}</code>，带宽与计费请在<a className="text-primary underline-offset-4 hover:underline" href="https://vpc.console.aliyun.com/" target="_blank" rel="noreferrer">EIP 控制台</a>管理</>]] as [string, ReactNode][] : []),
    ...(state?.status === 'Stopped' ? [['停机模式', state.stopped_mode ? stopMode(state.stopped_mode) : '待核实']] as [string, ReactNode][] : []),
    ...(state && state.spot_strategy !== 'NoSpot' ? [['实例类型', `抢占式实例${state.interruption_behavior === 'Terminate' ? '，云端中断会释放实例，释放后无法保活' : ''}`]] as [string, ReactNode][] : []),
    ...cdtGuardFacts(resource),
    ...(ecs ? [['IPv6 查询', `${time(resource.ipv6_checked_at ?? null)}${resource.ipv6_error ? ' · ' + message(resource.ipv6_error) + '；请核对 ecs:DescribeNetworkInterfaces 权限' : ''}`]] as [string, ReactNode][] : []),
    ['查询时间', `配置 ${time(resource.checked_at)}${ecs ? ` · 运行状态 ${time(resource.power_checked_at ?? null)}` : ''}`],
  ]
  const bill = resource.instance_bill
  return <details data-slot="cloud-resource-details" className="text-[13px]">
    <summary className="text-muted-foreground w-fit">详情</summary>
    <dl className="mt-2 grid gap-x-6 gap-y-2 sm:grid-cols-[max-content_minmax(0,1fr)]">{facts.map(([label, value]) => <div key={label} className="contents"><dt className="text-muted-foreground">{label}</dt><dd className="m-0 wrap-anywhere">{value}</dd></div>)}
      {account?.billing_enabled !== false && <div className="contents"><dt className="text-muted-foreground">实例账单</dt><dd className="m-0 wrap-anywhere">{bill?.rows.length ? bill.rows.map((row, index) => <span key={index} className="block">{billItems[row.item] ?? '账单项目'}：{row.amount} {row.currency}</span>) : '无项目'}<span className="text-muted-foreground block text-xs">{bill?.month ?? '当月'} · {time(bill?.queried_at ?? null)} · 缓存 6 小时，跨月重新查询</span>{resource.bill_error && <span className="text-destructive block text-xs">{message(resource.bill_error)}</span>}</dd></div>}
    </dl>
  </details>
}

function ResourceRow({ resource, account, data, actions }: { resource: Resource; account?: Account; data: Overview; actions: CloudActions }) {
  const ecs = resource.kind === 'ecs', state = resource.power_state, snapshot = resource.snapshot, attached = snapshot?.attached_eip_id != null
  const active = cloudBusy(data, resource.id), editable = actions.available && !actions.busy, managed = editable && Boolean(account?.enabled)
  const running = state?.status === 'Running', stopped = state?.status === 'Stopped'
  const ip = snapshot?.public_ip || state?.public_ips.join('、') || ''
  const errors = [message(resource.error_code), resource.power_error && `启停：${message(resource.power_error)}`, resource.cdt_guard_error && `CDT 守护：${message(resource.cdt_guard_error)}`].filter(Boolean).join('；')
  return <article data-slot="cloud-resource" className="grid gap-3 border-b px-4 py-4 last:border-b-0">
    <div className="flex flex-wrap items-start justify-between gap-x-4 gap-y-2">
      <div className="grid min-w-0 gap-1">
        <h3 className="m-0 flex flex-wrap items-center gap-2 text-sm font-semibold">{resource.name}
          {ecs && <Badge tone={state ? powerTones[state.status] ?? 'warm' : 'neutral'}>{powerStatus(state?.status)}</Badge>}
          {active && <Badge tone="warm">有待处理操作</Badge>}
          {errors && <Badge tone="warm">查询异常</Badge>}
          {state?.locked && <Badge tone="bad">云端锁定</Badge>}
          {resource.threshold_hold && <Badge tone="bad">流量停机保护</Badge>}
          {resource.manual_hold && <Badge tone="warm">已手动暂停开机</Badge>}
        </h3>
        <p className="text-muted-foreground m-0 flex flex-wrap gap-x-1.5 text-xs">{snapshot ? <>
          {ip && <><span className="tabular-nums">{ip}</span><span aria-hidden="true">·</span></>}
          <span>{snapshot.bandwidth_mbps} Mbps · {charge(snapshot.charge_type)}</span><span aria-hidden="true">·</span>
        </> : <><span>尚未查询配置</span><span aria-hidden="true">·</span></>}<span>{resource.region}</span></p>
      </div>
      <div className="flex flex-wrap items-center gap-1.5">
        {resource.manual_hold && ecs && <Button variant="ghost" size="sm" disabled={!managed || active} onClick={() => actions.resume(resource, account)}>恢复自动开机</Button>}
        {ecs && !stopped && <Button variant="outline" size="sm" disabled={!managed || active} onClick={() => actions.power(resource, 'stop')}>停机</Button>}
        {ecs && !running && <Button variant="outline" size="sm" disabled={!managed || active} onClick={() => actions.power(resource, 'start')}>开机</Button>}
        <ActionMenu label="资源操作" groups={[
          [{ label: '查询当前配置', disabled: !managed, onSelect: () => actions.refreshResource(resource, account) },
            ecs && { label: '配置自动启停', disabled: !editable, onSelect: () => actions.powerPolicy(resource) },
            // Only a running instance with an attached EIP can be moved to a new address.
            ecs && attached && { label: '更换公网 IP', disabled: !managed || active || !running, onSelect: () => actions.rotateEip(resource) },
            !attached && { label: '调整带宽与计费', disabled: !managed || active || (resource.kind === 'eip' && !snapshot), onSelect: () => actions.bandwidth(resource) }],
          [{ label: '编辑登记与降速', disabled: !editable, onSelect: () => actions.editResource(resource) }],
          [{ label: '移除登记', destructive: true, disabled: !editable || active, onSelect: () => actions.removeResource(resource, account) }],
        ]} />
      </div>
    </div>
    {(resource.ipv6_addresses?.length || resource.ipv6_error) ? <p className="text-muted-foreground m-0 text-[13px] wrap-anywhere">IPv6：{resource.ipv6_addresses?.join('、') || '尚未查询'}{resource.ipv6_error ? ' · 读取失败，保留上次地址' : ''}</p> : null}
    <Automation resource={resource} account={account} />
    {errors && <ErrorNotice message={errors} />}
    <ResourceDetails resource={resource} account={account} />
  </article>
}

/**
 * One card per cloud account: its CDT pools first, then each registered
 * resource with its state, guard and actions. Resources of a removed account
 * (normally impossible, since removal needs an empty account) get a card
 * without an account.
 */
export function AccountPanel({ account, resources, data, actions }: { account?: Account; resources: Resource[]; data: Overview; actions: CloudActions }) {
  const editable = actions.available && !actions.busy
  return <Panel data-slot="cloud-account">
    <PanelHeader>
      <PanelTitle>{account?.name ?? '账号已移除'}{account && !account.enabled && <Badge>已停用</Badge>}{account?.site === 'international' && <Badge>国际站</Badge>}</PanelTitle>
      <div className="flex items-center gap-1">
        {account && <ActionMenu label="账号操作" groups={[
          [{ label: '请求更新', disabled: !editable || !account.enabled, onSelect: () => actions.refreshAccount(account) }],
          [{ label: '编辑账号与策略', disabled: !editable, onSelect: () => actions.editAccount(account) },
            { label: '登记资源', disabled: !editable || actions.full, onSelect: () => actions.addResource(account) }],
          [{ label: '移除账号', destructive: true, disabled: !editable, onSelect: () => actions.removeAccount(account) }],
        ]} />}
      </div>
    </PanelHeader>
    {account && <AccountUsage account={account} />}
    {resources.length ? resources.map(resource => <ResourceRow key={resource.id} resource={resource} account={account} data={data} actions={actions} />)
      : <Empty icon={ServerIcon} title="尚未登记资源" description="按地域和资源 ID 登记此账号下的 ECS 或独立 EIP，不会在阿里云新建资源。">{account && <Button variant="outline" size="sm" disabled={!editable || actions.full} onClick={() => actions.addResource(account)}>登记资源</Button>}</Empty>}
  </Panel>
}
