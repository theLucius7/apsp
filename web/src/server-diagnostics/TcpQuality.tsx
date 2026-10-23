import { useState } from 'react'
import { api } from '../api'
import { Badge, ErrorNotice, Loading } from '../components'
import { time } from '../format'
import { resourceWriteError, useAction, useResource } from '../hooks'
import { diagnosticActive, diagnosticCancellable, diagnosticCancelError, diagnosticUnconfirmed } from '../diagnostics'
import type { DiagnosticRecord, DiagnosticView, TcpQualityTarget } from '../types'
import DiagnosticSections from './DiagnosticSections'
import { Button } from '@/components/ui/button'
import { NativeSelect } from '@/components/ui/native-select'
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table'
import { Notice, Panel, PanelHeader, PanelTitle } from '@/components/panel'

const regions = [{ value: 'configured', label: '全部已配置目标' }, { value: 'east_asia', label: '东亚' }, { value: 'southeast_asia', label: '东南亚' }, { value: 'europe', label: '欧洲' }, { value: 'americas', label: '美洲' }, { value: 'other', label: '其他地区' }]
const statuses = { queued: '等待设备领取', running: '设备正在测试', cleaning: '等待设备确认清理', cancel_requested: '等待设备确认取消', cancelled: '设备已确认取消', succeeded: '测试已完成', failed: '测试失败' }
const regionLabel = (value?: string | null) => regions.find(region => region.value === value)?.label ?? '地区未知'
const numeric = (value: unknown, unit = '') => typeof value === 'number' && Number.isFinite(value) ? `${value.toFixed(2)}${unit}` : '未知'
function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
}
function parsed(text?: string): Record<string, unknown> {
  try { return object(JSON.parse(text ?? '')) } catch { return {} }
}
function targetResults(record: DiagnosticRecord) {
  const results = new Map<string, Record<string, unknown>>()
  const add = (value: unknown) => {
    const entry = object(value); const target = object(entry.target)
    const attempted = object(entry.summary).attempted
    if (typeof target.id !== 'string' || !target.id || typeof entry.complete !== 'boolean' || typeof attempted !== 'number' || !Number.isSafeInteger(attempted) || attempted < 0) return
    const existing = results.get(target.id)
    const existingAttempted = object(existing?.summary).attempted
    if (!existing || (entry.complete && !existing.complete) || (entry.complete === existing.complete && typeof existingAttempted === 'number' && attempted > existingAttempted)) results.set(target.id, entry)
  }
  const report = parsed(record.report?.text)
  if (Array.isArray(report.targets)) report.targets.forEach(add)
  for (const section of record.sections ?? []) if (section.name.startsWith('tcp_target_')) add(parsed(section.text))
  return results
}

