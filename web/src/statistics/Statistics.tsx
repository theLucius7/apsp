import { useState } from 'react'
import { ErrorNotice, Loading, PageHeader, Refresh, Stat } from '../components'
import { bytes, time } from '../format'
import { useResource } from '../hooks'
import Ranking from './Ranking'
import TrafficChart from './TrafficChart'
import type { ProxyStatistics, ServerStatistics } from './data'

import { buttonVariants } from '@/components/ui/button'
import { ActivityIcon, ArrowDownIcon, ArrowUpIcon, PlusIcon, ServerIcon, UsersIcon, WaypointsIcon } from 'lucide-react'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'

export default function Statistics() {
  const [days, setDays] = useState(7)
  const servers = useResource<ServerStatistics>(`/api/statistics?days=${days}`, 30_000)
  const proxy = useResource<ProxyStatistics>(`/api/plugins/sing-box/statistics?days=${days}`, 30_000)
  const data = servers.data, business = proxy.data
  return <div data-slot="statistics-page" className="min-w-0">
    <PageHeader eyebrow="总览" title="统计报表" description="服务器在线情况、网卡流量与代理流量的趋势和排行，用来回顾一段时间内的整体运行。">
      <SegmentedGroup role="group" aria-label="统计时间范围">{[7, 30].map(value => <SegmentedButton key={value} type="button" aria-pressed={days === value} onClick={() => setDays(value)}>近 {value} 天</SegmentedButton>)}</SegmentedGroup>
      <Refresh onClick={() => { servers.reload(); proxy.reload() }} />
    </PageHeader>
    <ErrorNotice message={servers.error} retry={servers.reload} />
    {servers.loading && !data ? <Loading variant="cards" /> : data && <>
      <div data-slot="statistics-status-line" className="text-muted-foreground mb-3.5 flex flex-wrap justify-between gap-x-5 gap-y-1.5 text-xs"><span>当前服务器 · 含 {data.servers.hidden} 台展示隐藏节点</span><span>{servers.error ? '显示上次成功读取的数据' : '每 30 秒更新'} · {time(data.generated_at)}</span></div>
      <div className="mb-6 grid gap-3 max-sm:gap-2 grid-cols-2 lg:grid-cols-4">
        <Stat label="服务器总数" value={data.servers.total} note="全部未删除服务器" icon={ServerIcon} />
        <Stat label="在线服务器" value={data.servers.online} note="最近 60 秒有通信" icon={ActivityIcon} />
        <Stat label="离线服务器" value={data.servers.offline} note="已接入，等待重新上线" icon={ServerIcon} />
        <Stat label="待接入服务器" value={data.servers.pending} note="等待安装并注册 Agent" icon={PlusIcon} />
      </div>
      <div className="mt-7 mb-4 flex items-center justify-between gap-4 max-sm:items-start"><div><h2 className="text-base font-semibold">服务器网卡流量</h2><p className="text-muted-foreground mt-1 text-xs">近 {days} 个 UTC 自然日，覆盖 {data.traffic.sampled_servers} / {data.servers.total} 台服务器。</p></div><a href="#/servers" className={buttonVariants({ variant: 'ghost', size: 'sm' })}>管理服务器</a></div>
      <div data-slot="statistics-traffic-totals" className="mb-6 grid grid-cols-3 gap-3 max-sm:grid-cols-1 max-sm:gap-2">
        <Stat label="网卡累计上传" value={bytes(data.traffic.uploaded)} note="从服务器发出的观测流量" icon={ArrowUpIcon} />
        <Stat label="网卡累计下载" value={bytes(data.traffic.downloaded)} note="服务器收到的观测流量" icon={ArrowDownIcon} />
        <Stat label="网卡双向合计" value={bytes(data.traffic.total)} note={data.traffic.incomplete ? '存在采样缺失或计数重置' : '累计已收到的网卡观测'} icon={ActivityIcon} />
      </div>
      <div className="grid gap-5 xl:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]"><TrafficChart title="网卡流量趋势" points={data.points} /><Ranking title="服务器流量排行" rows={data.by_server} link={row => `#/servers/${row.id}`} /></div>
      <p data-slot="statistics-scope" className="text-muted-foreground mt-3.5 mb-6 text-xs leading-relaxed wrap-anywhere">仅统计当前未删除服务器（含展示隐藏）。按当前选择的统计网卡汇总；未选择时包含所有上报网卡。这里使用原始观测，不含账单周期流量矫正。未上报、网卡变化和重启可能使数据不完整。{data.traffic.last_sample_at ? `最新采样：${time(data.traffic.last_sample_at / 1000)}。` : ''}</p>
    </>}
    <div className="mt-7 mb-4 flex items-center justify-between gap-4 max-sm:items-start"><div><h2 className="text-base font-semibold">代理业务统计</h2><p className="text-muted-foreground mt-1 text-xs">独立汇总代理用户账本，不能与服务器网卡流量相加。</p></div><a href="#/plugins/sing-box/nodes" className={buttonVariants({ variant: 'ghost', size: 'sm' })}>管理代理节点</a></div>
    <ErrorNotice message={proxy.error} retry={proxy.reload} />
    {proxy.loading && !business ? <Loading variant="cards" /> : business && <>
      <div className="mb-6 grid gap-3 max-sm:gap-2 grid-cols-2 lg:grid-cols-4">
        <Stat label="代理节点" value={business.nodes} note="未删除服务器上的节点" icon={WaypointsIcon} />
        <Stat label="代理用户" value={business.users} note="当前未删除用户" icon={UsersIcon} />
        <Stat label={`近 ${days} 天代理流量`} value={bytes(business.traffic.total)} note={`${business.traffic.recorded_users} 位用户有账本记录`} icon={ActivityIcon} />
        <Stat label="有记录的代理节点" value={business.traffic.recorded_nodes} note="统计窗口内，包含历史节点" icon={WaypointsIcon} />
      </div>
      <TrafficChart title="代理流量趋势" points={business.points} />
      <div className="grid gap-5 xl:grid-cols-2"><Ranking title="代理用户流量排行" rows={business.by_user} /><Ranking title="代理节点流量排行" rows={business.by_node} /></div>
      <p data-slot="statistics-scope" className="text-muted-foreground mt-3.5 mb-6 text-xs leading-relaxed wrap-anywhere">上传和下载以代理用户为视角；按计量批次结束时间归入 UTC 日期，不按跨日时长拆分。保留已删除用户、节点的历史记录；离线补报会更新历史，暂无记录不代表实际零流量。{business.traffic.last_record_at ? `最新计量：${time(business.traffic.last_record_at)}。` : ''}{proxy.error ? ' 当前显示上次成功读取的数据。' : ''}</p>
    </>}
  </div>
}
