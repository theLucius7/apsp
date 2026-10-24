import { useState } from 'react'
import TrafficCorrection from './TrafficCorrection'
import { Badge, Meter, DialogPresence } from '../components'
import { time } from '../format'
import { assetDate, assetPrice, defaultAssets, expiryState, trafficModes, trafficSize } from '../server-assets'
import type { Server } from '../types'
import { Button } from '@/components/ui/button'
import { Panel, PanelHeader, PanelTitle } from '@/components/panel'

export default function ServerAssets({ server, onEdit, onReload }: { server: Server; onEdit: () => void; onReload: () => void }) {
  const [correcting, setCorrecting] = useState<Server | null>(null)
  const asset = { ...defaultAssets, ...server.asset_settings }, traffic = server.traffic
  const expiry = expiryState(asset), observed = traffic?.observed_from != null || traffic?.corrected
  return <><Panel><PanelHeader><PanelTitle>资产与流量额度</PanelTitle><div className="flex flex-wrap items-center justify-end gap-2"><Button variant="ghost" size="sm" disabled={!traffic} onClick={() => setCorrecting(server)}>流量矫正</Button><Button variant="ghost" size="sm" onClick={onEdit}>编辑资产配置</Button></div></PanelHeader><div className="p-4">
    <dl className="m-0 grid gap-4 p-0">
      <div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">地区 / 分组</dt><dd className="m-0 text-sm">{asset.region || '未设置地区'} / {asset.group_name || '未分组'}{asset.hidden && ' · 展示页已隐藏'}</dd></div>
      <div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">标签</dt><dd className="m-0 text-sm">{asset.tags.length ? asset.tags.join(' · ') : '未设置'}</dd></div>
      <div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">成本</dt><dd className="m-0 text-sm">{assetPrice(asset)}</dd></div>
      <div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">到期日期（UTC）</dt><dd className="m-0 text-sm">{assetDate(asset.expires_at)} <Badge tone={expiry.tone}>{expiry.label}</Badge>{asset.auto_renewal && ' · 自动顺延记录'}</dd></div>
      <div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">流量周期（UTC）</dt><dd className="m-0 text-sm">{traffic ? `${assetDate(traffic.cycle_start)} 至 ${assetDate(traffic.cycle_end)}（不含）` : '等待读取周期'} · 每月 {asset.reset_day} 日重置</dd></div>
      <div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">本期观测 / 额度</dt><dd className="m-0 text-sm">{observed ? trafficSize(traffic?.used) : '等待本期采样'} / {asset.traffic_limit === '0' ? '未设额度' : trafficSize(asset.traffic_limit)}{traffic?.exceeded && <Badge tone="bad">额度已用尽</Badge>}</dd></div>
      <div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">本期上传 / 下载</dt><dd className="m-0 text-sm">{observed ? `${trafficSize(traffic?.uploaded)} / ${trafficSize(traffic?.downloaded)}` : '暂无本期数据'}</dd></div>
      <div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">剩余流量 / 口径</dt><dd className="m-0 text-sm">{trafficSize(traffic?.remaining)} / {trafficModes[asset.traffic_limit_type]}</dd></div>
      <div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">统计网卡</dt><dd className="m-0 text-sm">{asset.network_interface || '所有上报网卡'}{traffic?.interfaces.length ? `（已观测：${traffic.interfaces.join('、')}）` : ''}</dd></div>
    </dl>
    {traffic?.percent != null && <div className="my-4 flex items-center gap-3 text-sm"><Meter value={traffic.percent} label="本期流量用量" warning={80} danger={100} /><span>{traffic.percent.toFixed(1)}%</span></div>}
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{observed ? `本期观测始于 ${time(traffic?.observed_from ? traffic.observed_from / 1000 : undefined)}，最新计数 ${time(traffic?.last_sample_at ? traffic.last_sample_at / 1000 : undefined)}。` : '等待具有采样时间的完整网卡计数，首个采样仅建立基线。'}{traffic?.incomplete ? ' 检测到采样缺失、网卡变化或计数重置，本期观测可能不完整。' : ''} 超额与到期仅提示状态；观测流量可能与供应商账单不同。</p>
  </div></Panel><DialogPresence>{correcting && <TrafficCorrection server={correcting} onClose={() => setCorrecting(null)} onSaved={() => { setCorrecting(null); onReload() }} />}</DialogPresence></>
}
