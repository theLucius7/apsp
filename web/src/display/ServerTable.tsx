import type { ProbeOverview } from '../probes'
import type { Server } from '../types'
import { uptime } from '../format'
import { AssetChips } from './AssetInfo'
import { dashboardServer } from './dashboard'
import { fresh, network, number, ratio, size, speed, status } from './data'
import { OSIcon } from './Icon'
import ProbeQuality from './ProbeQuality'
import { Metric } from './ServerCard'
import { StatusPill, surface } from './ui'
import { cn } from '@/lib/utils'

export default function ServerTable({ servers, unavailable, byServer, probesKnown, probeError, probeLoading, now }: {
  servers: Server[]; unavailable: boolean; byServer: Map<number, ProbeOverview[]>; probesKnown: boolean;
  probeError: boolean; probeLoading: boolean; now: number;
}) {
  return <section data-slot="fleet" className={cn(surface, 'min-w-0 overflow-hidden')} aria-label="服务器列表">
    <div className="max-h-[75dvh] overflow-auto outline-offset-[-3px] focus-visible:outline-2 focus-visible:outline-ring max-sm:max-h-none" tabIndex={0} role="region" aria-label="服务器表格，可横向滚动">
      <table className="w-full min-w-[1180px] border-separate border-spacing-0 text-sm [&_td]:border-b [&_td]:p-4 [&_td]:align-middle [&_th]:border-b [&_th]:p-4 [&_th]:text-left [&_th]:align-middle [&_tbody_tr:last-child>*]:border-b-0 [&_tbody_tr:hover>*]:bg-muted/40 [&_td>small]:text-muted-foreground [&_td>small]:mt-1 [&_td>small]:block [&_td>small]:text-xs [&_td>span]:block [&_td>span]:whitespace-nowrap">
        <caption className="sr-only">服务器资源与网络状态。数据不可用时保留历史值，实时速率显示为空缺。</caption>
        <thead className="[&_th]:bg-card [&_th]:text-muted-foreground [&_th]:sticky [&_th]:top-0 [&_th]:z-[2] [&_th]:text-xs [&_th]:font-medium [&_th]:whitespace-nowrap"><tr><th scope="col">服务器</th><th scope="col">状态</th><th scope="col">处理器</th><th scope="col">内存</th><th scope="col">磁盘</th><th scope="col">实时上下行</th><th scope="col">网络质量</th><th scope="col">运行与采样</th></tr></thead>
        <tbody>{servers.map(server => {
          const metrics = server.latest_metrics, info = server.static_info
          const live = fresh(server) && !unavailable, state = status(server, unavailable)
          const historical = !live ? '（最近上报）' : ''
          return <tr key={server.id} data-server-id={server.id} className={cn(!live && '[&_[data-slot=metric]_span>span]:opacity-45')}>
            <th scope="row" className="max-w-64 min-w-48 font-medium">
              <a className="hover:text-primary mb-1 flex items-center gap-2" href={dashboardServer(server.id)}><OSIcon system={info.system} /><strong className="text-sm font-medium wrap-anywhere">{server.name}</strong></a>
              <small className="text-muted-foreground block text-xs font-normal">{info.system || '系统尚未上报'}{info.cpu_cores ? ` · ${info.cpu_cores} 核` : ''}</small>
              <AssetChips server={server} className="mt-2 mb-0" />
            </th>
            <td className="min-w-32"><StatusPill tone={state.tone}>{state.label}</StatusPill>{!live && <small>{unavailable ? '等待刷新' : server.online ? server.metrics_stale ? '指标已过期' : '采样时间未知' : '历史数据'}</small>}</td>
            <td className="min-w-32"><Metric label={`处理器${historical}`} value={number(metrics.cpu_percent)} detail={`负载 ${number(metrics.load_1) === null ? '—' : metrics.load_1!.toFixed(2)}`} /></td>
            <td className="min-w-32"><Metric label={`内存${historical}`} value={ratio(metrics.memory_used, info.memory_total)} detail={`${size(metrics.memory_used)} / ${size(info.memory_total)}`} /></td>
            <td className="min-w-32"><Metric label={`磁盘${historical}`} value={ratio(metrics.disk_used, info.disk_total)} detail={`${size(metrics.disk_used)} / ${size(info.disk_total)}`} /></td>
            <td className="min-w-32 tabular-nums"><span className="text-success">↑ {live ? speed(network(metrics, 'transmit_bytes_per_sec', server.asset_settings?.network_interface)) : '—'}</span><span className="text-info">↓ {live ? speed(network(metrics, 'receive_bytes_per_sec', server.asset_settings?.network_interface)) : '—'}</span></td>
            <td className="min-w-56"><ProbeQuality probes={probesKnown ? byServer.get(server.id) ?? [] : undefined} now={now} online={server.online} unavailable={probeError || unavailable} loading={probeLoading} compact /></td>
            <td className="min-w-32"><span>{number(metrics.uptime_secs) === null ? '—' : uptime(metrics.uptime_secs)}</span><small>{server.metrics_sampled_at ? new Date(server.metrics_sampled_at).toLocaleTimeString('zh-CN', { hour12: false }) : '无采样时间'}</small><a className="text-primary mt-1.5 block text-xs hover:underline" href={dashboardServer(server.id)} aria-label={`查看 ${server.name} 详情`}>查看详情 →</a></td>
          </tr>
        })}</tbody>
      </table>
    </div>
  </section>
}
