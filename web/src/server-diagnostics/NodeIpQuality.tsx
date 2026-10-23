import { useRef, useState } from 'react'
import { api } from '../api'
import { Badge, ErrorNotice } from '../components'
import { diagnosticCancellable, diagnosticUnconfirmed } from '../diagnostics'
import { time } from '../format'
import { useAction } from '../hooks'
import { nodeIpCancelError, nodeIpStartError, queryErrorLabels } from '../ip-quality'
import type { DiagnosticRecord, NodeIpQuality as NodeIpQualityData, QualityErrorKind } from '../types'
import DiagnosticSections from './DiagnosticSections'

const statusLabels = { queued: '等待设备领取', running: '设备正在查询', cleaning: '等待设备确认清理', cancel_requested: '等待设备确认取消', cancelled: '设备已确认取消', succeeded: '执行已完成', failed: '执行失败' }
type Scope = { serverId: number; fresh: boolean; error: string; data?: NodeIpQualityData; isCurrent?: () => boolean; getCurrent?: () => NodeIpQualityData | undefined }
function object(value: unknown): Record<string, unknown> {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}
}
function envelope(record: DiagnosticRecord) {
  const section = record.sections?.find(item => item.name === 'ipquality_result')
  try { return object(JSON.parse(section?.text ?? record.report?.text ?? '')) } catch { return {} }
}

function NodeReport({ record, current, reload }: { record: DiagnosticRecord; current: React.RefObject<Scope>; reload: () => void }) {
  const cancel = useAction()
  const result = envelope(record)
  const attempts = Array.isArray(result.attempts) ? result.attempts.map(object) : []
  const server = current.current.serverId
  const cancelable = diagnosticCancellable(record)
  const submitCancel = () => void cancel.run(async () => {
    const error = nodeIpCancelError(current.current, server, record.id)
    if (error) throw new Error(error)
    return api(`/api/servers/${server}/diagnostics/${record.id}/cancel`, 'POST')
  }, reload)
  return <article className="quality-report">
    <div className="quality-report-heading"><Badge tone={record.status === 'failed' ? 'bad' : record.status === 'succeeded' ? 'neutral' : 'warm'}>{statusLabels[record.status]}</Badge><span className="subtle">{time(record.created_at)} · IPv{record.job?.options?.ip_version ?? '未知'}</span></div>
    <p className="helper">实际出口：{typeof result.egress_ip === 'string' ? result.egress_ip : '未知'} · 工具版本：{record.job?.version ?? '未知'}。</p>
    <p className="helper">开始于 {typeof result.started_at === 'number' ? time(result.started_at) : '等待设备回报'} · {typeof result.finished_at === 'number' ? `结束于 ${time(result.finished_at)}` : '执行结束时间未知'}。执行完成仅表示查询流程结束，各个来源的可用性分别查看。</p>
    {['failed', 'succeeded'].includes(record.status) && diagnosticUnconfirmed(record) && <p className="notice">已有结果尚未取得设备停止与清理确认，诊断位置仍被占用；可请求取消，历史结果保留。</p>}
    {record.status === 'cancel_requested' && <p className="notice">等待设备确认取消。进程和挂载清理确认之前，同机诊断保持互斥；已保存结果仍可查看。</p>}
    {record.status === 'cleaning' && <p className="notice">等待设备确认进程、挂载和排队任务均已清理。设备重启或面板断连后继续处理。</p>}
    {cancelable && <button className="button button-secondary" disabled={!current.current.fresh || !current.current.data?.cancel_supported || cancel.busy} onClick={submitCancel}>{cancel.busy ? '请求取消中…' : '请求取消节点自查'}</button>}
    <ErrorNotice message={cancel.error} />
    {(record.error || record.cancel_error) && <div className="notice notice-error">{record.cancel_error || record.error}</div>}
    {attempts.length > 0 && <details><summary>逐源查询记录（{attempts.length} 次）</summary><div className="table-wrap"><table><thead><tr><th>查询来源</th><th>目标与时间</th><th>结果</th></tr></thead><tbody>{attempts.map((attempt, index) => <tr key={typeof attempt.seq === 'number' ? attempt.seq : index}>
      <td>{typeof attempt.provider === 'string' ? attempt.provider : '来源未知'}<small className="helper">{typeof attempt.dataset === 'string' ? attempt.dataset : '响应视图未知'}</small></td>
      <td>{typeof attempt.target_ip === 'string' ? attempt.target_ip : '出口尚未确认'}<small className="helper">{typeof attempt.attempted_at === 'number' ? time(attempt.attempted_at) : '未尝试'} · {typeof attempt.elapsed_ms === 'number' ? `${attempt.elapsed_ms} 毫秒` : '耗时未知'}</small></td>
      <td>{typeof attempt.error_kind === 'string' ? queryErrorLabels[attempt.error_kind as QualityErrorKind] ?? '失败原因未分类' : attempt.status === 'succeeded' ? '请求成功，字段按响应单独确认' : '未知'}{typeof attempt.http_status === 'number' && ` · HTTP ${attempt.http_status}`}<small className="helper">{typeof attempt.error_message === 'string' ? attempt.error_message : ''}</small></td>
    </tr>)}</tbody></table></div></details>}
    <DiagnosticSections record={record} />
  </article>
}

export default function NodeIpQuality({ serverId, data, fresh, error, reload, isCurrent, getCurrent }: Scope & { reload: () => void }) {
  const run = useAction()
  const [family, setFamily] = useState('4')
  const current = useRef<Scope>({ serverId, data, fresh, error, isCurrent, getCurrent })
  current.current = { serverId, data, fresh, error, isCurrent, getCurrent }
  const startError = nodeIpStartError(current.current, serverId)
  const submit = () => void run.run(async () => {
    const error = nodeIpStartError(current.current, serverId)
    if (error) throw new Error(error)
    if (!['4', '6'].includes(family)) throw new Error('请选择 IPv4 或 IPv6。')
    return api<DiagnosticRecord>(`/api/servers/${serverId}/diagnostics/ipquality`, 'POST', { ip_version: family })
  }, reload)
  return <div className="quality-history node-ip-quality">
    <h3>节点出口自查</h3>
    <p className="helper">从节点自身网络出口执行固定版本的 IPQuality，逐源查询质量和流媒体可达性。单次选择一个 IP 版本；查询失败或字段不能确认时显示未知，最近成功结果继续保留。</p>
    <p className="helper">不进行硬件跑分、带宽测速、报告上传或宿主网络修改。邮件端口与 DNSBL 查询未执行；有凭据要求的来源保持未知，面板的 API 凭据不会发送给设备。</p>
    {startError && <div className="notice">{startError}</div>}
    <ErrorNotice message={run.error} />
    <div className="quality-options"><label>出口 IP 版本<select aria-label="出口 IP 版本" value={family} disabled={run.busy} onChange={event => setFamily(event.target.value)}><option value="4">IPv4</option><option value="6">IPv6</option></select></label><button className="button button-secondary" disabled={!!startError || run.busy} onClick={submit}>{run.busy ? '创建任务中…' : '运行节点出口自查'}</button></div>
    {data?.current_egress_ips.length ? <p className="helper">最近查询观察到的出口：{data.current_egress_ips.join('、')}。这表示本次查询出口，可能与网卡地址不同。</p> : <p className="helper">流媒体解锁：未知。当前出口尚未确认，不能据此判断流媒体是否解锁。</p>}
    {data?.reports.length ? data.reports.map(record => <NodeReport key={record.id} record={record} current={current} reload={reload} />) : <p className="helper">尚无节点自查报告。</p>}
  </div>
}
