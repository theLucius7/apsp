import { useMemo, useState } from 'react'
import type { Metrics, Server } from '../types'
import Chart from './Chart'
import { fresh, network, number, percentage, sampleGap, size, speed } from './data'
import { aggregatePoints, appendLive, duration, historyPollInterval, historyWindows, windowMinutes } from './history'
import type { HistoryWindow } from './history'
import { useHistory } from './useHistory'
import { ActivityIcon } from 'lucide-react'
import { DisplayNotice, SectionHeading } from './ui'
import { SegmentedButton, SegmentedGroup } from '@/components/segmented'

const diskSpeed = (metrics: Metrics, field: 'read_bytes_per_sec' | 'write_bytes_per_sec') => {
  const values = metrics.disks?.map(disk => number(disk[field])) ?? []
  return values.length && values.every(value => value !== null) ? values.reduce<number>((total, value) => total + value!, 0) : null
}
const loadFormat = (value: number | null) => value === null ? '—' : value.toFixed(2)

export default function ResourceCharts({ server, now, unavailable }: { server: Server; now: number; unavailable: boolean }) {
  const modern = Boolean(server.telemetry_settings)
  const [window, setWindow] = useState<HistoryWindow>('15m')
  const history = useHistory(server.id, window, modern)
  const aggregate = history.aggregate
  const samples = useMemo(() => {
    const byTime = new Map(history.samples.map(sample => [sample.sampled_at, sample]))
    if (!history.denied && server.metrics_sampled_at && server.metrics_sampled_at <= now) byTime.set(server.metrics_sampled_at, { id: 'latest', sampled_at: server.metrics_sampled_at, metrics: server.latest_metrics })
    return [...byTime.values()].filter(sample => sample.sampled_at >= now - windowMinutes(window) * 60_000).sort((a, b) => a.sampled_at - b.sampled_at)
  }, [history.samples, history.denied, server.metrics_sampled_at, server.latest_metrics, now, window])
  const props = { from: aggregate?.from ?? now - windowMinutes(window) * 60_000, to: Math.max(aggregate?.to ?? now, Math.min(server.metrics_sampled_at ?? 0, now)), gap: aggregate ? aggregate.bucket_ms * 1.5 : sampleGap(samples) }
  const series = (label: string, color: string, key: string, read: (metrics: Metrics) => number | null) => ({ label, color,
    points: modern ? aggregate ? appendLive(aggregatePoints(aggregate, key), aggregate, !unavailable && fresh(server) ? server.metrics_sampled_at : null, read(server.latest_metrics), now) : [] : samples.map(sample => ({ at: sample.sampled_at, value: read(sample.metrics) })) })
  return <section data-slot="resource-charts" className="flex flex-col gap-3">
    <SectionHeading icon={ActivityIcon} title="资源趋势"><SegmentedGroup aria-label="资源时间范围" className="max-[480px]:grid max-[480px]:grid-cols-3 max-sm:*:flex-1">{historyWindows.filter((_, index) => modern || index < 3).map(item => <SegmentedButton key={item.value} aria-pressed={window === item.value} onClick={() => setWindow(item.value)}>{item.label}</SegmentedButton>)}</SegmentedGroup></SectionHeading>
    {history.error && <DisplayNotice tone="error" onRetry={history.reload}>{history.error} 历史采样刷新失败。</DisplayNotice>}
    {history.loading && <p className="text-muted-foreground text-xs" role="status">正在读取历史采样…</p>}
    {aggregate && <p data-slot="history-resolution" className="text-muted-foreground m-0 flex flex-wrap gap-x-4 gap-y-1 text-xs">每 {duration(aggregate.bucket_ms)} 聚合 · {duration(historyPollInterval(aggregate.bucket_ms))} 刷新 · 历史保留 {aggregate.retention_days} 天<span>曲线为平均值，色带为最小值至最大值。末端实心点为最新实际采样。</span></p>}
    <div className="grid gap-3 md:grid-cols-2">
      <Chart {...props} title="处理器" maximum={100} format={percentage} series={[series('使用率', 'var(--destructive)', 'cpu_percent', metrics => number(metrics.cpu_percent))]} />
      <Chart {...props} title="内存" maximum={number(server.static_info.memory_total) ?? undefined} format={size} series={[series('已用内存', 'var(--success)', 'memory_used', metrics => number(metrics.memory_used))]} />
      <Chart {...props} title="磁盘" maximum={number(server.static_info.disk_total) ?? undefined} format={size} series={[series('已用磁盘', 'var(--warning)', 'disk_used', metrics => number(metrics.disk_used))]} />
      <Chart {...props} title="网络速率" format={speed} series={[series('上行', 'var(--success)', 'network_transmit_bytes_per_sec', metrics => network(metrics, 'transmit_bytes_per_sec', server.asset_settings?.network_interface)), series('下行', 'var(--info)', 'network_receive_bytes_per_sec', metrics => network(metrics, 'receive_bytes_per_sec', server.asset_settings?.network_interface))]} />
      {modern && <><Chart {...props} title="系统负载" format={loadFormat} series={(['load_1', 'load_5', 'load_15'] as const).map((key, index) => series(['1 分钟', '5 分钟', '15 分钟'][index], ['var(--success)', 'var(--info)', 'var(--warning)'][index], key, metrics => number(metrics[key])))} />
        <Chart {...props} title="磁盘读写速率" format={speed} series={[series('读取', 'var(--success)', 'disk_read_bytes_per_sec', metrics => diskSpeed(metrics, 'read_bytes_per_sec')), series('写入', 'var(--info)', 'disk_write_bytes_per_sec', metrics => diskSpeed(metrics, 'write_bytes_per_sec'))]} /></>}
    </div>
    <p className="text-muted-foreground my-1 text-xs leading-relaxed">鼠标移到曲线上查看采样，键盘可用左右方向键移动；点击图例可隐藏曲线。空缺不补零，缺少整个聚合窗口时以断线显示。{aggregate && '聚合样本数仅统计实际收到的采样，边界或旧历史不完整时单独标记；不会用网卡累计计数推算费用或周期用量。'}</p>
  </section>
}
