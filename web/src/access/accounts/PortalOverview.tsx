import type { ReactNode } from 'react'
import { DownloadIcon, LinkIcon } from 'lucide-react'
import { Badge, CopyField, Refresh } from '../../components'
import type { Tone } from '../../components'
import { bytes, time } from '../../format'
import { buttonVariants } from '@/components/ui/button'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'
import { ProgressRing } from '@/components/progress-ring'

export type Entitlement = { package_name: string | null; used_bytes: string; monthly_bytes: string | null; expires_at: number | null; next_reset: number | null; cycle_start?: number | null }
export type SubscriptionStatus = { status: string; message: string; entitlement: Entitlement }

// A package without resets stores its next reset as the last second of year 9999.
const never = 253402300799
const states: Record<string, { label: string; tone: Tone }> = {
  ready: { label: '可以使用', tone: 'good' },
  blocked: { label: '已暂停', tone: 'bad' },
  empty: { label: '暂无可用线路', tone: 'warm' },
  format_unavailable: { label: '订阅暂不可用', tone: 'warm' },
}

const day = (seconds: number) => new Date(seconds * 1000).toLocaleDateString('zh-CN')
function fromNow(seconds: number, now = Date.now() / 1000) {
  const days = Math.ceil((seconds - now) / 86400)
  return days <= 1 ? '一天内' : `${days} 天后`
}
function amount(value: string | null) {
  try { return value === null ? null : BigInt(value) } catch { return 0n }
}

/** The period's share of the quota; the dot marks how far the period has run, so an arc past it is ahead of time. */
function QuotaRing({ used, total, elapsed }: { used: bigint; total: bigint | null; elapsed: number | null }) {
  const level = total === null ? null : total > 0n ? Number(used) / Number(total) * 100 : 100
  const shown = level === null ? '∞' : used > 0n && level < 1 ? '<1%' : `${Math.min(999, Math.round(level))}%`
  const label = level === null ? '不限流量' : `本期已用 ${shown}${elapsed === null ? '' : `，本期已过 ${Math.round(elapsed)}%`}`
  return <ProgressRing value={level} full={level === null} marker={level === null ? null : elapsed} tone={level !== null && level >= 90 ? 'danger' : level !== null && level >= 75 ? 'warning' : 'brand'} label={label} className="size-32 max-sm:size-28">
    <strong className="text-2xl font-semibold tracking-tight">{shown}</strong>
    <small className="text-muted-foreground mt-1.5 block text-[11px]">{level === null ? '不限流量' : '本期已用'}</small>
  </ProgressRing>
}

function Fact({ label, value, note, title }: { label: string; value: ReactNode; note?: string; title?: string }) {
  return <div className="min-w-0">
    <dt className="text-muted-foreground text-xs">{label}</dt>
    <dd className="mt-1 text-[15px] leading-snug font-semibold tabular-nums wrap-anywhere" title={title}>{value}{note && <small className="text-muted-foreground ml-1.5 text-xs font-normal">{note}</small>}</dd>
  </div>
}

