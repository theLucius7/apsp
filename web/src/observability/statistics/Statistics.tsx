import { useState } from 'react'
import { ErrorNotice, InfoTip, Loading, PageHeader, Refresh } from '../../components'
import { Stat, StatStrip } from '../display/ui'
import { bytes, time } from '../../format'
import { useResource } from '../../hooks'
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
    <PageHeader title="统计报表">
      <SegmentedGroup role="group" aria-label="统计时间范围">{[7, 30].map(value => <SegmentedButton key={value} type="button" aria-pressed={days === value} onClick={() => setDays(value)}>近 {value} 天</SegmentedButton>)}</SegmentedGroup>
      <Refresh onClick={() => { servers.reload(); proxy.reload() }} />
    </PageHeader>
    <ErrorNotice message={servers.error} retry={servers.reload} />
    {servers.loading && !data ? <Loading variant="cards" /> : data && <>
      {servers.error && <p data-slot="statistics-status-line" className="text-warning mb-3 text-xs">显示上次成功读取的数据 · {time(data.generated_at)}</p>}
      <StatStrip label="服务器概况" columns={4}>
        <Stat icon={ServerIcon} label="服务器总数" value={data.servers.total} />
        <Stat icon={ActivityIcon} label="在线服务器" value={data.servers.online} />
        <Stat icon={ServerIcon} label="离线服务器" value={data.servers.offline} />
        <Stat icon={PlusIcon} label="待接入服务器" value={data.servers.pending} />
      </StatStrip>
      <div className="mt-6 mb-3 flex items-center justify-between gap-4"><span className="flex min-w-0 items-center gap-1.5"><h2 className="flex items-center gap-1.5 text-sm font-semibold">服务器网卡流量
        
        {data.traffic.sampled_servers < data.servers.total && <span className="text-warning text-xs font-normal">覆盖 {data.traffic.sampled_servers} / {data.servers.total} 台</span>}</h2><InfoTip label="统计口径"><p data-slot="statistics-scope">近 {days} 个 UTC 自然日。仅统计当前未删除的服务器（含展示隐藏），按所选统计网卡汇总，未选择时包含所有上报网卡。使用原始观测，不含账单周期流量矫正；未上报、网卡变化和重启可能使数据不完整。{data.traffic.last_sample_at ? `最新采样：${time(data.traffic.last_sample_at / 1000)}。` : ''}</p></InfoTip></span>
        <a href="#/servers" className={buttonVariants({ variant: 'ghost', size: 'sm' })}>管理服务器</a></div>
      <div data-slot="statistics-traffic-totals" className="mb-6"><StatStrip label="网卡流量合计" columns={3}>
        <Stat icon={ArrowUpIcon} label="网卡累计上传" value={bytes(data.traffic.uploaded)} />
        <Stat icon={ArrowDownIcon} label="网卡累计下载" value={bytes(data.traffic.downloaded)} />
        <Stat icon={ActivityIcon} label="网卡双向合计" value={bytes(data.traffic.total)}>{data.traffic.incomplete && <span className="text-warning">存在采样缺失或计数重置</span>}</Stat>
      </StatStrip></div>
      <div className="grid gap-5 xl:grid-cols-[minmax(0,1.35fr)_minmax(0,1fr)]"><TrafficChart title="网卡流量趋势" points={data.points} /><Ranking title="服务器流量排行" rows={data.by_server} link={row => `#/servers/${row.id}`} /></div>
    </>}
    <div className="mt-6 mb-3 flex items-center justify-between gap-4"><span className="flex min-w-0 items-center gap-1.5"><h2 className="flex items-center gap-1.5 text-sm font-semibold">代理业务统计
      </h2><InfoTip label="统计口径"><p>独立汇总代理用户账本，不能与服务器网卡流量相加。上传和下载以代理用户为视角，按计量批次结束时间归入 UTC 日期，不按跨日时长拆分；保留已删除用户与节点的历史，离线补报会更新历史，暂无记录不代表实际零流量。{business?.traffic.last_record_at ? `最新计量：${time(business.traffic.last_record_at)}。` : ''}</p></InfoTip></span>
      <a href="#/nodes" className={buttonVariants({ variant: 'ghost', size: 'sm' })}>管理代理节点</a></div>
    <ErrorNotice message={proxy.error} retry={proxy.reload} />
    {proxy.loading && !business ? <Loading variant="cards" /> : business && <>
      <StatStrip label="代理业务概况" columns={4}>
        <Stat icon={WaypointsIcon} label="代理节点" value={business.nodes} />
        <Stat icon={UsersIcon} label="代理用户" value={business.users} />
        <Stat icon={ActivityIcon} label={`近 ${days} 天代理流量`} value={bytes(business.traffic.total)}>{`${business.traffic.recorded_users} 位用户有账本记录`}</Stat>
        <Stat icon={WaypointsIcon} label="有记录的代理节点" value={business.traffic.recorded_nodes} />
      </StatStrip>
      <TrafficChart title="代理流量趋势" points={business.points} />
      <div className="grid gap-5 xl:grid-cols-2"><Ranking title="代理用户流量排行" rows={business.by_user} /><Ranking title="代理节点流量排行" rows={business.by_node} /></div>
    </>}
  </div>
}
