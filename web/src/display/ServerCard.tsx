import { memo } from 'react'
import { ArrowDownIcon, ArrowUpIcon, CalendarIcon, WalletIcon } from 'lucide-react'
import type { ProbeOverview } from '../probes'
import ProbeQuality from './ProbeQuality'
import { time, uptime } from '../format'
import type { Server } from '../types'
import { fresh, network, number, percentage, ratio, size, speed, status } from './data'
import { OSIcon, Region } from './Icon'
import { dashboardServer } from './dashboard'
import { assetDate, assetPrice, defaultAssets, expiryState, trafficSize } from '../server-assets'
import { useCurrency } from './CurrencyContext'
import { convert, money, remainingCost } from './currency'
import CurrencyReference from './CurrencyReference'
import { Track, surface, toneFill, toneText } from './ui'
import { cn } from '@/lib/utils'

export function Metric({ label, value, detail, display }: { label: string; value: number | null; detail: string; display?: string }) {
  const tone = value !== null && value >= 90 ? 'danger' : value !== null && value >= 75 ? 'warning' : 'good'
  return <div data-slot="metric" className="flex min-w-0 flex-col gap-1.5">
    <div className="flex min-w-0 items-center justify-between gap-2 text-sm leading-4 whitespace-nowrap">
      <span className="text-muted-foreground min-w-0 truncate">{label}</span>
      <strong className={cn('flex-none font-medium tabular-nums', value !== null && value >= 75 && toneText[tone])}>{display ?? percentage(value)}</strong>
    </div>
    <Track value={value} tone={tone} />
    <small className="text-muted-foreground truncate text-xs leading-3.5 tabular-nums" title={detail}>{detail}</small>
  </div>
}

const datum = 'bg-muted/50 flex min-w-0 flex-col gap-1 overflow-hidden rounded-lg px-2 py-1.5 [&>span]:flex [&>span]:min-w-0 [&>span]:items-center [&>span]:gap-1 [&>span]:truncate [&>span]:text-xs [&>span]:tabular-nums [&_svg]:size-3 [&_svg]:flex-none'

