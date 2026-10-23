import { useState } from 'react'
import { AgentSettings } from './AgentSettings'
import AgentTasks from './AgentTasks'
import { Badge, ErrorNotice, Loading, PageHeader, Refresh, Stat } from '../components'
import { bytes, navigate, percent, time, uptime } from '../format'
import { useResource } from '../hooks'
import type { Server } from '../types'
import ServerNavigation from './ServerNavigation'
import ServerEnrollment from './ServerEnrollment'
import ServerEdit from './ServerEdit'
import ServerAssets from './ServerAssets'
import { ServerPlugins } from '../plugins'
import { Button, buttonVariants } from '@/components/ui/button'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { ActivityIcon, ArrowLeftIcon, BoxIcon, CheckIcon, PlusIcon, ServerIcon } from 'lucide-react'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'

export default function ServerDetail({ id }: { id: number }) {
  const server = useResource<Server>(`/api/servers/${id}`)
  const [pluginRevision, setPluginRevision] = useState(0)
  const [installing, setInstalling] = useState(false)
  const [editing, setEditing] = useState(false)
  const refresh = () => { server.reload(); setPluginRevision(value => value + 1) }
  const entry = server.data
  if (!entry) return <><Button variant="ghost" size="sm" className="mb-4 -ml-2" onClick={() => navigate('/servers')}><ArrowLeftIcon aria-hidden="true" />返回服务器</Button><ErrorNotice message={server.error} retry={server.reload} />{server.loading && <Loading />}</>
  const metrics = entry.latest_metrics, info = entry.static_info
  const rows = [['操作系统', info.system], ['内核版本', info.kernel], ['系统架构', info.arch], ['主机名', info.hostname], ['处理器型号', info.cpu_model], ['核心数', info.cpu_cores], ['总内存', info.memory_total === undefined ? undefined : bytes(info.memory_total)], ['磁盘容量', info.disk_total === undefined ? undefined : bytes(info.disk_total)], ['虚拟化', info.virtualization], ['Agent 版本', info.agent_version], ['最近设备消息', entry.last_seen ? time(entry.last_seen) : undefined], ['最后心跳', entry.last_heartbeat_at ? time(entry.last_heartbeat_at) : '尚未记录'], ['最后指标', entry.metrics_sampled_at ? time(entry.metrics_sampled_at / 1000) : Object.keys(metrics).length ? '时间未知' : '尚未上报']]
  return <>
    <Button variant="ghost" size="sm" className="mb-4 -ml-2" onClick={() => navigate('/servers')}><ArrowLeftIcon aria-hidden="true" />返回服务器</Button>
    <PageHeader eyebrow={`服务器 #${entry.id}`} title={entry.name} description="系统概况、运行指标与设备状态。"><Badge tone={entry.online ? 'good' : 'neutral'}>{entry.online ? '在线' : entry.device_public_key ? '离线' : '待接入'}</Badge>{entry.metrics_stale && <Badge tone="warm">指标过期</Badge>}<Refresh onClick={refresh} /><Button onClick={() => setInstalling(true)}><PlusIcon aria-hidden="true" />接入 / 升级</Button></PageHeader>
    <ServerNavigation id={id} active="overview" />
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed"><a className={buttonVariants({ variant: 'ghost', size: 'sm' })} href="#/system/plugins">管理服务器插件</a> · 需要代理服务时，可在插件设置中安装 sing-box。</p>
    <ErrorNotice message={server.error} retry={refresh} />
    {!entry.online && <Notice>{entry.device_public_key ? '设备当前离线，以下指标是最近一次上报的数据。' : '设备尚未接入。点击“接入 / 升级”获取安装命令。'}</Notice>}
    {entry.metrics_stale && <Notice>指标过期，以下保留的是最后一次采集的历史数据。最后指标时间：{entry.metrics_sampled_at ? time(entry.metrics_sampled_at / 1000) : '未知'}。心跳独立更新，在线状态不代表指标仍在采集。</Notice>}
    <div className="mb-6 grid gap-3 max-sm:gap-2 grid-cols-2 lg:grid-cols-4"><Stat icon={ActivityIcon} label="处理器使用率" value={percent(metrics.cpu_percent)} note={metrics.load_1 === undefined ? '负载尚未上报' : `1 分钟负载 ${metrics.load_1.toFixed(2)}`} /><Stat icon={ServerIcon} label="已用内存" value={bytes(metrics.memory_used)} note={info.memory_total === undefined ? '总量尚未上报' : `总计 ${bytes(info.memory_total)}`} /><Stat icon={BoxIcon} label="已用磁盘" value={bytes(metrics.disk_used)} note={info.disk_total === undefined ? '总量尚未上报' : `总计 ${bytes(info.disk_total)}`} /><Stat icon={CheckIcon} label="运行时间" value={uptime(metrics.uptime_secs)} note="自系统最近一次启动" /></div>
    <Panel><PanelHeader><PanelTitle>连接概况</PanelTitle><ActivityIcon className="text-muted-foreground size-4" aria-hidden="true" /></PanelHeader><div className="p-4"><dl className="m-0 grid gap-4 p-0"><div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">TCP 连接数</dt><dd className="m-0 text-sm">{metrics.tcp_connections ?? '暂无数据'}</dd></div><div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">UDP 连接数</dt><dd className="m-0 text-sm">{metrics.udp_connections ?? '暂无数据'}</dd></div><div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">系统负载（1 / 5 / 15 分钟）</dt><dd className="m-0 text-sm">{[metrics.load_1, metrics.load_5, metrics.load_15].map(value => value?.toFixed(2) ?? '—').join(' / ')}</dd></div><div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">面板期望版本</dt><dd className="m-0 text-sm">{entry.manifest_rev || '尚未发布'}</dd></div></dl></div></Panel>
    <ServerAssets onReload={server.reload} server={entry} onEdit={() => setEditing(true)} />
    <AgentSettings serverId={id} liveSupported={Array.isArray(entry.capabilities) && entry.capabilities.includes('telemetry:live:v1')} />
    <Panel><PanelHeader><PanelTitle>扩展系统指标</PanelTitle></PanelHeader><div className="p-4"><dl className="m-0 grid gap-4 p-0"><div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">SWAP</dt><dd className="m-0 text-sm">{bytes(metrics.swap_used)} / {bytes(metrics.swap_total)}</dd></div><div className="flex items-center justify-between gap-3"><dt className="text-muted-foreground text-sm">进程数</dt><dd className="m-0 text-sm">{metrics.processes ?? '暂无数据'}</dd></div></dl></div>
      {metrics.disks?.length ? <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>磁盘 / 挂载点</TableHead><TableHead>读速率</TableHead><TableHead>写速率</TableHead><TableHead>读 / 写 IOPS</TableHead><TableHead>等待 / 利用率</TableHead></TableRow></TableHeader><TableBody>{metrics.disks.map((disk, index) => <TableRow key={`${disk.name}-${index}`}><TableCell>{disk.name}<small>{disk.mount_point}</small></TableCell><TableCell>{disk.read_bytes_per_sec == null ? '暂无数据' : `${bytes(disk.read_bytes_per_sec)}/秒`}</TableCell><TableCell>{disk.write_bytes_per_sec == null ? '暂无数据' : `${bytes(disk.write_bytes_per_sec)}/秒`}</TableCell><TableCell>{disk.read_iops?.toFixed(1) ?? '—'} / {disk.write_iops?.toFixed(1) ?? '—'}</TableCell><TableCell>{disk.await_ms?.toFixed(1) ?? '—'} 毫秒 / {disk.utilization_percent?.toFixed(1) ?? '—'}%</TableCell></TableRow>)}</TableBody></Table></div> : <div className="text-muted-foreground px-4 py-6 text-sm">设备尚未上报逐磁盘指标。</div>}
      {metrics.gpus?.length ? <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>GPU</TableHead><TableHead>使用率</TableHead><TableHead>显存</TableHead></TableRow></TableHeader><TableBody>{metrics.gpus.map((gpu, index) => <TableRow key={index}><TableCell>{gpu.model}</TableCell><TableCell>{gpu.usage_percent?.toFixed(1) ?? '暂无数据'}{gpu.usage_percent != null && '%'}</TableCell><TableCell>{bytes(gpu.memory_used ?? undefined)} / {bytes(gpu.memory_total ?? undefined)}</TableCell></TableRow>)}</TableBody></Table></div> : <div className="text-muted-foreground px-4 py-6 text-sm">暂无 GPU 数据。</div>}
    </Panel>
    <AgentTasks key={id} serverId={id} commandsEnabled={Array.isArray(entry.capabilities) && entry.capabilities.includes('command:execute')} />
    <Panel><PanelHeader><PanelTitle>系统信息</PanelTitle><span className="text-muted-foreground text-xs">缺失字段显示暂无数据</span></PanelHeader><dl className="m-0 grid grid-cols-2 gap-x-6 gap-y-5 p-4 md:grid-cols-3">{rows.map(([label, value]) => <div key={label}><dt className="text-muted-foreground mb-1 text-xs">{label}</dt><dd className="m-0 text-sm wrap-anywhere">{value ?? '暂无数据'}</dd></div>)}</dl></Panel>
    <Panel><PanelHeader><PanelTitle>网络接口</PanelTitle><span className="text-muted-foreground text-xs">服务器层面的累计计数</span></PanelHeader>{Object.keys(metrics.network_interfaces ?? {}).length ? <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>接口</TableHead><TableHead>累计接收</TableHead><TableHead>累计发送</TableHead><TableHead>接收速率</TableHead><TableHead>发送速率</TableHead></TableRow></TableHeader><TableBody>{Object.entries(metrics.network_interfaces ?? {}).map(([name, metric]) => <TableRow key={name}><TableCell className="font-mono text-xs">{name}</TableCell><TableCell>{bytes(metric.received_bytes)}</TableCell><TableCell>{bytes(metric.transmitted_bytes)}</TableCell><TableCell>{metric.receive_bytes_per_sec === undefined ? '暂无数据' : `${bytes(metric.receive_bytes_per_sec)}/秒`}</TableCell><TableCell>{metric.transmit_bytes_per_sec === undefined ? '暂无数据' : `${bytes(metric.transmit_bytes_per_sec)}/秒`}</TableCell></TableRow>)}</TableBody></Table></div> : <div className="text-muted-foreground px-4 py-6 text-sm">设备尚未上报网络接口信息。</div>}</Panel>
    <ServerPlugins key={`${id}-${pluginRevision}`} server={entry} />
    {editing && <ServerEdit key={id} server={entry} onClose={() => setEditing(false)} onSaved={() => { setEditing(false); server.reload() }} />}
    {installing && <ServerEnrollment key={id} server={entry} onClose={() => { setInstalling(false); server.reload() }} />}
  </>
}
