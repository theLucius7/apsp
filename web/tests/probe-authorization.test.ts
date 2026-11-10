import { expect, test } from 'bun:test'
import { authorizationState, deleteLatencyTask, deleteServerProbe, initialProbePayloads, probePayloadError, probeState, saveLatencyTask, saveServerProbe } from '../src/observability/probes'
import type { LatencyTask, Probe, ProbeAuthorization, ProbeWriteSnapshot, TaskWriteSnapshot } from '../src/observability/probes'

const authorization: ProbeAuthorization = { kind: 'owned', source: '自有管理清单', scope: 'TEST_ONLY 管理确认：TCP 443，每30秒', enabled: true, expires_at: null, identity: { kind: 'tcp', target: '127.0.0.1', port: 443, address_family: 'any' } }
const probe: Probe = { id: 'probe-1', name: '回环目标', kind: 'tcp', target: '127.0.0.1', port: 443, interval_secs: 30, carrier: '', enabled: true, revision: 1, monitor: { region: '测试地区', address_family: 'any', authorization } }
const task: LatencyTask = { id: 'task-1', spec: probe, revision: 1, server_ids: [1], default_enabled: false }
const resource = <T,>(data: T[]) => ({ data, fresh: true, error: '' })
const single = (): ProbeWriteSnapshot => ({ serverId: 1, probes: resource([structuredClone(probe)]) })
const unified = (): TaskWriteSnapshot => ({ tasks: resource([structuredClone(task)]), servers: resource([{ id: 1 }, { id: 2 }]) })
const authorized = (value: Partial<ProbeAuthorization>): Probe => ({ ...probe, monitor: { ...probe.monitor!, authorization: { ...authorization, ...value } } })

test('explicit authorization is byte bounded and expires at the exact second', () => {
  expect(probePayloadError({ ...probe, monitor: null }, 100_000)).toContain('授权')
  expect(probePayloadError({ ...probe, enabled: false, monitor: null }, 100_000)).toBe('')
  for (const value of [{ scope: '' }, { source: '' }, { source: 'x\n' }, { scope: 'x\u0085y' }, { source: '源'.repeat(86) }, { scope: '证'.repeat(171) }, { expires_at: NaN }, { expires_at: 0 }, { expires_at: 100.5 }, { expires_at: Number.MAX_SAFE_INTEGER + 1 }, { enabled: false }]) expect(probePayloadError(authorized(value), 100_000)).not.toBe('')
  expect(probePayloadError({ ...probe, monitor: { ...probe.monitor!, region: '区'.repeat(22) } }, 100_000)).not.toBe('')
  expect(probePayloadError(authorized({ expires_at: 101 }), 100_000)).toBe('')
  expect(probePayloadError(authorized({ expires_at: 100 }), 100_000)).not.toBe('')
  expect(probePayloadError({ ...authorized({ expires_at: 100 }), enabled: false }, 100_000)).toBe('')
})

test('public execution status and historical zero samples never invent target authority', () => {
  const point = { id: 'result', probe_id: probe.id, sampled_at: 100_000, latency_ms: 0, loss_percent: 0, error: null }
  expect(probeState({ ...probe, monitor: null }, point, 101_000)).toBe('未取得执行授权')
  expect(probeState(authorized({ expires_at: 101 }), point, 101_000)).toBe('授权已过期')
  expect(probeState({ ...probe, monitor: null, execution_authorized: true }, point, 101_000)).toBe('最近采样')
  expect(probeState({ ...probe, execution_authorized: false }, point, 101_000)).toBe('未取得执行授权')
  expect(probeState(probe, point, 101_000, true)).toBe('状态未知')
  expect(authorizationState({ ...probe, target: 'other.example.com' }, 100_000)).toContain('重新确认')
})

test('direct single-server callbacks make zero writes on stale, failed or missing source reads', async () => {
  for (const kind of ['pending', 'failure', 'missing'] as const) {
    const snapshot = single(), writes: unknown[] = []
    snapshot.probes.fresh = kind === 'missing'
    snapshot.probes.error = kind === 'failure' ? 'TEST_ONLY 读取失败' : ''
    if (kind === 'missing') snapshot.probes.data = undefined
    await expect(saveServerProbe(snapshot, 1, probe, { ...probe, name: '保留草稿' }, async body => { writes.push(body) })).rejects.toThrow()
    await expect(deleteServerProbe(snapshot, 1, probe, async (...body) => { writes.push(body) })).rejects.toThrow()
    expect(writes).toEqual([])
    expect(probe.name).toBe('回环目标')
  }
})