function Report({ record, serverId, cancelSupported, writeError, reload }: { record: DiagnosticRecord; serverId: number; cancelSupported: boolean; writeError: (id: string) => string; reload: () => void }) {
  const cancel = useAction()
  const copy = useAction()
  const metadata = record.job.tcpquality
  const report = parsed(record.report?.text)
  const engine = object(report.engine)
  const results = targetResults(record)
  const canCancel = diagnosticCancellable(record)
  return <article data-slot="quality-report" className="bg-muted/40 mt-4 min-w-0 rounded-xl border p-4 max-sm:p-3">
    <div className="flex flex-wrap items-center gap-2.5"><Badge tone={record.status === 'succeeded' ? 'good' : record.status === 'failed' ? 'bad' : 'warm'}>{statuses[record.status]}</Badge><span className="text-muted-foreground text-xs">{time(record.created_at)} · IPv{record.job.options.ip_version} · 每目标 {record.job.options.count ?? '未知'} 次 · {record.job.options.concurrency ?? '未知'} 并发 · {regionLabel(metadata?.region)}</span></div>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">工具版本：{record.job.version ?? '未知'}。{typeof engine.source_commit === 'string' && `执行源码：${engine.source_commit}。`}此结果只代表本次目标和参数下的 TCP 连接，不进行排名。</p>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">开始时间：{typeof report.started_at_ms === 'number' ? time(report.started_at_ms / 1000) : '等待设备回报'} · 结束时间：{typeof report.finished_at_ms === 'number' ? time(report.finished_at_ms / 1000) : '尚未回报'}。缺失或未执行的指标显示未知。</p>
    {record.status === 'cleaning' && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">测试执行已停止或正在停止，等待设备确认进程、排队任务和挂载已清理。已有报告仍可查看；断连或 Agent 重启后继续清理，确认前不能开始下一项诊断。</p>}
    {['failed', 'succeeded'].includes(record.status) && diagnosticUnconfirmed(record) && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">已有执行结果尚未取得设备停止与清理确认，仍占用诊断位置。报告保留，可请求取消；确认清理完成前不能开始下一项诊断。</p>}
    {record.status === 'cancel_requested' && <Notice className="my-3">等待设备确认取消。Agent 断连或重启后继续处理；确认清理完成前，此服务器不能开始下一项诊断。</Notice>}
    {canCancel && <Button variant="outline" disabled={cancel.busy || Boolean(writeError(record.id))} onClick={() => { if (writeError(record.id)) return; void cancel.run(() => api(`/api/servers/${serverId}/diagnostics/${record.id}/cancel`, 'POST'), reload) }}>{cancel.busy ? '提交取消请求…' : '请求取消测试'}</Button>}
    {canCancel && !cancelSupported && <p className="text-muted-foreground mt-3 text-xs leading-relaxed">此 Agent 尚不支持确认式取消，请先升级。</p>}
    <ErrorNotice message={cancel.error} />
    {record.cancel_error && <Notice className="my-3">{record.cancel_error}</Notice>}
    {record.error && <Notice tone="error" className="my-3">{record.error}</Notice>}
    <h4 className="text-sm font-medium">本次冻结的目标范围</h4>
    <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>目标</TableHead><TableHead>地区 / 运营商</TableHead><TableHead>连接成功</TableHead><TableHead>连接耗时</TableHead><TableHead>执行情况</TableHead></TableRow></TableHeader><TableBody>{(Array.isArray(metadata?.targets) ? metadata.targets : []).map(target => {
      const result = results.get(target.id) ?? {}; const summary = object(result.summary)
      return <TableRow key={target.id}><TableCell><strong>{target.name}</strong><small className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">{target.target}:{target.port}{typeof result.address === 'string' && ` · 实际 ${result.address}`}</small></TableCell><TableCell>{regionLabel(target.region)} / {target.carrier || '运营商未知'}</TableCell><TableCell>{numeric(summary.connection_success_percent, '%')}<small className="text-muted-foreground mt-3 text-xs leading-relaxed">{typeof summary.succeeded === 'number' && typeof summary.attempted === 'number' ? `${summary.succeeded} / ${summary.attempted} 次` : '尚未回报'}</small></TableCell><TableCell>均值 {numeric(summary.latency_mean_ms, ' ms')}<small className="text-muted-foreground mt-3 text-xs leading-relaxed">最小 {numeric(summary.latency_min_ms, ' ms')} / 最大 {numeric(summary.latency_max_ms, ' ms')}</small></TableCell><TableCell>{result.complete === true ? '已完成' : '未完成'}{typeof result.error === 'string' && <small className="text-muted-foreground mt-3 text-xs leading-relaxed">{result.error}</small>}</TableCell></TableRow>
    })}</TableBody></Table></div>
    <p className="text-muted-foreground mt-3 text-xs leading-relaxed">连接成功率不是网络丢包率；本测试不测重传率或带宽。资源限制和启动负载保存在环境章节。</p>
    <DiagnosticSections record={record} />
    {record.report && <><Button variant="outline" disabled={copy.busy} onClick={() => void copy.run(() => navigator.clipboard.writeText(record.report!.text), () => {})}>复制本次报告 JSON</Button><ErrorNotice message={copy.error} /><details data-slot="quality-report-text" className="mt-4"><summary className="text-primary cursor-pointer text-sm">查看完整原始报告</summary><pre className="bg-card mt-3 max-h-[460px] overflow-auto rounded-lg border p-4 leading-relaxed whitespace-pre-wrap wrap-anywhere max-sm:p-2.5">{record.report.text}</pre></details></>}
  </article>
}

export default function TcpQuality({ serverId }: { serverId: number }) {
  const diagnostics = useResource<DiagnosticView>(`/api/servers/${serverId}/diagnostics`)
  const targets = useResource<TcpQualityTarget[]>(`/api/plugins/tcpquality/servers/${serverId}/targets`)
  const run = useAction()
  const configure = useAction()
  const [region, setRegion] = useState('configured')
  const [ipVersion, setIpVersion] = useState('4')
  const [count, setCount] = useState(4)
  const [concurrency, setConcurrency] = useState(1)
  const data = diagnostics.data
  const readiness = diagnostics.error ? undefined : data?.plugins.find(plugin => plugin.plugin === 'tcpquality')
  const active = data?.reports.some(diagnosticActive)
  const currentTargets = targets.error ? undefined : targets.data
  const selected = currentTargets?.filter(target => region === 'configured' || target.region === region)
  const reports = data?.reports.filter(record => record.job.plugin === 'tcpquality') ?? []
  const cancelError = (id: string) => resourceWriteError(diagnostics) || diagnosticCancelError(diagnostics.getCurrent(), id)
  const createError = () => {
    const stale = resourceWriteError(diagnostics, targets)
    if (stale) return stale
    const current = diagnostics.getCurrent()!, readiness = current.plugins.find(plugin => plugin.plugin === 'tcpquality')
    if (!readiness?.ready) return readiness?.reason || '当前设备尚不能执行 TCP 诊断，请刷新后确认。'
    if (current.reports.some(diagnosticActive)) return '此服务器已有诊断任务或正在等待设备确认清理，请等待设备完成。'
    const selected = targets.getCurrent()?.filter(target => region === 'configured' || target.region === region)
    return !selected?.length || selected.length > 8 ? '请确认本次选择包含 1 至 8 个已配置目标。' : ''
  }
  const submit = () => void run.run(() => { const error = createError(); if (error) throw new Error(error); return api<DiagnosticRecord>(`/api/servers/${serverId}/diagnostics/tcpquality`, 'POST', { region, ip_version: ipVersion, count, concurrency }) }, diagnostics.reload)
  return <Panel><PanelHeader><PanelTitle>TCP 连接诊断</PanelTitle></PanelHeader><div data-slot="quality-body" className="p-4 grid gap-5 grid-cols-1 *:min-w-0">
    <ErrorNotice message={diagnostics.error || targets.error || run.error || configure.error} retry={() => { diagnostics.reload(); targets.reload() }} />
    {!data ? diagnostics.loading && <Loading /> : <>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">原生 Rust 探测，仅连接本机已配置的 TCP 拨测目标，不发送应用数据、不向第三方上传报告、不测速。最多 8 个目标、64 次连接、60 秒；预算 64 MiB / 32 个任务，启动前需额外预留 256 MiB 可用内存和 2 GiB 磁盘。</p>
      {readiness?.reason && <Notice>{readiness.reason}</Notice>}
      {diagnostics.error ? <Notice>当前诊断状态未知，无法确认设备能力或正在执行的任务；状态恢复前暂停创建。</Notice> : !readiness && <Notice>当前面板尚未登记 TCP 诊断插件。</Notice>}
      {active && <Notice>此服务器已有诊断任务或正在等待取消确认；请等待设备完成。</Notice>}
      <div data-slot="quality-options" className="mt-4 flex flex-wrap items-end gap-3 max-sm:items-stretch">
        <label className="text-muted-foreground grid gap-1.5 text-xs max-sm:min-w-32 max-sm:flex-1">目标地区<NativeSelect className="w-full" aria-label="目标地区" value={region} onChange={event => setRegion(event.target.value)} disabled={run.busy || active}>{regions.map(region => <option key={region.value} value={region.value}>{region.label}</option>)}</NativeSelect></label>
        <label className="text-muted-foreground grid gap-1.5 text-xs max-sm:min-w-32 max-sm:flex-1">IP 版本<NativeSelect className="w-full" aria-label="IP 版本" value={ipVersion} onChange={event => setIpVersion(event.target.value)} disabled={run.busy || active}><option value="4">IPv4</option><option value="6">IPv6</option></NativeSelect></label>
        <label className="text-muted-foreground grid gap-1.5 text-xs max-sm:min-w-32 max-sm:flex-1">每目标连接次数<NativeSelect className="w-full" aria-label="每目标连接次数" value={count} onChange={event => setCount(Number(event.target.value))} disabled={run.busy || active}><option value={4}>4 次</option><option value={8}>8 次</option></NativeSelect></label>
        <label className="text-muted-foreground grid gap-1.5 text-xs max-sm:min-w-32 max-sm:flex-1">最大并发<NativeSelect className="w-full" aria-label="最大并发" value={concurrency} onChange={event => setConcurrency(Number(event.target.value))} disabled={run.busy || active}><option value={1}>1</option><option value={2}>2</option></NativeSelect></label>
        <Button className="max-sm:w-full" disabled={Boolean(createError()) || run.busy || configure.busy} onClick={submit}>{run.busy ? '创建任务…' : '开始 TCP 诊断'}</Button>
      </div>
      <p className="text-muted-foreground mt-3 text-xs leading-relaxed">{selected ? `本次将冻结 ${selected.length} 个目标` : '尚未取得目标列表，本次目标数未知'}；单次允许 1 至 8 个。IPv4/IPv6 不可解析或连接失败会保留原因。不同目标、地区或参数的结果不做横向排名。</p>
      <details><summary className="cursor-pointer text-sm font-medium">配置目标地区</summary><p className="text-muted-foreground mt-3 text-xs leading-relaxed">地区由管理员标注，不根据 IP 推断。仅使用自有或获准使用的目标；在服务器「Agent 与任务」的周期拨测中添加、停用或修改目标。</p>
        {!currentTargets ? <p className="text-muted-foreground mt-3 text-xs leading-relaxed">尚未取得已启用的 TCP 拨测目标，当前目标配置未知。</p> : !currentTargets.length ? <p className="text-muted-foreground mt-3 text-xs leading-relaxed">没有已启用的 TCP 拨测目标。</p> : <div className="max-w-full min-w-0"><Table><TableHeader><TableRow><TableHead>目标</TableHead><TableHead>运营商</TableHead><TableHead>地区</TableHead></TableRow></TableHeader><TableBody>{currentTargets.map(target => <TableRow key={target.id}><TableCell>{target.name}<small className="text-muted-foreground mt-3 text-xs leading-relaxed wrap-anywhere">{target.target}:{target.port}</small></TableCell><TableCell>{target.carrier || '未知'}</TableCell><TableCell><NativeSelect aria-label={`${target.name}地区`} value={target.region ?? ''} disabled={configure.busy} onChange={event => void configure.run(() => api(`/api/plugins/tcpquality/servers/${serverId}/targets/${target.id}`, 'PATCH', { region: event.target.value || null }), targets.reload)}><option value="">地区未知</option>{regions.filter(region => region.value !== 'configured').map(region => <option key={region.value} value={region.value}>{region.label}</option>)}</NativeSelect></TableCell></TableRow>)}</TableBody></Table></div>}
      </details>
      <div className="min-w-0 border-t pt-5"><h3 className="mb-2 text-sm font-semibold">最近 TCP 报告</h3>{reports.length ? reports.map(record => <Report key={record.id} record={record} serverId={serverId} cancelSupported={data.cancel_supported} writeError={cancelError} reload={diagnostics.reload} />) : <p className="text-muted-foreground mt-3 text-xs leading-relaxed">尚无 TCP 诊断报告。</p>}</div>
    </>}
  </div></Panel>
}
