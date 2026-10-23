import type { ReactNode } from 'react'
import { ActivityIcon, ArrowLeftIcon, ClockIcon, CpuIcon, DatabaseIcon, NetworkIcon, RefreshCwIcon, ServerIcon } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { time, uptime } from '../format'
import { count, fresh, network, number, percentage, size, speed, status } from './data'
import ProbeCharts from './ProbeCharts'
import { OSIcon } from './Icon'
import { useDashboardServers } from './useDashboardServers'
import ResourceCharts from './ResourceCharts'
import AssetInfo from './AssetInfo'
import { snapshotUnavailable } from './dashboard'
import { DisplayNotice, StatusPill, surface } from './ui'
import { cn } from '@/lib/utils'
import { Button, buttonVariants } from '@/components/ui/button'
import { Spinner } from '@/components/ui/spinner'

function InfoGroup({ title, icon: Icon, items }: { title: string; icon: LucideIcon; items: [string, ReactNode][] }) {
  return <section data-slot="info-group" className={cn(surface, 'min-w-0 p-4')}>
    <h2 className="flex items-center gap-2 text-sm font-semibold"><Icon className="text-primary size-4" aria-hidden="true" />{title}</h2>
    <dl className="mt-1.5 grid grid-cols-2 gap-x-4 [&>div]:min-w-0 [&>div]:py-2.5 [&>div:nth-child(n+3)]:border-t">
      {items.map(([label, value]) => <div key={label}><dt className="text-muted-foreground text-xs">{label}</dt><dd className="mt-0.5 text-sm font-medium wrap-anywhere">{value ?? '—'}</dd></div>)}
    </dl>
  </section>
}

function DataTable({ icon: Icon, title, children }: { icon: LucideIcon; title: string; children: ReactNode }) {
  return <section data-slot="table-section" className={cn(surface, 'min-w-0 overflow-hidden')}>
    <h2 className="flex items-center gap-2 p-4 text-sm font-semibold"><Icon className="text-primary size-4" aria-hidden="true" />{title}</h2>
    {children}
  </section>
}

const table = 'w-full border-collapse text-sm whitespace-nowrap tabular-nums [&_td]:border-t [&_td]:px-4 [&_td]:py-3 [&_td]:text-left [&_th]:border-t [&_th]:px-4 [&_th]:py-3 [&_th]:text-left [&_th]:font-normal [&_thead]:bg-muted/50 [&_thead]:text-muted-foreground [&_thead]:text-xs [&_small]:text-muted-foreground [&_small]:block [&_small]:text-xs'

