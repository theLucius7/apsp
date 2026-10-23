import { useState } from 'react'
import { AgentSettings } from './AgentSettings'
import AgentTasks from './AgentTasks'
import { Badge, ErrorNotice, Icon, Loading, PageHeader, Refresh, Stat } from '../components'
import { bytes, navigate, percent, time, uptime } from '../format'
import { useResource } from '../hooks'
import type { Server } from '../types'
import ServerNavigation from './ServerNavigation'
import ServerEnrollment from './ServerEnrollment'
import ServerEdit from './ServerEdit'
import ServerAssets from './ServerAssets'
import { PluginSettings, ServerPlugins } from '../plugins'

export type ServerDetailSection = 'overview' | 'assets' | 'agent' | 'plugins'

const descriptions: Record<ServerDetailSection, string> = {
  overview: '这台服务器的运行指标、系统信息和网络接口。',
  assets: '地区、成本、到期与网卡流量额度；流量提醒按这里的账单周期计算。',
  agent: 'Agent 的采集与上报设置、这台服务器发起的周期拨测，以及远程命令。',
  plugins: '在这台服务器上启用插件，并查看已启用插件的部署与运行状态。',
}

export default function ServerDetail({ id, section = 'overview' }: { id: number; section?: ServerDetailSection }) {
  const server = useResource<Server>(`/api/servers/${id}`)
  const [pluginRevision, setPluginRevision] = useState(0)
  const [installing, setInstalling] = useState(false)
  const [editing, setEditing] = useState(false)
  const refresh = () => { server.reload(); setPluginRevision(value => value + 1) }
  const entry = server.data
  const back = <button className="back-link" onClick={() => navigate('/servers')}><Icon name="back" size={16} />返回服务器列表</button>
  if (!entry) return <>
    {back}
    <ErrorNotice message={server.error} retry={server.reload} />
    {server.loading && <Loading />}
    {/* Plugin enablement only needs the server ID, so it stays usable while the server read fails. */}
    {section === 'plugins' && !server.loading && <PluginSettings key={`${id}-${pluginRevision}`} serverId={id} />}
  </>
  return <>
    {back}
    <PageHeader eyebrow={`服务器 #${entry.id}`} title={entry.name} description={descriptions[section]}>
      <Badge tone={entry.online ? 'good' : 'neutral'}>{entry.online ? '在线' : entry.device_public_key ? '离线' : '待接入'}</Badge>
      {entry.metrics_stale && <Badge tone="warm">指标过期</Badge>}
      <Refresh onClick={refresh} />
      <button className="button button-primary" onClick={() => setInstalling(true)}><Icon name="plus" size={16} />接入 / 升级</button>
    </PageHeader>
    <ServerNavigation id={id} active={section} />
    <ErrorNotice message={server.error} retry={refresh} />
    {section === 'overview' && <Overview entry={entry} />}
    {section === 'assets' && <>
      <ServerAssets onReload={server.reload} server={entry} onEdit={() => setEditing(true)} />
      <p className="helper">供应商与订单、采购配置、续费与年度成本账本，以及维护状态在「运维工具 → 资产与生命周期」中记录；带有效期的续费记录会更新这里的到期日期。 <a className="text-button" href={`#/servers/${id}/fleet`}>打开运维工具</a></p>
    </>}
    {section === 'agent' && <>
      <AgentSettings serverId={id} liveSupported={Array.isArray(entry.capabilities) && entry.capabilities.includes('telemetry:live:v1')} />
      <AgentTasks key={id} serverId={id} commandsEnabled={Array.isArray(entry.capabilities) && entry.capabilities.includes('command:execute')} />
    </>}
    {section === 'plugins' && <>
      <PluginSettings key={`${id}-${pluginRevision}`} serverId={id} />
      <ServerPlugins key={`business-${id}-${pluginRevision}`} server={entry} />
    </>}
    {editing && <ServerEdit key={id} server={entry} onClose={() => setEditing(false)} onSaved={() => { setEditing(false); server.reload() }} />}
    {installing && <ServerEnrollment key={id} server={entry} onClose={() => { setInstalling(false); server.reload() }} />}
  </>
}

