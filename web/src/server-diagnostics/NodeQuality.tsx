import { useState } from 'react'
import DiagnosticSections from './DiagnosticSections'
import { api } from '../api'
import { Badge, ErrorNotice, Icon, Loading } from '../components'
import { time } from '../format'
import { resourceWriteError, useAction, useResource } from '../hooks'
import { diagnosticActive, diagnosticCancellable, diagnosticCancelError, diagnosticUnconfirmed } from '../diagnostics'
import type { DiagnosticRecord, IpQuality, NodeQuality as NodeQualityData } from '../types'

function safeReportLink(value?: string) {
  if (!value) return undefined
  try {
    const url = new URL(value)
    if (url.protocol === 'https:' && ['nodequality.com', 'www.nodequality.com'].includes(url.hostname) && !url.username && !url.password && !url.port && !url.hash) return url.href
  } catch { /* Reports remain available as plain text when a link is invalid. */ }
  return undefined
}

const statusLabels = { queued: '等待设备领取', running: '设备正在测试', cleaning: '等待设备确认清理', cancel_requested: '等待设备确认取消', cancelled: '设备已确认取消', succeeded: '报告已完成', failed: '报告失败' }
const versionLabels: Record<string, string> = { both: 'IPv4 与 IPv6', ipv4: 'IPv4', ipv6: 'IPv6' }

function ReportResult({ record, serverId, cancelSupported, writeError, reload }: { record: DiagnosticRecord; serverId: number; cancelSupported: boolean; writeError: (id: string) => string; reload: () => void }) {
  const cancel = useAction()
  const copy = useAction()
  const canCancel = diagnosticCancellable(record)
  const reportUrl = safeReportLink(record.report?.report_url)
  return <article className="quality-report">
    <div className="quality-report-heading"><Badge tone={record.status === 'succeeded' ? 'good' : record.status === 'failed' ? 'bad' : 'warm'}>{statusLabels[record.status]}</Badge><span className="subtle">{time(record.created_at)} · {record.job.options.mode === 'daily' ? '日常检查' : record.job.options.mode === 'ip' ? '正式节点 IP 查询' : '完整验机'} · {versionLabels[record.job.options.ip_version] ?? '全部 IP'} · {record.job.options.network_mode === 'low' ? '低流量模式' : '标准流量模式'}{record.job.options.upload_report !== undefined && ` · ${record.job.options.upload_report === 'true' ? '历史上传设置开启' : ['daily', 'ip'].includes(record.job.options.mode ?? '') ? '仅本地报告' : '历史顶层上传设置关闭'}`}</span></div>
    {record.status === 'queued' && <p className="helper">{['daily', 'ip'].includes(record.job.options.mode ?? '') ? '任务已保存，等待在线 Agent 领取。通常会在数秒内开始。' : '旧完整任务已暂停新执行；已有运行检查点仍可回收报告或请求取消。'}</p>}
    {record.status === 'running' && <p className="helper">{record.job.options.mode === 'daily' ? '正在执行有限 TCP 连接检查，IP 信息通过面板查询并保留缓存。' : record.job.options.mode === 'ip' ? '正在 Agent 节点核实出口并调用已配置的正式 IP 接口，失败保留上次成功；不执行流媒体认证。' : '正在服务器本机测试处理器、磁盘、IP 质量与网络，完整测试需要数分钟。'}页面会自动更新。任务截止 {time(record.expires_at)}。</p>}
    {record.status === 'cleaning' && <p className="helper">测试执行已停止或正在停止，等待设备确认进程、排队任务和挂载已清理。已有报告仍可查看；断连或 Agent 重启后继续清理，确认前不能开始下一项诊断。</p>}
    {['failed', 'succeeded'].includes(record.status) && diagnosticUnconfirmed(record) && <p className="helper">已有执行结果尚未取得设备停止与清理确认，仍占用诊断位置。报告保留，可请求取消；确认清理完成前不能开始下一项诊断。</p>}
    {record.status === 'cancel_requested' && <p className="helper">取消请求已保存，等待设备确认取消。只有设备确认进程与挂载已清理后才显示已取消；设备断连或重启后会继续处理。</p>}
    {canCancel && <div className="quality-report-actions"><button className="button button-secondary" disabled={cancel.busy || Boolean(writeError(record.id))} onClick={() => { if (writeError(record.id)) return; void cancel.run(() => api<DiagnosticRecord>(`/api/servers/${serverId}/diagnostics/${record.id}/cancel`, 'POST'), reload) }}>{cancel.busy ? '提交取消请求…' : '请求取消测试'}</button>{!cancelSupported && <span className="helper">此 Agent 或服务后端不支持确认式取消，请先升级。</span>}</div>}
    <ErrorNotice message={cancel.error} />
    {record.cancel_error && <div className="notice break-all">{record.cancel_error}</div>}
    {record.error && <div className="notice notice-error break-all">{record.error}</div>}
    <DiagnosticSections record={record} />
    {record.report && <><div className="quality-report-actions">{reportUrl ? <a className="button button-secondary" href={reportUrl} target="_blank" rel="noopener noreferrer">打开公开报告 <Icon name="arrow" size={14} /></a> : <span className="helper">未生成在线链接，本地报告如下。</span>}<button className="button button-secondary" disabled={copy.busy} onClick={() => void copy.run(() => navigator.clipboard.writeText(record.report!.text), () => {})}>复制报告文本</button></div><ErrorNotice message={copy.error} /><details className="quality-report-text"><summary>查看报告文本</summary><pre>{record.report.text}</pre></details></>}
  </article>
}