export const ServerCard = memo(function ServerCard({ server, unavailable, probes, probeError, probeLoading, now }: { server: Server; unavailable: boolean; probes?: ProbeOverview[]; probeError: boolean; probeLoading: boolean; now: number }) {
  const metrics = server.latest_metrics, info = server.static_info
  const state = status(server, unavailable), live = fresh(server) && !unavailable
  const up = network(metrics, 'transmit_bytes_per_sec', server.asset_settings?.network_interface), down = network(metrics, 'receive_bytes_per_sec', server.asset_settings?.network_interface)
  const asset = { ...defaultAssets, ...server.asset_settings }
  const { currency, quote } = useCurrency()
  const quota = asset.traffic_limit !== '0'
  const trafficKnown = server.traffic?.observed_from != null || server.traffic?.corrected
  const showAsset = !server.public_view && (asset.price !== null || asset.expires_at !== null)
  const expiry = expiryState(asset, now)
  const remainder = convert(remainingCost(asset, now), asset.currency, currency, quote)
  const sampleTime = server.metrics_sampled_at ? time(server.metrics_sampled_at / 1000) : '采样时间未知'
  const offline = !server.online && !unavailable
  return <a data-slot="server-card" data-offline={offline || undefined} href={dashboardServer(server.id)} aria-label={`${server.name}，${state.label}，查看详情`}
    className={cn(surface, 'group/card hover:ring-foreground/20 hover:bg-card focus-visible:ring-ring relative flex w-full min-w-0 flex-col overflow-hidden text-left transition-[box-shadow,background-color] outline-none hover:shadow-md focus-visible:ring-2', offline && 'ring-destructive/30 ring-2')}>
    <div data-slot="card-header" className="flex min-h-12 min-w-0 items-center gap-2 px-4 py-3">
      <span className={cn('size-2 flex-none rounded-full', toneFill[state.tone])} title={state.label} />
      <strong className="min-w-0 flex-1 truncate text-sm font-semibold" title={[server.name, asset.group_name, ...asset.tags].filter(Boolean).join(' · ')}>{server.name}</strong>
      <OSIcon system={info.system} /><Region region={asset.region} />
    </div>
    <div className="relative flex flex-1 flex-col gap-2.5 px-4 pb-4">
      <div className="flex h-5 gap-1.5 overflow-hidden text-xs [&>span]:bg-muted [&>span]:text-muted-foreground [&>span]:truncate [&>span]:rounded-full [&>span]:px-2 [&>span]:py-0.5 [&>span]:leading-4 [&>span+span]:ml-auto [&>span+span]:shrink-0">
        <span title={`${info.system ?? '系统尚未上报'} · ${info.arch ?? '架构未知'} · 最近采样 ${sampleTime}`}>{live && metrics.uptime_secs !== undefined ? `运行 ${uptime(metrics.uptime_secs)}` : state.label}</span>
        {!server.public_view && asset.price !== null && <span title={assetPrice(asset)}>{assetPrice(asset)}</span>}
      </div>
      <div className={cn('grid grid-cols-2 gap-x-4 gap-y-3', !live && '[&_[data-slot=metric]_span>span]:opacity-45')}>
        <Metric label={`处理器${info.cpu_cores ? ` · ${info.cpu_cores} 核` : ''}`} value={number(metrics.cpu_percent)} detail={[metrics.load_1, metrics.load_5, metrics.load_15].map(value => number(value) === null ? '—' : value!.toFixed(2)).join(' / ')} />
        <Metric label="内存" value={ratio(metrics.memory_used, info.memory_total)} detail={`${size(metrics.memory_used)} / ${size(info.memory_total)}`} />
        <Metric label="磁盘" value={ratio(metrics.disk_used, info.disk_total)} detail={`${size(metrics.disk_used)} / ${size(info.disk_total)}`} />
        <Metric label="流量" value={quota ? server.traffic?.percent ?? null : null} display={quota ? undefined : '∞'} detail={`${trafficKnown ? trafficSize(server.traffic?.used) : '—'} / ${quota ? trafficSize(asset.traffic_limit) : '∞'}${server.traffic?.incomplete ? ' · 不完整' : ''}`} />
      </div>
      <div data-slot="data-grid" className={cn('grid gap-1.5', showAsset ? 'grid-cols-3' : 'grid-cols-2')}>
        <div data-slot="data" className={datum} title="实时速率" aria-label="实时速率"><span className="text-success"><ArrowUpIcon aria-hidden="true" />{live ? speed(up) : '—'}</span><span className="text-info"><ArrowDownIcon aria-hidden="true" />{live ? speed(down) : '—'}</span></div>
        <div data-slot="data" className={cn(datum, '[&>span]:text-muted-foreground')} title="累计流量" aria-label="累计流量"><span><ArrowUpIcon aria-hidden="true" />{size(network(metrics, 'transmitted_bytes', server.asset_settings?.network_interface))}</span><span><ArrowDownIcon aria-hidden="true" />{size(network(metrics, 'received_bytes', server.asset_settings?.network_interface))}</span></div>
        {showAsset && <div data-slot="data" className={datum} aria-label="剩余价值与到期"><span className={expiry.tone === 'bad' ? 'text-destructive' : expiry.tone === 'warm' ? 'text-warning' : 'text-muted-foreground'} title={`到期 ${assetDate(asset.expires_at)}（UTC）`}><CalendarIcon aria-hidden="true" />{expiry.label}</span><span className="text-muted-foreground" title={`剩余价值 ${money(remainder, currency)}`}><WalletIcon aria-hidden="true" />{money(remainder, currency)}</span>{asset.price !== null && <CurrencyReference from={asset.currency} to={currency} compact />}</div>}
      </div>
      <ProbeQuality probes={probes} now={now} online={server.online} unavailable={probeError || unavailable} loading={probeLoading} />
      {!live && server.online && <div className="text-warning flex items-center justify-between gap-2 text-xs" title={`最近采样 ${sampleTime}`}>{unavailable ? '状态待确认' : server.metrics_stale ? '指标已过期' : '采样时间未知'}</div>}
      {offline && <div data-slot="offline-overlay" className="bg-card/60 supports-backdrop-filter:backdrop-blur-[3px] supports-backdrop-filter:backdrop-saturate-[.2] absolute inset-0 flex flex-col items-center justify-center">
        <strong className="text-destructive text-sm font-semibold">{state.label}</strong>
        <span className="text-muted-foreground mt-1 max-w-full px-2 text-center text-xs">{server.last_seen ? `最后在线 ${time(server.last_seen)}` : '等待设备首次接入'}</span>
      </div>}
    </div>
  </a>
})
