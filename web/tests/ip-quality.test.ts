import { expect, test } from 'bun:test'
import { nodeIpCancelError, nodeIpStartError } from '../src/observability/ip-quality'
import type { DiagnosticRecord, NodeIpQuality } from '../src/types'

const record: DiagnosticRecord = { id: 'job', status: 'running', agent_completed: false, cancel_requested_at: null, cancel_error: null, job: { plugin: 'ipquality', options: { ip_version: '4' } }, report: null, error: null, created_at: 1, updated_at: 1, expires_at: 100 }
const ready: NodeIpQuality = { ready: true, reason: null, version: 'fixed', reports: [], cancel_supported: true, observed_egress_ips: [], current_egress_ips: [] }
const scope = { serverId: 1, fresh: true, error: '', data: ready }

test('node IP creation rechecks server, freshness, capabilities and active ownership at callback time', () => {
  expect(nodeIpStartError(scope, 1)).toBe('')
  expect(nodeIpStartError(scope, 2)).not.toBe('')
  expect(nodeIpStartError({ ...scope, fresh: false }, 1)).not.toBe('')
  expect(nodeIpStartError({ ...scope, error: 'failed read' }, 1)).not.toBe('')
  expect(nodeIpStartError({ ...scope, data: undefined }, 1)).not.toBe('')
  expect(nodeIpStartError({ ...scope, data: { ...ready, ready: false } }, 1)).not.toBe('')
  for (const status of ['queued', 'running', 'cleaning', 'cancel_requested'] as const) {
    expect(nodeIpStartError({ ...scope, data: { ...ready, reports: [{ ...record, status }] } }, 1)).not.toBe('')
  }
})

test('node IP cancellation cannot use removed, changed, confirmed or cross-server reports', () => {
  const active = { ...scope, data: { ...ready, reports: [record] } }
  expect(nodeIpCancelError(active, 1, record.id)).toBe('')
  expect(nodeIpCancelError(active, 2, record.id)).not.toBe('')
  expect(nodeIpCancelError({ ...active, fresh: false }, 1, record.id)).not.toBe('')
  expect(nodeIpCancelError(scope, 1, record.id)).not.toBe('')
  for (const change of [{ agent_completed: true }, { status: 'cancel_requested' as const }, { status: 'cancelled' as const }, { status: 'failed' as const }, { status: 'succeeded' as const }, { job: { ...record.job, plugin: 'nodequality' } }]) {
    expect(nodeIpCancelError({ ...active, data: { ...active.data, reports: [{ ...record, ...change }] } }, 1, record.id)).not.toBe('')
  }
  expect(nodeIpCancelError({ ...active, data: { ...active.data, cancel_supported: false } }, 1, record.id)).not.toBe('')
})


test('unconfirmed terminal IP reports retain cleanup ownership and support capability-based cancellation', () => {
  for (const status of ['failed', 'succeeded'] as const) {
    const pending = { ...record, status, cleanup_pending: true, job: { ...record.job, id: record.id } }
    const current = { ...scope, data: { ...ready, ready: false, reports: [pending] } }
    expect(nodeIpStartError(current, 1)).not.toBe('')
    expect(nodeIpCancelError(current, 1, record.id)).toBe('')
    expect(nodeIpCancelError({ ...current, data: { ...current.data, cancel_supported: false } }, 1, record.id)).not.toBe('')
    expect(nodeIpStartError({ ...scope, data: { ...ready, reports: [{ ...pending, agent_completed: true, cleanup_pending: false }] } }, 1)).toBe('')
  }
})