export default function NodeQuality({ serverId }: { serverId: number }) {
  const resource = useResource<NodeQualityData>(`/api/servers/${serverId}/node-quality/reports`)
  const refresh = useAction()
  const run = useAction()
  const [ipVersion, setIpVersion] = useState('both')
  const [networkMode, setNetworkMode] = useState('low')
  const [uploadReport, setUploadReport] = useState(false)
  const [confirmFull, setConfirmFull] = useState(false)
  const [trafficAcknowledged, setTrafficAcknowledged] = useState(false)
  const data = resource.data
  const active = data?.reports.some(diagnosticActive)
  const trafficWarning = data?.proxy_activity?.state !== 'not_enabled'
  const warningText = data?.proxy_activity?.reason ?? '代理流量状态未知，不能确认当前无活跃连接。'
  const cancelError = (id: string) => resourceWriteError(resource) || diagnosticCancelError(resource.getCurrent(), id)
  const createError = (mode: 'daily' | 'full') => {
    const stale = resourceWriteError(resource)
    if (stale) return stale
    const current = resource.getCurrent()!
    if (!current.plugin_ready) return current.plugin_reason || '当前设备尚不能执行诊断，请刷新后确认。'
    if (current.reports.some(diagnosticActive)) return '此服务器已有诊断任务或正在等待设备确认清理，请等待设备完成。'
    if (mode === 'full' && (!current.full_ready || !confirmFull || (current.proxy_activity?.state !== 'not_enabled' && !trafficAcknowledged))) return current.full_reason || '请确认完整验机的资源占用与当前代理流量风险。'
    return ''
  }
  const submit = (mode: 'daily' | 'full') => void run.run(
    () => { const error = createError(mode); if (error) throw new Error(error); return api<DiagnosticRecord>(`/api/servers/${serverId}/node-quality/reports`, 'POST', {
      mode, ip_version: ipVersion, network_mode: mode === 'daily' ? 'low' : networkMode,
      upload_report: mode === 'full' && uploadReport, confirm_full: mode === 'full' && confirmFull,
      acknowledge_traffic_warning: mode === 'full' && trafficAcknowledged,
    }) },
    () => {
      setConfirmFull(false); setTrafficAcknowledged(false); setUploadReport(false); resource.reload()
      if (mode === 'daily') void refresh.run(() => api<IpQuality[]>(`/api/servers/${serverId}/ip-quality/refresh`, 'POST'), () => resource.reload())
    },
  )
  return <section className="panel">
    <div className="panel-heading"><h2>NodeQuality 验机</h2></div>
    <div className="panel-body quality-body"><ErrorNotice message={resource.error || refresh.error || run.error} retry={resource.reload} />
      {!data ? resource.loading && <Loading /> : <>
        <div className="quality-runner"><h3>检查入口</h3><p className="helper">日常检查刷新 IP 查询缓存，并对最多 4 个已启用的 TCP 拨测目标做有限连接探测。每种 IP 版本 4 次，DNS 最多 2 秒、每连接最多 1 秒；不跑硬件、带宽或回程路由，不公开上传。未配置目标时网络质量未知，IP 查询不是节点流媒体解锁验证。</p><p className="helper">完整验机目前暂停新执行，等待离线受控工具链。日常检查预算为 64 MiB / 32 个任务，仍需额外预留 256 MiB 可用内存和 2 GiB 磁盘。已保存的完整报告、独立章节及取消功能继续可用。</p>
          {data.plugin_reason && <div className="notice">{data.plugin_reason}</div>}
          {!data.full_ready && <div className="notice notice-error">{data.full_reason ?? '完整验机当前不可运行：离线受控工具链状态未知。'}</div>}
          <div className="quality-options"><label>测试 IP 版本<select value={ipVersion} onChange={event => setIpVersion(event.target.value)} disabled={run.busy || active}><option value="both">IPv4 与 IPv6</option><option value="ipv4">仅 IPv4</option><option value="ipv6">仅 IPv6</option></select></label><label>网络测试流量<select value={networkMode} onChange={event => setNetworkMode(event.target.value)} disabled={!data.full_ready || run.busy || active}><option value="low">低流量模式</option><option value="normal">标准流量模式</option></select></label><label>公开报告上传<select value={String(uploadReport)} onChange={event => setUploadReport(event.target.value === 'true')} disabled={!data.full_ready || run.busy || active}><option value="false">关闭，仅保留本地报告</option><option value="true">上传并生成公开报告</option></select></label><button className="button button-secondary" disabled={Boolean(createError('daily')) || run.busy || refresh.busy} onClick={() => submit('daily')}><Icon name="activity" size={16} />日常检查</button></div>
          <div className="notice"><label><input type="checkbox" checked={confirmFull} disabled={!data.full_ready || run.busy || active} onChange={event => setConfirmFull(event.target.checked)} /> 我已确认完整验机会占用处理器、内存、磁盘和网络资源。</label>{trafficWarning && <><p>{warningText}</p><label><input type="checkbox" checked={trafficAcknowledged} disabled={!data.full_ready || run.busy || active} onChange={event => setTrafficAcknowledged(event.target.checked)} /> 我已知悉活跃代理流量或流量未知的风险，仍运行完整验机。</label></>}<p className="helper">完整验机的历史上传设置只能约束顶层，不能证明旧脚本没有内层外传。新执行已禁用。</p><button className="button button-primary" disabled={Boolean(createError('full')) || run.busy} onClick={() => submit('full')}>{run.busy ? '创建任务…' : active ? '报告正在执行' : '完整验机'}</button></div>
        </div>
        <div className="quality-history"><h3>最近报告</h3>{data.reports.length ? data.reports.map(record => <ReportResult key={record.id} record={record} serverId={serverId} cancelSupported={data.cancel_supported} writeError={cancelError} reload={resource.reload} />) : <p className="helper">还没有运行过报告。</p>}</div>
      </>}
    </div>
  </section>
}