test('fresh single-server callbacks recheck server, ID, immutable target, positive revision and task ownership', async () => {
  for (const kind of ['server', 'deleted', 'revision', 'unknown-version', 'zero-version', 'unsafe-version', 'target', 'task'] as const) {
    const snapshot = single(), writes: unknown[] = []
    if (kind === 'server') snapshot.serverId = 2
    if (kind === 'deleted') snapshot.probes.data = []
    if (kind === 'revision') snapshot.probes.data![0].revision = 2
    if (kind === 'unknown-version') delete snapshot.probes.data![0].revision
    if (kind === 'zero-version') snapshot.probes.data![0].revision = 0
    if (kind === 'unsafe-version') snapshot.probes.data![0].revision = Number.MAX_SAFE_INTEGER + 1
    if (kind === 'target') snapshot.probes.data![0].target = 'other.example.com'
    if (kind === 'task') snapshot.probes.data![0].task_id = 'task-1'
    await expect(saveServerProbe(snapshot, 1, probe, probe, async body => { writes.push(body) })).rejects.toThrow()
    await expect(deleteServerProbe(snapshot, 1, probe, async (...body) => { writes.push(body) })).rejects.toThrow()
    expect(writes).toEqual([])
  }
})

test('synchronously invalidated or replaced snapshots reject cached fresh closures', async () => {
  let valid = true, values = [structuredClone(probe)]
  const snapshot: ProbeWriteSnapshot = { serverId: 1, probes: { ...resource([probe]), isCurrent: () => valid, getCurrent: () => valid ? values : undefined } }
  const writes: unknown[] = []
  valid = false
  await expect(saveServerProbe(snapshot, 1, probe, probe, async body => { writes.push(body) })).rejects.toThrow()
  valid = true; values = []
  await expect(deleteServerProbe(snapshot, 1, probe, async (...body) => { writes.push(body) })).rejects.toThrow()
  expect(writes).toEqual([])
})

test('single-server writes preserve main monitor identity and send revision without derived execution flags', async () => {
  const writes: unknown[] = []
  await saveServerProbe(single(), 1, probe, { ...probe, name: '新名称', execution_authorized: true }, async body => { writes.push(body) })
  expect(Object.keys(writes[0] as object).sort()).toEqual(['monitor', 'carrier', 'enabled', 'id', 'interval_secs', 'kind', 'name', 'port', 'revision', 'target'].sort())
  expect((writes[0] as Probe).monitor?.authorization).toEqual(authorization)
  await deleteServerProbe(single(), 1, probe, async (id, body) => { writes.push({ id, ...body }) })
  expect(writes[1]).toEqual({ id: probe.id, revision: 1 })
  await saveServerProbe(single(), 1, probe, { ...probe, enabled: false, monitor: null }, async body => { writes.push(body) })
  expect((writes[2] as Probe).enabled).toBe(false)
})

test('all unified dependencies gate save/delete callbacks without discarding drafts', async () => {
  for (const dependency of ['tasks', 'servers'] as const) for (const kind of ['pending', 'failure', 'missing'] as const) {
    const snapshot = unified(), writes: unknown[] = []
    snapshot[dependency].fresh = kind === 'missing'
    snapshot[dependency].error = kind === 'failure' ? 'TEST_ONLY 读取失败' : ''
    if (kind === 'missing') snapshot[dependency].data = undefined
    await expect(saveLatencyTask(snapshot, task, task, async body => { writes.push(body) })).rejects.toThrow()
    await expect(deleteLatencyTask(snapshot, task, async (...body) => { writes.push(body) })).rejects.toThrow()
    expect(writes).toEqual([]); expect(task.server_ids).toEqual([1])
  }
})

test('unified writes reject removed selections and changed revisions while allowing offline configuration', async () => {
  for (const kind of ['server', 'revision', 'unknown-version', 'deleted', 'target'] as const) {
    const snapshot = unified(), writes: unknown[] = []
    if (kind === 'server') snapshot.servers.data = [{ id: 2 }]
    if (kind === 'revision') snapshot.tasks.data![0].revision = 2
    if (kind === 'unknown-version') snapshot.tasks.data![0].revision = 0
    if (kind === 'deleted') snapshot.tasks.data = []
    if (kind === 'target') snapshot.tasks.data![0].spec = { ...probe, target: 'other.example.com' }
    await expect(saveLatencyTask(snapshot, task, task, async body => { writes.push(body) })).rejects.toThrow()
    expect(writes).toEqual([])
  }
  const writes: unknown[] = []
  await saveLatencyTask(unified(), task, { ...task, server_ids: [1, 2] }, async body => { writes.push(body) })
  expect((writes[0] as LatencyTask).server_ids).toEqual([1, 2])
  expect((writes[0] as LatencyTask).spec.monitor?.authorization).toEqual(authorization)
  await deleteLatencyTask(unified(), task, async (id, body) => { writes.push({ id, ...body }) })
  expect(writes[1]).toEqual({ id: task.id, revision: 1 })
})

test('server enrollment builds authorized initial targets atomically without silently pausing incomplete drafts', () => {
  const incomplete = { ...probe, name: '第二目标', monitor: null }
  expect(() => initialProbePayloads([probe, incomplete])).toThrow()
  expect(incomplete.name).toBe('第二目标'); expect(incomplete.enabled).toBe(true)
  const result = initialProbePayloads([probe])
  expect(result[0].monitor?.authorization).toEqual(authorization)
  expect(Object.hasOwn(result[0], 'revision')).toBe(false)
  expect(() => initialProbePayloads(Array.from({ length: 33 }, () => probe))).toThrow('32')
})
