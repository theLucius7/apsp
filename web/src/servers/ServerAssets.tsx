import { useState } from 'react'
import TrafficCorrection from './TrafficCorrection'
import { Badge, Meter } from '../components'
import { time } from '../format'
import { assetDate, assetPrice, defaultAssets, expiryState, trafficModes, trafficSize } from '../server-assets'
import type { Server } from '../types'

export default function ServerAssets({ server, onEdit, onReload }: { server: Server; onEdit: () => void; onReload: () => void }) {
  const [correcting, setCorrecting] = useState<Server | null>(null)
  const asset = { ...defaultAssets, ...server.asset_settings }, traffic = server.traffic
  const expiry = expiryState(asset), observed = traffic?.observed_from != null || traffic?.corrected
  return <><section className="panel"><div className="panel-heading"><h2>资产与流量额度</h2><div className="row-actions"><button className="text-button" disabled={!traffic} onClick={() => setCorrecting(server)}>流量矫正</button><button className="text-button" onClick={onEdit}>编辑资产配置</button></div></div><div className="panel-body">
    <dl className="detail-list">
      <div><dt>地区 / 分组</dt><dd>{asset.region || '未设置地区'} / {asset.group_name || '未分组'}{asset.hidden && ' · 展示页已隐藏'}</dd></div>
      <div><dt>标签</dt><dd>{asset.tags.length ? asset.tags.join(' · ') : '未设置'}</dd></div>
      <div><dt>成本</dt><dd>{assetPrice(asset)}</dd></div>
      <div><dt>到期日期（UTC）</dt><dd>{assetDate(asset.expires_at)} <Badge tone={expiry.tone}>{expiry.label}</Badge>{asset.auto_renewal && ' · 自动顺延记录'}</dd></div>
      <div><dt>流量周期（UTC）</dt><dd>{traffic ? `${assetDate(traffic.cycle_start)} 至 ${assetDate(traffic.cycle_end)}（不含）` : '等待读取周期'} · 每月 {asset.reset_day} 日重置</dd></div>
      <div><dt>本期观测 / 额度</dt><dd>{observed ? trafficSize(traffic?.used) : '等待本期采样'} / {asset.traffic_limit === '0' ? '未设额度' : trafficSize(asset.traffic_limit)}{traffic?.exceeded && <Badge tone="bad">额度已用尽</Badge>}</dd></div>
      <div><dt>本期上传 / 下载</dt><dd>{observed ? `${trafficSize(traffic?.uploaded)} / ${trafficSize(traffic?.downloaded)}` : '暂无本期数据'}</dd></div>
      <div><dt>剩余流量 / 口径</dt><dd>{trafficSize(traffic?.remaining)} / {trafficModes[asset.traffic_limit_type]}</dd></div>
      <div><dt>统计网卡</dt><dd>{asset.network_interface || '所有上报网卡'}{traffic?.interfaces.length ? `（已观测：${traffic.interfaces.join('、')}）` : ''}</dd></div>
    </dl>
    {traffic?.percent != null && <div className="server-assets-progress"><Meter value={traffic.percent} /><span>{traffic.percent.toFixed(1)}%</span></div>}
    <p className="helper">{observed ? `本期观测始于 ${time(traffic?.observed_from ? traffic.observed_from / 1000 : undefined)}，最新计数 ${time(traffic?.last_sample_at ? traffic.last_sample_at / 1000 : undefined)}。` : '等待具有采样时间的完整网卡计数，首个采样仅建立基线。'}{traffic?.incomplete ? ' 检测到采样缺失、网卡变化或计数重置，本期观测可能不完整。' : ''} 超额与到期仅提示状态；观测流量可能与供应商账单不同。</p>
  </div></section>{correcting && <TrafficCorrection server={correcting} onClose={() => setCorrecting(null)} onSaved={() => { setCorrecting(null); onReload() }} />}</>
}
