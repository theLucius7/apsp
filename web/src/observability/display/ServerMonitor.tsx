import type { ReactNode } from 'react'
import { ActivityIcon, ArrowDownIcon, ArrowUpIcon, ClockIcon, CpuIcon, DatabaseIcon, NetworkIcon, ServerIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { bytes, time, uptime } from '../../format'
import type { Server } from '../../types'
import { defaultAssets } from '../../resources/server-assets'
import { count, fresh, network, number, percentage, ratio, size, speed } from './data'
import ProbeCharts from './ProbeCharts'
import ResourceCharts from './ResourceCharts'
import { Meter } from '../../components'
import { surface } from '@/components/board'
import { Notice } from '@/components/panel'
import { cn } from '@/lib/utils'
import { ProgressRing } from '@/components/progress-ring'

// One monitoring panel for one server. The console's server page and the public
// board both show it, so a server never has two different "details" views; only
// the endpoints it reads differ (administrators read their own, visitors the
// board's filtered projection).

const levelTone = (value: number | null) => value === null ? 'muted' : value >= 90 ? 'danger' : value >= 75 ? 'warning' : 'good'

function Gauge({ label, value, main, detail }: { label: string; value: number | null; main: ReactNode; detail: ReactNode }) {
  // Phones stack the ring over its numbers so three gauges fit in one row.
  return <div data-slot="gauge" className={cn(surface, 'col-span-2 flex min-w-0 flex-col items-center gap-2 px-2 py-3 text-center sm:flex-row sm:gap-3 sm:px-3.5 sm:text-left xl:col-span-1')}>
    {/* Colour follows the same thresholds as the cards. */}
    <ProgressRing value={value} tone={levelTone(value)} label={`${label} ${percentage(value)}`} className="size-12 sm:size-14"><span className="text-[11px] font-semibold">{value === null ? '—' : `${Math.round(value)}%`}</span></ProgressRing>
    <div className="grid w-full min-w-0 gap-0.5">
      <span className="text-muted-foreground text-xs">{label}</span>
      <strong className="truncate text-base leading-tight font-semibold tabular-nums">{main}</strong>
      <small className="text-muted-foreground truncate text-xs tabular-nums">{detail}</small>
    </div>
  </div>
}

function InfoGroup({ title, icon: Icon, items }: { title: string; icon: LucideIcon; items: [string, ReactNode][] }) {
  return <section data-slot="info-group" className={cn(surface, 'min-w-0 p-4')}>
    <h2 className="flex items-center gap-2 text-sm font-semibold"><Icon className="text-primary size-4" aria-hidden="true" />{title}</h2>
    <dl className="mt-1.5 grid grid-cols-2 gap-x-4 [&>div]:min-w-0 [&>div]:py-2 [&>div:nth-child(n+3)]:border-t">
      {items.map(([label, value]) => <div key={label}><dt className="text-muted-foreground text-xs">{label}</dt><dd className="mt-0.5 text-[13px] font-medium wrap-anywhere">{value ?? '—'}</dd></div>)}
    </dl>
  </section>
}

function DataTable({ icon: Icon, title, children }: { icon: LucideIcon; title: string; children: ReactNode }) {
  return <section data-slot="table-section" className={cn(surface, 'min-w-0 overflow-hidden')}>
    <h2 className="flex items-center gap-2 px-4 py-3 text-sm font-semibold"><Icon className="text-primary size-4" aria-hidden="true" />{title}</h2>
    {children}
  </section>
}

const table = 'w-full border-collapse text-[13px] whitespace-nowrap tabular-nums [&_td]:border-t [&_td]:px-4 [&_td]:py-2.5 [&_td]:text-left [&_th]:border-t [&_th]:px-4 [&_th]:py-2.5 [&_th]:text-left [&_th]:font-normal [&_thead]:bg-muted/50 [&_thead]:text-muted-foreground [&_thead]:text-xs [&_small]:text-muted-foreground [&_small]:block [&_small]:text-xs'

export type MonitorProps = {
  server: Server
  /** Where this server's history and probes are read: the console's or the board's endpoints. */
  base: string
  now: number
  /** The snapshot itself could not be confirmed recently. */
  unavailable: boolean
  /** Settings for the agent intervals are known (newer agents). */
  modern: boolean
  /** Where an administrator configures this server's probes; absent on the public board. */
  probeSettings?: string
}

export default function ServerMonitor({ server, base, now, unavailable, modern, probeSettings }: MonitorProps) {
  // A server that never reported has no readings yet; every figure then shows a dash.
  const info = server.static_info ?? {}, metrics = server.latest_metrics ?? {}
  const asset = { ...defaultAssets, ...server.asset_settings }
  const live = fresh(server) && !unavailable
  const traffic = server.traffic, quota = asset.traffic_limit !== '0'
  const trafficKnown = traffic?.observed_from != null || Boolean(traffic?.corrected)
  const load = [metrics.load_1, metrics.load_5, metrics.load_15].map(value => number(value) === null ? '—' : value!.toFixed(2)).join(' / ')
  const disks = metrics.disks ?? [], gpus = metrics.gpus ?? [], interfaces = Object.entries(metrics.network_interfaces ?? {})
  return <div data-slot="server-monitor" className="flex min-w-0 flex-col gap-4">
    {!live && <Notice tone="warning" className="mb-0">{unavailable ? '超过 15 秒未收到新的服务器快照，状态待确认。' : !server.online ? (server.registered ?? Boolean(server.device_public_key)) ? '设备离线。' : '设备尚未接入。' : server.metrics_stale ? '指标已过期；在线心跳不代表指标仍在采集。' : '采样时间未知，无法确认指标是否仍然有效。'}以下为最后上报的数据，实时速率暂不显示。</Notice>}
    <section data-slot="live-strip" aria-label="实时状态" className="grid min-w-0 grid-cols-6 gap-2.5 sm:gap-3 xl:grid-cols-5">
      <Gauge label="处理器" value={number(metrics.cpu_percent)} main={percentage(metrics.cpu_percent)} detail={`${info.cpu_cores ? `${info.cpu_cores} 核 · ` : ''}负载 ${number(metrics.load_1) === null ? '—' : metrics.load_1!.toFixed(2)}`} />
      <Gauge label="内存" value={ratio(metrics.memory_used, info.memory_total)} main={size(metrics.memory_used)} detail={`共 ${size(info.memory_total)}`} />
      <Gauge label="磁盘" value={ratio(metrics.disk_used, info.disk_total)} main={size(metrics.disk_used)} detail={`共 ${size(info.disk_total)}`} />
      <div data-slot="live-network" className={cn(surface, 'col-span-3 grid min-w-0 content-center gap-1 px-3.5 py-3 xl:col-span-1')}>
        <span className="text-muted-foreground text-xs">实时网络</span>
        {([['上行', ArrowUpIcon, 'text-success', 'transmit_bytes_per_sec'], ['下行', ArrowDownIcon, 'text-info', 'receive_bytes_per_sec']] as const).map(([label, Icon, tone, field]) =>
          <div key={label} className="flex min-w-0 items-baseline justify-between gap-2"><small className="text-muted-foreground flex shrink-0 items-center gap-0.5 text-xs"><Icon className="size-3" aria-hidden="true" />{label}</small><strong className={cn('truncate text-[15px] leading-tight font-semibold tabular-nums', tone)}>{live ? speed(network(metrics, field, asset.network_interface)) : '—'}</strong></div>)}
        <small className="text-muted-foreground truncate text-xs tabular-nums" title="网卡累计上传 / 下载">累计 {size(network(metrics, 'transmitted_bytes', asset.network_interface))} / {size(network(metrics, 'received_bytes', asset.network_interface))}</small>
      </div>
      <div data-slot="live-traffic" className={cn(surface, 'col-span-3 grid min-w-0 content-center gap-1.5 px-3.5 py-3 xl:col-span-1')}>
        <span className="text-muted-foreground flex items-center justify-between gap-2 text-xs"><span>本期流量</span>{traffic?.exceeded ? <span className="text-destructive">额度已用尽</span> : traffic?.incomplete ? <span className="text-warning">观测不完整</span> : null}</span>
        <strong className="truncate text-base leading-tight font-semibold tabular-nums">{trafficKnown ? bytes(traffic?.used, '—') : '等待采样'}<small className="text-muted-foreground ml-1 text-xs font-normal">/ {quota ? bytes(asset.traffic_limit, '—') : '不限'}</small></strong>
        <Meter value={quota ? traffic?.percent : null} tone={!quota || traffic?.percent == null ? 'muted' : traffic.exceeded ? 'danger' : traffic.percent >= 80 ? 'warning' : 'good'} label={quota && traffic?.percent != null ? `流量额度已使用 ${traffic.percent.toFixed(1)}%` : undefined} className="mt-0 max-w-none" />
        <small className="text-muted-foreground truncate text-xs">每月 {asset.reset_day} 日重置{traffic?.remaining != null && quota ? ` · 剩余 ${bytes(traffic.remaining, '—')}` : ''}</small>
      </div>
    </section>
    <ResourceCharts key={`resources/${base}`} server={server} base={base} now={now} unavailable={unavailable} />
    <ProbeCharts key={`probes/${base}`} base={base} now={now} settings={probeSettings} unavailable={unavailable || !server.online} />
    <div data-slot="info-groups" className="grid gap-3 lg:grid-cols-2">
      <InfoGroup title="硬件信息" icon={CpuIcon} items={[
        ['处理器', info.cpu_model], ['核心 / 架构', `${count(info.cpu_cores)} 核 / ${info.arch ?? '—'}`],
        ['内存 / 磁盘', `${size(info.memory_total)} / ${size(info.disk_total)}`], ['虚拟化', info.virtualization],
      ]} />
      <InfoGroup title="系统信息" icon={ServerIcon} items={[
        ...(!server.public_view ? [['主机名', info.hostname] as [string, ReactNode]] : []), ['运行时间', metrics.uptime_secs === undefined ? undefined : uptime(metrics.uptime_secs)],
        ['操作系统', info.system], ['内核版本', info.kernel],
        ['交换内存', `${size(metrics.swap_used)} / ${size(metrics.swap_total)}`], ['负载（1 / 5 / 15 分钟）', load],
        ['进程 / 连接', `${count(metrics.processes)} / ${count(number(metrics.tcp_connections) !== null && number(metrics.udp_connections) !== null ? metrics.tcp_connections! + metrics.udp_connections! : null)}`],
      ]} />
    </div>
    {(interfaces.length > 0 || disks.length > 0) && <div className="grid min-w-0 gap-3 xl:grid-cols-2">
      <DataTable icon={NetworkIcon} title="网络接口">{interfaces.length ? <div className="max-w-full overflow-auto"><table className={table}><thead><tr><th>接口</th><th>累计上传 / 下载</th><th>上行 / 下行</th></tr></thead><tbody>{interfaces.map(([name, metric]) => <tr key={name}><th scope="row" className="font-mono text-xs">{name}</th><td>{size(metric.transmitted_bytes)} / {size(metric.received_bytes)}</td><td><span className="text-success">{live ? speed(metric.transmit_bytes_per_sec) : '—'}</span> / <span className="text-info">{live ? speed(metric.receive_bytes_per_sec) : '—'}</span></td></tr>)}</tbody></table></div> : <p className="text-muted-foreground px-4 pb-4 text-sm">设备尚未上报网卡数据。</p>}</DataTable>
      {disks.length > 0 && <DataTable icon={DatabaseIcon} title="磁盘读写"><div className="max-w-full overflow-auto"><table className={table}><thead><tr><th>磁盘 / 挂载点</th><th>已用 / 容量</th><th>读取 / 写入</th><th>等待 / 利用率</th></tr></thead><tbody>{disks.map((disk, index) => <tr key={`${disk.name}-${index}`}><th scope="row">{disk.name}<small>{disk.mount_point}</small></th><td>{size(disk.used_bytes)} / {size(disk.total_bytes)}</td><td>{speed(disk.read_bytes_per_sec)} / {speed(disk.write_bytes_per_sec)}<small>{count(disk.read_iops)} / {count(disk.write_iops)} 次每秒</small></td><td>{number(disk.await_ms) === null ? '—' : `${disk.await_ms!.toFixed(1)} ms`} / {percentage(disk.utilization_percent)}</td></tr>)}</tbody></table></div></DataTable>}
    </div>}
    {gpus.length > 0 && <DataTable icon={CpuIcon} title="图形处理器"><div className="max-w-full overflow-auto"><table className={table}><thead><tr><th>型号</th><th>使用率</th><th>已用显存 / 总显存</th></tr></thead><tbody>{gpus.map((gpu, index) => <tr key={index}><th scope="row">{gpu.model}</th><td>{percentage(gpu.usage_percent)}</td><td>{size(gpu.memory_used)} / {size(gpu.memory_total)}</td></tr>)}</tbody></table></div></DataTable>}
    <div data-slot="info-groups" className="grid gap-3 lg:grid-cols-2"><InfoGroup title="设备状态" icon={ActivityIcon} items={[
      // The proxy runtime's version belongs to the sing-box card: a reported version does not mean it is installed.
      ['设备版本', info.agent_version], ['最近设备消息', time(server.last_seen)], ['最后心跳', time(server.last_heartbeat_at)],
    ]} /><InfoGroup title="采样信息" icon={ClockIcon} items={[
      ['最后采样', server.metrics_sampled_at ? time(server.metrics_sampled_at / 1000) : '时间未知'],
      ...(modern ? [
        ['最近状态接收', server.metrics_received_at ? time(server.metrics_received_at / 1000) : '暂无实时接收时间'],
        ['最近已存采样', server.metrics_persisted_at ? time(server.metrics_persisted_at / 1000) : '暂无持久采样'],
        ['采集 / 状态上报', server.agent_settings ? `${server.agent_settings.sample_interval_secs} 秒 / ${server.agent_settings.upload_interval_secs} 秒` : '由设备配置'],
        ['历史写入配置', server.telemetry_settings ? `${server.telemetry_settings.persist_interval_secs} 秒` : '由设备配置'],
      ] as [string, ReactNode][] : []),
      ['TCP / UDP 连接', `${count(metrics.tcp_connections)} / ${count(metrics.udp_connections)}`],
    ]} /></div>
  </div>
}
