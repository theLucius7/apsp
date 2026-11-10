import type { DiagnosticRecord } from '../types'

// New panels project cleanup evidence; older ID-bearing history stays blocked
// until the Agent confirms completion, including damaged task identities.
export const diagnosticUnconfirmed = (record: DiagnosticRecord) => record.cleanup_pending ?? (record.agent_completed === false && record.job != null && Object.prototype.hasOwnProperty.call(record.job, 'id'))
export const diagnosticActive = (record: DiagnosticRecord) => ['queued', 'running', 'cleaning', 'cancel_requested'].includes(record.status) || diagnosticUnconfirmed(record)
export const diagnosticCancellable = (record: DiagnosticRecord) => diagnosticActive(record) && record.agent_completed === false && record.status !== 'cancelled' && record.status !== 'cancel_requested'

export function diagnosticCancelError(current: { cancel_supported: boolean; reports: DiagnosticRecord[] } | undefined, id: string): string {
  if (!current?.cancel_supported) return '此 Agent 或服务后端不支持确认式取消，请先升级。'
  const record = current.reports.find(value => value.id === id)
  return !record || !diagnosticCancellable(record) ? '此诊断任务已不存在或状态已改变，请刷新后重新确认。' : ''
}