function Overview({ entry }: { entry: Server }) {
  const metrics = entry.latest_metrics, info = entry.static_info
  const rows = [['操作系统', info.system], ['内核版本', info.kernel], ['系统架构', info.arch], ['主机名', info.hostname], ['处理器型号', info.cpu_model], ['核心数', info.cpu_cores], ['总内存', info.memory_total === undefined ? undefined : bytes(info.memory_total)], ['磁盘容量', info.disk_total === undefined ? undefined : bytes(info.disk_total)], ['虚拟化', info.virtualization], ['Agent 版本', info.agent_version], ['最近设备消息', entry.last_seen ? time(entry.last_seen) : undefined], ['最后心跳', entry.last_heartbeat_at ? time(entry.last_heartbeat_at) : '尚未记录'], ['最后指标', entry.metrics_sampled_at ? time(entry.metrics_sampled_at / 1000) : Object.keys(metrics).length ? '时间未知' : '尚未上报']]
  const interfaces = Object.entries(metrics.network_interfaces ?? {})
  return <>
    {!entry.online && <div className="notice">{entry.device_public_key ? '设备当前离线，以下指标是最近一次上报的数据。' : '设备尚未接入。点击“接入 / 升级”获取安装命令。'}</div>}
    {entry.metrics_stale && <div className="notice">指标过期，以下保留的是最后一次采集的历史数据。最后指标时间：{entry.metrics_sampled_at ? time(entry.metrics_sampled_at / 1000) : '未知'}。心跳独立更新，在线状态不代表指标仍在采集。</div>}
    <div className="stats-grid stats-four">
      <Stat icon="activity" label="处理器使用率" value={percent(metrics.cpu_percent)} note={metrics.load_1 === undefined ? '负载尚未上报' : `1 分钟负载 ${metrics.load_1.toFixed(2)}`} />
      <Stat icon="server" label="已用内存" value={bytes(metrics.memory_used)} note={info.memory_total === undefined ? '总量尚未上报' : `总计 ${bytes(info.memory_total)}`} />
      <Stat icon="box" label="已用磁盘" value={bytes(metrics.disk_used)} note={info.disk_total === undefined ? '总量尚未上报' : `总计 ${bytes(info.disk_total)}`} />
      <Stat icon="check" label="运行时间" value={uptime(metrics.uptime_secs)} note="自系统最近一次启动" />
    </div>
    <section className="panel"><div className="panel-heading"><h2>连接概况</h2><Icon name="activity" size={18} /></div><div className="panel-body"><dl className="detail-list">
      <div><dt>TCP 连接数</dt><dd>{metrics.tcp_connections ?? '暂无数据'}</dd></div>
      <div><dt>UDP 连接数</dt><dd>{metrics.udp_connections ?? '暂无数据'}</dd></div>
      <div><dt>系统负载（1 / 5 / 15 分钟）</dt><dd>{[metrics.load_1, metrics.load_5, metrics.load_15].map(value => value?.toFixed(2) ?? '—').join(' / ')}</dd></div>
      <div><dt>面板期望版本</dt><dd>{entry.manifest_rev || '尚未发布'}</dd></div>
    </dl></div></section>
    <section className="panel"><div className="panel-heading"><h2>扩展系统指标</h2></div>
      <div className="panel-body"><dl className="detail-list"><div><dt>SWAP</dt><dd>{bytes(metrics.swap_used)} / {bytes(metrics.swap_total)}</dd></div><div><dt>进程数</dt><dd>{metrics.processes ?? '暂无数据'}</dd></div></dl></div>
      {metrics.disks?.length ? <div className="table-wrap"><table><thead><tr><th>磁盘 / 挂载点</th><th>读速率</th><th>写速率</th><th>读 / 写 IOPS</th><th>等待 / 利用率</th></tr></thead><tbody>{metrics.disks.map((disk, index) => <tr key={`${disk.name}-${index}`}>
        <td>{disk.name}<small>{disk.mount_point}</small></td>
        <td>{disk.read_bytes_per_sec == null ? '暂无数据' : `${bytes(disk.read_bytes_per_sec)}/秒`}</td>
        <td>{disk.write_bytes_per_sec == null ? '暂无数据' : `${bytes(disk.write_bytes_per_sec)}/秒`}</td>
        <td>{disk.read_iops?.toFixed(1) ?? '—'} / {disk.write_iops?.toFixed(1) ?? '—'}</td>
        <td>{disk.await_ms?.toFixed(1) ?? '—'} 毫秒 / {disk.utilization_percent?.toFixed(1) ?? '—'}%</td>
      </tr>)}</tbody></table></div> : <div className="inline-empty">设备尚未上报逐磁盘指标。</div>}
      {metrics.gpus?.length ? <div className="table-wrap"><table><thead><tr><th>GPU</th><th>使用率</th><th>显存</th></tr></thead><tbody>{metrics.gpus.map((gpu, index) => <tr key={index}>
        <td>{gpu.model}</td>
        <td>{gpu.usage_percent?.toFixed(1) ?? '暂无数据'}{gpu.usage_percent != null && '%'}</td>
        <td>{bytes(gpu.memory_used ?? undefined)} / {bytes(gpu.memory_total ?? undefined)}</td>
      </tr>)}</tbody></table></div> : <div className="inline-empty">暂无 GPU 数据。</div>}
    </section>
    <section className="panel"><div className="panel-heading"><h2>系统信息</h2><span className="subtle">缺失字段显示暂无数据</span></div><dl className="info-grid">{rows.map(([label, value]) => <div key={label}><dt>{label}</dt><dd>{value ?? '暂无数据'}</dd></div>)}</dl></section>
    <section className="panel"><div className="panel-heading"><h2>网络接口</h2><span className="subtle">服务器层面的累计计数</span></div>
      {interfaces.length ? <div className="table-wrap"><table><thead><tr><th>接口</th><th>累计接收</th><th>累计发送</th><th>接收速率</th><th>发送速率</th></tr></thead><tbody>{interfaces.map(([name, metric]) => <tr key={name}>
        <td className="mono">{name}</td>
        <td>{bytes(metric.received_bytes)}</td>
        <td>{bytes(metric.transmitted_bytes)}</td>
        <td>{metric.receive_bytes_per_sec === undefined ? '暂无数据' : `${bytes(metric.receive_bytes_per_sec)}/秒`}</td>
        <td>{metric.transmit_bytes_per_sec === undefined ? '暂无数据' : `${bytes(metric.transmit_bytes_per_sec)}/秒`}</td>
      </tr>)}</tbody></table></div> : <div className="inline-empty">设备尚未上报网络接口信息。</div>}
    </section>
  </>
}