/** Who is signed in and whether their package lets them connect: status, quota, renewal and traffic. */
export function PortalSummary({ name, status, usage, legacy, refresh }: { name: string; status: SubscriptionStatus; usage: { uplink: string; downlink: string }; legacy: boolean; refresh: () => void }) {
  const entitlement = status.entitlement, state = states[status.status] ?? { label: status.status, tone: 'neutral' as const }
  const used = amount(entitlement.used_bytes) ?? 0n, total = amount(entitlement.monthly_bytes)
  const left = total === null ? null : total > used ? total - used : 0n
  const now = Date.now() / 1000, expires = entitlement.expires_at
  const reset = entitlement.next_reset && entitlement.next_reset < never ? entitlement.next_reset : null
  // A cycle start of 0 means the bridge does not know it.
  const start = entitlement.cycle_start && entitlement.cycle_start > 0 ? entitlement.cycle_start : null
  const elapsed = start !== null && reset !== null && reset > start ? Math.min(100, Math.max(0, (now - start) / (reset - start) * 100)) : null
  return <Panel data-slot="portal-summary" className="relative mb-0">
    <span className="pointer-events-none absolute inset-0 bg-[radial-gradient(90%_70%_at_100%_0%,color-mix(in_oklch,var(--brand-to)_10%,transparent),transparent_65%)]" aria-hidden="true" />
    <div className="relative p-5">
      <div className="flex items-start gap-3.5">
        <span className="bg-primary from-primary text-primary-foreground shadow-button grid size-11 shrink-0 place-items-center rounded-2xl bg-gradient-to-br to-[color-mix(in_oklch,var(--primary),var(--brand-to)_38%)] text-lg font-semibold" aria-hidden="true">{Array.from(name.trim())[0]?.toUpperCase() ?? '·'}</span>
        <div className="min-w-0 flex-1">
          <h1 className="truncate text-xl leading-tight font-semibold tracking-tight">{name}</h1>
          <p className="mt-1.5 flex min-w-0 flex-wrap items-center gap-2 text-sm"><Badge tone={state.tone}>{state.label}</Badge><span className="text-muted-foreground min-w-0 truncate">{entitlement.package_name ?? '未设置套餐限制'}</span></p>
        </div>
        <Refresh onClick={refresh} />
      </div>
      <div className="mt-5 flex items-center gap-5">
        <QuotaRing used={used} total={total} elapsed={elapsed} />
        <div className="grid min-w-0 flex-1 gap-3.5">
          <dl className="grid gap-3.5">
            <Fact label="本期已用 / 额度" value={`${bytes(used)} / ${total === null ? '不限量' : bytes(total)}`} />
            <Fact label="剩余流量" value={left === null ? '不限量' : bytes(left)} />
          </dl>
          {total !== null && elapsed !== null && <p className="text-muted-foreground flex items-center gap-1.5 text-xs"><span className="border-foreground/70 bg-card size-2 shrink-0 rounded-full border-[1.5px]" aria-hidden="true" />本期已过 {Math.round(elapsed)}%</p>}
        </div>
      </div>
      <dl className="mt-5 grid grid-cols-2 gap-x-4 gap-y-3.5 border-t pt-4">
        <Fact label="到期" value={expires ? day(expires) : '不限期'} note={expires ? expires <= now ? '已到期' : fromNow(expires, now) : undefined} title={expires ? time(expires) : undefined} />
        <Fact label="下次额度重置" value={reset ? day(reset) : '不适用'} note={reset ? fromNow(reset, now) : undefined} title={reset ? time(reset) : undefined} />
        <Fact label={legacy ? '本期上传' : '受管节点累计上传'} value={bytes(usage.uplink)} />
        <Fact label={legacy ? '本期下载' : '受管节点累计下载'} value={bytes(usage.downlink)} />
      </dl>
      <p className="text-muted-foreground mt-4 text-xs leading-relaxed">订阅检查当前授权与套餐状态。受管流量约每三十秒刷新；外部节点用量由提供方计量，已下载的外部凭据也由提供方控制。</p>
    </div>
  </Panel>
}

function CopyBlock({ title, hint, text, label, primary = false }: { title: string; hint?: string; text: string; label: string; primary?: boolean }) {
  return <div className="grid min-w-0 gap-1.5">
    <span className="text-sm font-medium">{title}</span>
    <CopyField text={text} label={label} singleLine primary={primary} className="my-0" />
    {hint && <small className="text-muted-foreground text-xs leading-relaxed">{hint}</small>}
  </div>
}

/** The addresses a client imports; downloading the configuration is the fallback for clients without subscriptions. */
export function PortalSubscription({ url, clash, status }: { url: string; clash?: string; status: SubscriptionStatus }) {
  const ready = status.status === 'ready'
  return <Panel data-slot="portal-subscription" className="mb-0">
    <PanelHeader><PanelTitle><LinkIcon className="text-primary size-4" aria-hidden="true" />导入订阅</PanelTitle>
      {ready && <a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href={`${url}${url.includes('?') ? '&' : '?'}download=true`}><DownloadIcon aria-hidden="true" />下载订阅配置</a>}</PanelHeader>
    <div className="grid gap-4 p-5">
      <p className="text-muted-foreground text-[13px] leading-relaxed">复制地址后在客户端添加订阅。地址只属于你，请勿分享。</p>
      <CopyBlock title="订阅地址" text={url} label="复制订阅地址" primary />
      {clash && <CopyBlock title="Clash 订阅" hint="适用于 Clash Verge、FlClash、mihomo 等 Clash Meta 客户端" text={clash} label="复制 Clash 订阅" />}
      {!ready && <Notice role="status" className="mb-0">{status.message}</Notice>}
    </div>
  </Panel>
}