export default function ServerView({ id, now }: { id: number; now: number }) {
  const resource = useDashboardServers(false, id)
  const server = resource.data?.[0]
  if (!server) return <>
    <a className={cn(buttonVariants(), 'mb-4')} href="#/dashboard"><ArrowLeftIcon aria-hidden="true" />返回看板</a>
    {resource.error ? <DisplayNotice tone="error" onRetry={resource.reload}>{resource.error}</DisplayNotice> : <div className="text-muted-foreground flex min-h-80 items-center justify-center gap-2 text-sm" role="status"><Spinner role="presentation" aria-hidden="true" aria-label={undefined} />正在读取服务器…</div>}
  </>
  const unavailable = snapshotUnavailable(resource.updatedAt, now, false, resource.error)
  const info = server.static_info, metrics = server.latest_metrics, state = status(server, unavailable)
  const live = fresh(server) && !unavailable
  return <div data-slot="server-detail" className="flex flex-col gap-4">
    <section data-slot="detail-hero" className={cn(surface, 'flex min-w-0 items-center gap-3 px-5 py-3.5 max-sm:gap-2 max-sm:px-3')}>
      <a href="#/dashboard" className={buttonVariants({ variant: 'ghost', size: 'icon' })} aria-label="返回服务器看板"><ArrowLeftIcon /></a>
      <div className="min-w-0 flex-1">
        <h1 className="flex flex-wrap items-center gap-2 text-xl font-semibold wrap-anywhere max-sm:text-lg">{server.name}<StatusPill tone={state.tone}>{state.label}</StatusPill></h1>
        <p className="text-muted-foreground mt-1 flex flex-wrap items-center gap-1.5 text-sm"><OSIcon system={info.system} />{info.system ?? '系统尚未上报'}<span>·</span>{info.arch ?? '架构未知'}</p>
      </div>
      <Button variant="ghost" size="icon" aria-label="刷新服务器详情" onClick={resource.reload}><RefreshCwIcon /></Button>
    </section>
    {resource.error && <DisplayNotice tone="error">{resource.error} 以下保留最近一次读取的信息。</DisplayNotice>}
    {!live && <DisplayNotice>{resource.error ? '暂时无法读取最新设备状态。' : unavailable ? '超过 15 秒未收到新的服务器快照，状态待确认。' : !server.online ? '设备离线或尚未接入。' : server.metrics_stale ? '指标已过期；在线心跳不代表指标仍在采集。' : '采样时间未知，无法确认指标是否仍然有效。'}以下为最后上报的数据，实时速率暂不显示。</DisplayNotice>}
    <div data-slot="info-groups" className="grid gap-3 lg:grid-cols-2">
      <InfoGroup title="硬件信息" icon={CpuIcon} items={[
        ['处理器', info.cpu_model], ['核心 / 架构', `${count(info.cpu_cores)} 核 / ${info.arch ?? '—'}`],
        ['内存 / 磁盘', `${size(info.memory_total)} / ${size(info.disk_total)}`], ['虚拟化', info.virtualization],
      ]} />
      <InfoGroup title="系统信息" icon={ServerIcon} items={[
        ...(!server.public_view ? [['主机名', info.hostname] as [string, ReactNode]] : []), ['运行时间', metrics.uptime_secs === undefined ? undefined : uptime(metrics.uptime_secs)],
        ['内核版本', info.kernel], ['进程 / 连接', `${count(metrics.processes)} / ${count(number(metrics.tcp_connections) !== null && number(metrics.udp_connections) !== null ? metrics.tcp_connections! + metrics.udp_connections! : null)}`],
      ]} />
    </div>
    <div data-slot="live-strip" className={cn(surface, 'grid grid-cols-2 md:grid-cols-4 [&>div]:min-w-0 [&>div]:px-5 [&>div]:py-3.5 max-sm:[&>div]:px-3.5 max-md:[&>div:nth-child(even)]:border-l max-md:[&>div:nth-child(-n+2)]:border-b md:[&>div+div]:border-l [&_span]:text-muted-foreground [&_span]:mb-1 [&_span]:block [&_span]:text-xs [&_strong]:text-lg [&_strong]:font-semibold [&_strong]:tabular-nums [&_strong]:wrap-anywhere')}>
      <div><span>处理器</span><strong>{percentage(metrics.cpu_percent)}</strong></div>
      <div><span>已用内存</span><strong>{size(metrics.memory_used)}</strong></div>
      <div><span>实时上行</span><strong className="text-success">{live ? speed(network(metrics, 'transmit_bytes_per_sec', server.asset_settings?.network_interface)) : '—'}</strong></div>
      <div><span>实时下行</span><strong className="text-info">{live ? speed(network(metrics, 'receive_bytes_per_sec', server.asset_settings?.network_interface)) : '—'}</strong></div>
    </div>
    <ResourceCharts key={`resources/${id}/${Boolean(server.public_view)}`} server={server} now={now} unavailable={unavailable} />
    <AssetInfo server={server} now={now} detail />
    <ProbeCharts key={`probes/${id}/${Boolean(server.public_view)}`} id={id} now={now} publicView={Boolean(server.public_view)} unavailable={unavailable || !server.online} />
    <DataTable icon={NetworkIcon} title="网络接口">{Object.keys(metrics.network_interfaces ?? {}).length ? <div className="max-w-full overflow-auto"><table className={table}><thead><tr><th>接口</th><th>累计上传</th><th>累计下载</th><th>上行速率</th><th>下行速率</th></tr></thead><tbody>{Object.entries(metrics.network_interfaces ?? {}).map(([name, metric]) => <tr key={name}><th scope="row">{name}</th><td>{size(metric.transmitted_bytes)}</td><td>{size(metric.received_bytes)}</td><td className="text-success">{live ? speed(metric.transmit_bytes_per_sec) : '—'}</td><td className="text-info">{live ? speed(metric.receive_bytes_per_sec) : '—'}</td></tr>)}</tbody></table></div> : <p className="text-muted-foreground px-4 pb-4 text-sm">设备尚未上报网卡数据。</p>}</DataTable>
    {metrics.disks?.length ? <DataTable icon={DatabaseIcon} title="磁盘读写"><div className="max-w-full overflow-auto"><table className={table}><thead><tr><th>磁盘 / 挂载点</th><th>已用 / 容量</th><th>读取 / 写入速率</th><th>读 / 写操作数（每秒）</th><th>等待 / 利用率</th></tr></thead><tbody>{metrics.disks.map((disk, index) => <tr key={`${disk.name}-${index}`}><th scope="row">{disk.name}<small>{disk.mount_point}</small></th><td>{size(disk.used_bytes)} / {size(disk.total_bytes)}</td><td>{speed(disk.read_bytes_per_sec)} / {speed(disk.write_bytes_per_sec)}</td><td>{count(disk.read_iops)} / {count(disk.write_iops)}</td><td>{number(disk.await_ms) === null ? '—' : `${disk.await_ms!.toFixed(1)} ms`} / {percentage(disk.utilization_percent)}</td></tr>)}</tbody></table></div></DataTable> : null}
    {metrics.gpus?.length ? <DataTable icon={CpuIcon} title="图形处理器"><div className="max-w-full overflow-auto"><table className={table}><thead><tr><th>型号</th><th>使用率</th><th>已用显存 / 总显存</th></tr></thead><tbody>{metrics.gpus.map((gpu, index) => <tr key={index}><th scope="row">{gpu.model}</th><td>{percentage(gpu.usage_percent)}</td><td>{size(gpu.memory_used)} / {size(gpu.memory_total)}</td></tr>)}</tbody></table></div></DataTable> : null}
    <div data-slot="info-groups" className="grid gap-3 lg:grid-cols-2"><InfoGroup title="设备状态" icon={ActivityIcon} items={[
      ['设备版本', info.agent_version], ['运行时版本', info.runtime_version], ['最近设备消息', time(server.last_seen)], ['最后心跳', time(server.last_heartbeat_at)],
    ]} /><InfoGroup title="采样信息" icon={ClockIcon} items={[
      ['最后采样', server.metrics_sampled_at ? time(server.metrics_sampled_at / 1000) : '时间未知'],
      ...(resource.modern ? [
        ['最近状态接收', server.metrics_received_at ? time(server.metrics_received_at / 1000) : '暂无实时接收时间'],
        ['最近已存采样', server.metrics_persisted_at ? time(server.metrics_persisted_at / 1000) : '暂无持久采样'],
        ['采集 / 状态上报', server.agent_settings ? `${server.agent_settings.sample_interval_secs} 秒 / ${server.agent_settings.upload_interval_secs} 秒` : '由设备配置'],
        ['历史写入配置', server.telemetry_settings ? `${server.telemetry_settings.persist_interval_secs} 秒` : '由设备配置'],
      ] as [string, ReactNode][] : []),
      ['交换内存', `${size(metrics.swap_used)} / ${size(metrics.swap_total)}`], ['TCP / UDP 连接', `${count(metrics.tcp_connections)} / ${count(metrics.udp_connections)}`], ['系统负载（1 / 5 / 15 分钟）', [metrics.load_1, metrics.load_5, metrics.load_15].map(value => number(value) === null ? '—' : value!.toFixed(2)).join(' / ')],
    ]} /></div>
  </div>
}
