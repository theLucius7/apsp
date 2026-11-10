import { expect, test } from 'bun:test'
import { diagnosticActive, diagnosticCancelError } from '../src/observability/diagnostics'
import type { DiagnosticRecord } from '../src/types'

const record: DiagnosticRecord = { id: 'a0000000-0000-0000-0000-000000000145', status: 'failed', agent_completed: false, cancel_requested_at: null, cancel_error: null, job: { plugin: 'nodequality', options: { ip_version: 'ipv4' } }, report: { text: 'TEST_ONLY retained report' }, error: 'TEST_ONLY panel deadline', created_at: 1, updated_at: 2, expires_at: 2 }

test('unconfirmed results continue occupying diagnostics while text-only history remains readable', () => {
  expect(diagnosticActive(record)).toBe(false)
  for (const id of [record.id, record.id.toUpperCase(), null, 'TEST_ONLY malformed identity'])
    expect(diagnosticActive({ ...record, job: { ...record.job, id } })).toBe(true)
  expect(diagnosticActive({ ...record, cleanup_pending: true })).toBe(true)
  expect(diagnosticActive({ ...record, cleanup_pending: false, job: { ...record.job, id: record.id } })).toBe(false)
  for (const status of ['queued', 'running', 'cleaning', 'cancel_requested'] as const)
    expect(diagnosticActive({ ...record, status })).toBe(true)
  expect(diagnosticActive({ ...record, agent_completed: true, job: { ...record.job, id: record.id } })).toBe(false)
})

test('cancellation depends on its current capability and entity state independently of new task readiness', () => {
  const current = { cancel_supported: true, reports: [{ ...record, cleanup_pending: true }] }
  expect(diagnosticCancelError(current, record.id)).toBe('')
  expect(diagnosticCancelError({ ...current, cancel_supported: false }, record.id)).not.toBe('')
  expect(diagnosticCancelError({ ...current, reports: [] }, record.id)).not.toBe('')
  for (const report of [{ ...record, status: 'cancel_requested' as const }, { ...record, agent_completed: true }])
    expect(diagnosticCancelError({ ...current, reports: [report] }, record.id)).not.toBe('')
})
