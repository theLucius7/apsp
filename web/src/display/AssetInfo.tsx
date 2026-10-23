import type { Server } from '../types'
import { assetDate, assetPrice, defaultAssets, expiryState, trafficModes, trafficSize } from '../server-assets'
import { useCurrency } from './CurrencyContext'
import { convert, money, price, remainingCost } from './currency'
import CurrencyReference from './CurrencyReference'
import { Track, surface } from './ui'
import { cn } from '@/lib/utils'

export function AssetChips({ server, className }: { server: Server; className?: string }) {
  const asset = { ...defaultAssets, ...server.asset_settings }
  return asset.region || asset.group_name || asset.tags.length ? <div data-slot="asset-chips" className={cn('mb-3 flex flex-wrap gap-1.5 [&>span]:bg-muted [&>span]:text-muted-foreground [&>span]:max-w-full [&>span]:rounded-md [&>span]:px-1.5 [&>span]:py-0.5 [&>span]:text-xs [&>span]:font-normal [&>span]:wrap-anywhere', className)}>
    {asset.region && <span title="地区">{asset.region}</span>}{asset.group_name && <span title="分组">{asset.group_name}</span>}{asset.tags.map(tag => <span key={tag} title={tag}>{tag}</span>)}
  </div> : null
}

export default function AssetInfo({ server, now, detail = false }: { server: Server; now: number; detail?: boolean }) {
  const asset = { ...defaultAssets, ...server.asset_settings }, traffic = server.traffic
  const { currency, quote } = useCurrency()
  const converted = convert(price(asset.price), asset.currency, currency, quote)
  const remainder = convert(remainingCost(asset, now), asset.currency, currency, quote)
  const expiry = expiryState(asset, now)
  const configured = asset.price !== null || asset.expires_at !== null || asset.traffic_limit !== '0'
  if (!configured && !detail) return null
  const row = 'flex flex-wrap items-baseline justify-between gap-x-3.5 gap-y-1.5 wrap-anywhere'
  return <section data-slot={detail ? 'asset-detail' : 'asset-info'} className={cn('grid gap-2.5 text-sm', detail ? cn(surface, 'p-5 max-sm:p-3.5') : 'mt-3 border-t pt-3')} aria-label="资产与流量额度">
    {detail && <h2 className="mb-1 text-sm font-semibold">资产与流量额度</h2>}
    {!server.public_view && <div className={row}><span>{assetPrice(asset)}</span><span className={expiry.tone === 'bad' ? 'text-destructive' : expiry.tone === 'warm' ? 'text-warning' : undefined} title={`到期日期（UTC）：${assetDate(asset.expires_at)}`}>{expiry.label}{asset.auto_renewal ? ' · 自动顺延' : ''}</span></div>}
    {!server.public_view && asset.price !== null && (asset.currency !== currency || detail) && <div data-slot="asset-conversion" className="text-muted-foreground flex flex-wrap justify-between gap-x-3 gap-y-1 text-xs [&>small]:basis-full"><span>{converted === null ? `缺少 ${asset.currency} → ${currency} 汇率` : `折算 ${money(converted, currency)} / ${asset.billing_cycle ? `${asset.billing_cycle} 天` : '一次性'}`}</span>{remainder !== null && <span>剩余价值 {money(remainder, currency)}</span>}<CurrencyReference from={asset.currency} to={currency} /></div>}
    {detail && !server.public_view && <><AssetChips server={server} className="m-0" /><p className="text-muted-foreground leading-relaxed">到期日期（UTC）：{assetDate(asset.expires_at)}。{asset.auto_renewal ? '自动顺延仅更新日期记录。' : ''}</p></>}
    <div className={row}><span>{trafficModes[asset.traffic_limit_type]} · 本期观测</span><strong className={cn('font-medium', traffic?.exceeded && 'text-destructive')}>{(traffic?.observed_from != null || traffic?.corrected) ? trafficSize(traffic.used) : '等待采样'} / {asset.traffic_limit === '0' ? '未设额度' : trafficSize(asset.traffic_limit)}</strong></div>
    {traffic?.percent != null && <Track value={traffic.percent} tone={traffic.exceeded ? 'danger' : traffic.percent >= 80 ? 'warning' : 'good'} label={`流量额度已使用 ${traffic.percent.toFixed(1)}%`} />}
    <div className={cn(row, 'text-muted-foreground text-xs')}><span>每月 {asset.reset_day} 日重置 · UTC</span><span>{traffic?.exceeded ? '额度已用尽' : traffic?.remaining != null ? `剩余 ${trafficSize(traffic.remaining)}` : traffic?.incomplete ? '观测不完整' : traffic?.corrected ? '已矫正' : '网卡观测值'}</span></div>
    {detail && <><p className="text-muted-foreground leading-relaxed">本期：{traffic ? `${assetDate(traffic.cycle_start)} 至 ${assetDate(traffic.cycle_end)}（不含）` : '等待读取'}。上传 {(traffic?.observed_from != null || traffic?.corrected) ? trafficSize(traffic.uploaded) : '—'}，下载 {(traffic?.observed_from != null || traffic?.corrected) ? trafficSize(traffic.downloaded) : '—'}。</p><p className="text-muted-foreground leading-relaxed">{!server.public_view && <>统计网卡：{asset.network_interface || '所有上报网卡'}。</>}{traffic?.observed_from != null ? `本期观测始于 ${new Date(traffic.observed_from).toLocaleString('zh-CN')}。` : '首次有效采样仅建立基线。'}{traffic?.incomplete ? '检测到断档、网卡变化或计数重置，观测可能不完整。' : ''}额度仅用于状态提示，可能与供应商账单不同。</p></>}
  </section>
}
