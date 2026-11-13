import { expect, test } from 'bun:test'
import { validatedSnapshot } from '../src/lib/resource-snapshot'
import { adoptSourceNode, cancelSourceJob, deleteSource, emptySourceDraft, prepareSourceMutation, previewCommit, previewInput, refreshSource, SourceFileReader, sourceAuthHeaders, sourceCommand, sourceMetadataError, sourceMutationReplay, sourceWriteError, submitSourceMutation } from '../src/traffic/sourceRequests'
import type { SourceSnapshot } from '../src/traffic/sourceRequests'
import { MAX_SOURCE_BYTES, orderedPreviewError, sourceInterval, sourceJobActive, sourceRoot, sourceStatusText, validOrderedPreview, validSourceCounts, validSourceHistory, validSourceJob, validSourceNodePage, validSubscriptionSource, validSubscriptionSources } from '../src/traffic/orderedSourceTypes'
import type { OrderedPreview, SourceJob, SourceNode, SourceNodePage, SubscriptionSource } from '../src/traffic/orderedSourceTypes'
import { sourceJobFixture, sourceNodeFixture, sourceNodePageFixture, sourcePreviewFixture, sourceRevisionFixture, sourceUuid, subscriptionSourceFixture } from './subscription-source-fixtures.mjs'
const source = () => subscriptionSourceFixture() as SubscriptionSource
const snapshot = (): SourceSnapshot => ({ data: [source()], fresh: true, error: '' })
const id = sourceUuid(900)
const create = () => sourceCommand('create', { ...emptySourceDraft(), name: '来源', url: 'https://subscription.example.com/private?token=TEST_ONLY', authorization: 'Bearer TEST_ONLY' })
const receipt = { source_id: 1, settings_revision: 1, identity_epoch: 1, job_id: sourceUuid(201) }

test('public source decoder rejects secret fields, malformed revisions and duplicate source identities', () => {
  expect(validSubscriptionSource(source())).toBe(true)
  for (const key of ['url', 'content', 'input_config', 'auth_headers', 'raw_digest']) expect(validSubscriptionSource({ ...source(), [key]: 'TEST_ONLY' })).toBe(false)
  expect(validSubscriptionSource({ ...source(), latest_success: { ...source().latest_success!, input_config: {} } })).toBe(false)
  expect(validSubscriptionSources([source(), source()])).toBe(false)
  expect(validSubscriptionSource({ ...source(), host: 'host.example/private?token=TEST_ONLY' })).toBe(false)
  expect(validSubscriptionSource({ ...source(), dependencies: [{ secret: 'unknown future schema' }] })).toBe(false)
  expect(validSubscriptionSource({ ...source(), active_job: sourceJobFixture(2) })).toBe(false)
})
test('metadata revisions do not invalidate same-epoch success; immutable versions may belong to older batches', () => {
  expect(validSourceCounts({ supported: 1, unsupported: 0, ambiguous: 0, missing: 10000 })).toBe(true)
  expect(validSourceCounts({ supported: 5001, unsupported: 0, ambiguous: 0, missing: 0 })).toBe(false)
  const item = { ...source(), settings_revision: 3 }
  expect(validSubscriptionSource(item)).toBe(true)
  const nodes = sourceNodePageFixture({ current_settings_revision: 3, success_revision: sourceRevisionFixture(1, { id: sourceUuid(105), settings_revision: 2 }), nodes: [sourceNodeFixture({ source_revision_id: sourceUuid(101) })] })
  expect(validSourceNodePage(nodes)).toBe(true)
  expect(sourceWriteError({ data: [item], fresh: true, error: '' }, 1, 1)).toContain('设置已变化')
})
test('unknown, unsupported, missing and ambiguous node states remain distinct without invented endpoints', () => {
  const nodes = [sourceNodeFixture({ present_in_latest: false, selectable: false, reasons: ['当前批次缺失'] }), sourceNodeFixture({ id: sourceUuid(302), version_id: sourceUuid(402), ordinal: 1, supported: false, selectable: false, identity_state: 'unresolved', parse_status: 'unsupported', protocol: null, server: null, server_port: null, sni: null, transport: null, capabilities: { tcp: false, udp: false }, unsupported_reasons: [{ code: 'unsupported', message: '协议不支持' }] }), sourceNodeFixture({ id: sourceUuid(303), version_id: sourceUuid(403), ordinal: 2, identity_state: 'ambiguous', selectable: false })]
  expect(validSourceNodePage(sourceNodePageFixture({ nodes }))).toBe(true)
  expect(validSourceNodePage(sourceNodePageFixture({ nodes: [{ ...nodes[1], server_port: 0 }] }))).toBe(false)
  expect(validSourceNodePage(sourceNodePageFixture({ nodes: [{ ...nodes[2], selectable: true }] }))).toBe(false)
  expect(validSourceNodePage(sourceNodePageFixture({ nodes: [{ ...nodes[0], normalized_config: { password: 'TEST_ONLY' } }] }))).toBe(false)
  expect(validSourceNodePage(sourceNodePageFixture({ nodes: [nodes[0], nodes[0]] }))).toBe(false)
  expect(validSourceNodePage(sourceNodePageFixture({ nodes: [{ ...nodes[0], identity_epoch: 2, selectable: true, present_in_latest: true }] }))).toBe(false)
})
test('history decoder binds revisions to a source and does not manufacture selectability', () => {
  expect(validSourceHistory({ source_id: 1, revisions: [sourceRevisionFixture()] })).toBe(true)
  expect(validSourceHistory({ source_id: 1, revisions: [sourceRevisionFixture(2)] })).toBe(false)
  const old = sourceNodePageFixture({ nodes: [sourceNodeFixture({ selectable: false, reasons: ['历史批次仅供查看'] })] })
  expect(validSourceNodePage(old)).toBe(true)
  expect((old as SourceNodePage).nodes[0].selectable).toBe(false)
})
test('failed or malformed fresh reads retain known public history but block all mutations', async () => {
  const previous = [source()]
  const stale = validatedSnapshot({ data: [{ id: 1, name: 'legacy' }], fresh: true, error: '' }, validSubscriptionSources, previous)
  expect(stale.data).toBe(previous); expect(stale.fresh).toBe(false)
  const writes: unknown[] = [], writer = async (...args: unknown[]) => { writes.push(args); return receipt }
  for (const current of [{ ...snapshot(), fresh: false }, { ...snapshot(), error: '403' }, stale]) {
    expect(() => prepareSourceMutation(create(), current, undefined, () => id)).toThrow()
    await expect(refreshSource(current, source(), writer)).rejects.toThrow()
    await expect(deleteSource(current, source(), writer)).rejects.toThrow()
  }
  expect(writes).toHaveLength(0)
})
test('lost create response replays the same exact body and key after the source becomes visible', async () => {
  const current = snapshot(), pending = prepareSourceMutation(create(), current, undefined, () => id), sent: string[] = []
  await expect(submitSourceMutation(pending, current, async (_path, _method, body) => { sent.push(JSON.stringify(body)); throw new Error('response lost') })).rejects.toThrow('response lost')
  current.data = [{ ...source(), settings_revision: 9, archived: true }]
  const retry = prepareSourceMutation(create(), current, pending, () => { throw new Error('new key must not be allocated') })
  expect(retry).toBe(pending); expect(sourceMutationReplay(create(), pending)).toBe(true)
  await submitSourceMutation(retry, current, async (_path, _method, body) => { sent.push(JSON.stringify(body)); return receipt })
  expect(sent).toEqual([pending.serialized, pending.serialized])
  const changed = { ...create(), fields: { ...create().fields, name: 'new draft' } }
  expect(prepareSourceMutation(changed, current, pending, () => sourceUuid(901)).request_id).not.toBe(id)
})
test('lost PATCH response permits same-identity CAS advancement but blocks archive, deletion and replacement', async () => {
  const command = sourceCommand('metadata', { ...emptySourceDraft(), name: 'changed' }, source()), pending = prepareSourceMutation(command, snapshot(), undefined, () => id), sent: string[] = []
  await expect(submitSourceMutation(pending, snapshot(), async (_path, _method, body) => { sent.push(JSON.stringify(body)); throw new Error('lost') })).rejects.toThrow()
  for (const change of ['archived', 'deleted', 'epoch', 'pending'] as const) {
    const current = snapshot(); current.data![0].settings_revision = 2
    if (change === 'archived') current.data![0].archived = true
    if (change === 'deleted') current.data = []
    if (change === 'epoch') current.data![0].identity_epoch = 2
    if (change === 'pending') current.fresh = false
    expect(() => prepareSourceMutation(command, current, pending, () => id)).toThrow()
    await expect(submitSourceMutation(pending, current, async (_path, _method, body) => { sent.push(JSON.stringify(body)); return receipt })).rejects.toThrow()
    expect(pending.serialized).toBe(sent[0]); expect(pending.command).toBe(JSON.stringify(command))
  }
  expect(sent).toHaveLength(1)
  const current = snapshot(); current.data![0].settings_revision = 2
  expect(prepareSourceMutation(command, current, pending, () => { throw new Error('new key forbidden') })).toBe(pending)
  await submitSourceMutation(pending, current, async (_path, _method, body) => { sent.push(JSON.stringify(body)); return { ...receipt, settings_revision: 2, job_id: null } })
  expect(sent).toEqual([pending.serialized, pending.serialized]); expect(sent[0]).not.toContain('expected_identity_epoch')
  expect(() => prepareSourceMutation({ ...command, fields: { name: 'another' } }, current, pending, () => sourceUuid(902))).toThrow('设置已变化')
})
test('request integrity and receipt source identity are checked before clearing unknown outcomes', async () => {
  const current = snapshot(), pending = prepareSourceMutation(create(), current, undefined, () => id), writes: unknown[] = []
  await expect(submitSourceMutation({ ...pending, serialized: pending.serialized.replace('TEST_ONLY', 'CHANGED') }, current, async body => { writes.push(body); return receipt })).rejects.toThrow('草稿不一致')
  expect(writes).toHaveLength(0)
  await expect(submitSourceMutation(pending, current, async () => ({ ...receipt, secret: 'forbidden' }))).rejects.toThrow('收据尚未确认')
  expect(pending.attempted).toBe(true)
  const update = prepareSourceMutation(sourceCommand('metadata', { ...emptySourceDraft(), name: 'name' }, source()), current, undefined, () => sourceUuid(903))
  await expect(submitSourceMutation(update, current, async () => ({ ...receipt, source_id: 99 }))).rejects.toThrow('收据尚未确认')
})
test('URL auth is restricted to three lowercase headers and bounded by UTF-8 bytes', () => {
  expect(sourceAuthHeaders({ authorization: 'Bearer x', cookie: 'x=y', apiKey: 'key' })).toEqual({ authorization: 'Bearer x', cookie: 'x=y', 'x-api-key': 'key' })
  for (const bad of [' ', 'a\rb', 'a\nb', 'a\0b']) expect(() => sourceAuthHeaders({ authorization: bad, cookie: '', apiKey: '' })).toThrow()
  expect(() => sourceAuthHeaders({ authorization: '中'.repeat(3000), cookie: '', apiKey: '' })).toThrow('8 KiB')
  for (const url of ['http://example.com/', 'https://user:pass@example.com/', 'https://example.com/#secret']) expect(() => sourceCommand('create', { ...emptySourceDraft(), name: 'a', url })).toThrow('HTTPS')
})
test('inline update and replacement carry explicit identity intent; secret metadata is never inferred from GET', () => {
  const item = { ...source(), kind: 'inline' as const, host: null, refresh_interval_secs: 0 }, draft = { ...emptySourceDraft(), kind: 'inline' as const, name: 'a', content: 'trojan://TEST_ONLY@exit.example.com:443' }
  expect(sourceCommand('update', draft, item).fields.input).toEqual({ kind: 'inline', content: draft.content, identity_action: 'update' })
  expect(sourceCommand('replace', draft, item).fields.input).toEqual({ kind: 'inline', content: draft.content, identity_action: 'replace' })
  expect(sourceCommand('metadata', draft, item).fields).toEqual({ name: 'a' })
  expect(() => sourceCommand('update', draft, source())).toThrow('同一来源')
  expect(() => sourceCommand('create', { ...draft, name: 'https://not-a-name.example' })).toThrow('不能包含链接')
  expect(() => sourceCommand('create', { ...draft, name: 'a'.repeat(129) })).toThrow('128')
  expect(() => sourceCommand('create', { ...draft, content: '中'.repeat(MAX_SOURCE_BYTES / 2) })).toThrow('2 MiB')
  expect((sourceCommand('replace', { ...emptySourceDraft(), name: 'a', authAction: 'clear' }, source()).fields.input as { auth_headers: unknown }).auth_headers).toEqual({ action: 'clear' })
})
test('refresh and delete bind current settings, and dependency conflicts propagate without local removal', async () => {
  const current = snapshot(), paths: string[] = [], job = sourceJobFixture() as SourceJob
  await expect(refreshSource(current, source(), async (path, method, body) => { paths.push(path); expect(method).toBe('POST'); expect(body).toEqual({ settings_revision: 1 }); return job })).resolves.toEqual(job)
  await expect(deleteSource(current, source(), async () => { throw new Error('409 当前路径 #7 引用') })).rejects.toThrow('当前路径 #7')
  expect(current.data).toHaveLength(1)
  current.data![0].archived = true
  await expect(refreshSource(current, source(), async () => job)).rejects.toThrow('归档')
  current.data![0].settings_revision = 2
  await expect(deleteSource(current, source(), async () => undefined)).rejects.toThrow('设置已变化')
  expect(paths).toHaveLength(1)
  const inline = { ...source(), kind: 'inline' as const, host: null, refresh_interval_secs: 0 }
  await expect(refreshSource({ data: [inline], fresh: true, error: '' }, inline, async () => job)).rejects.toThrow('显式更新内容')
})
test('cancellation is pending until the backend returns a terminal status', async () => {
  const job = sourceJobFixture(1, { status: 'running', stage: 'fetch' }) as SourceJob, current = { data: job, fresh: true, error: '' }
  const pending = await cancelSourceJob(current, job.id, async () => ({ ...job, status: 'cancelling' }))
  expect(sourceJobActive(pending)).toBe(true); expect(sourceStatusText[pending.status]).toBe('等待取消确认')
  const cancelled = { ...job, status: 'cancelled' as const, stage: 'done' as const, finished_at: 1790860801 }
  expect(validSourceJob(cancelled)).toBe(true); expect(sourceJobActive(cancelled)).toBe(false)
  await expect(cancelSourceJob({ ...current, fresh: false }, job.id, async () => cancelled)).rejects.toThrow()
  await expect(cancelSourceJob({ ...current, data: cancelled }, job.id, async () => cancelled)).rejects.toThrow('已结束')
})
test('file reader refuses oversized files before reading and rejects invalid UTF-8', async () => {
  const reader = new SourceFileReader(), bytes = new Uint8Array([0xff]).buffer
  let reads = 0
  await expect(reader.read({ size: MAX_SOURCE_BYTES + 1, arrayBuffer: async () => { reads++; return bytes } })).rejects.toThrow('2 MiB')
  expect(reads).toBe(0)
  await expect(reader.read({ size: 1, arrayBuffer: async () => bytes })).rejects.toThrow('UTF-8')
})
test('late file results and late file errors cannot repopulate a replaced or cleared draft', async () => {
  const reader = new SourceFileReader()
  let finish!: (bytes: ArrayBuffer) => void, fail!: (error: Error) => void
  const first = reader.read({ size: 4, arrayBuffer: () => new Promise(resolve => { finish = resolve }) })
  reader.invalidate(); finish(new TextEncoder().encode('late').buffer)
  await expect(first).resolves.toBeNull()
  const second = reader.read({ size: 1, arrayBuffer: () => new Promise((_resolve, reject) => { fail = reject }) })
  reader.invalidate(); fail(new Error('late file error'))
  await expect(second).resolves.toBeNull()
  expect(sourceMetadataError(snapshot())).toBe('')
})
test('merged source settings carry the User-Agent, automatic refresh and the widened interval', () => {
  const item = { ...source(), user_agent: 'Old/1' }
  expect(validSubscriptionSource(item)).toBe(true)
  for (const secs of [299, 2592001]) expect(validSubscriptionSource({ ...source(), refresh_interval_secs: secs })).toBe(false)
  for (const secs of [300, 2592000]) expect(validSubscriptionSource({ ...source(), refresh_interval_secs: secs })).toBe(true)
  for (const key of ['user_agent', 'auto_refresh', 'traffic', 'changes']) { const copy: Record<string, unknown> = { ...source() }; delete copy[key]; expect(validSubscriptionSource(copy)).toBe(false) }
  expect(validSubscriptionSource({ ...source(), traffic: { upload: 1, download: 2, total: 3, expire: 4, updated_at: 5 } })).toBe(true)
  expect(validSubscriptionSource({ ...source(), traffic: { upload: 1, secret: 2 } })).toBe(false)
  expect(sourceInterval(300)).toBe('每 5 分钟'); expect(sourceInterval(7200)).toBe('每 2 小时'); expect(sourceInterval(2592000)).toBe('每 30 天')
  const created = sourceCommand('create', { ...emptySourceDraft(), name: 'a', url: 'https://subscription.example.com/s', userAgent: ' Agent/1 ', autoRefresh: 'off', interval: '300' }).fields
  expect(created).toMatchObject({ user_agent: 'Agent/1', auto_refresh: false, refresh_interval_secs: 300 })
  expect(sourceCommand('create', { ...emptySourceDraft(), name: 'a', url: 'https://subscription.example.com/s' }).fields.user_agent).toBeNull()
  const draft = { ...emptySourceDraft(), name: 'a', userAgent: 'Old/1' }
  expect(sourceCommand('metadata', draft, item).fields).toEqual({ name: 'a', refresh_interval_secs: 86400, auto_refresh: true })
  expect(sourceCommand('metadata', { ...draft, userAgent: 'New/2' }, item).fields.user_agent).toEqual({ action: 'replace', value: 'New/2' })
  expect(sourceCommand('metadata', { ...draft, userAgent: '' }, item).fields.user_agent).toEqual({ action: 'clear' })
  expect(sourceCommand('metadata', { ...draft, autoRefresh: 'off' }, item).fields.auto_refresh).toBe(false)
  for (const bad of ['中文', 'a\tb', 'x'.repeat(257)]) expect(() => sourceCommand('metadata', { ...draft, userAgent: bad }, item)).toThrow('请求标识')
  for (const interval of ['299', '2592001', '1.5']) expect(() => sourceCommand('metadata', { ...draft, interval }, item)).toThrow('刷新周期')
})
test('node pages expose the public id, adoption and the shared catalog revision', () => {
  for (const key of ['public_id', 'adopted', 'metadata_revision']) { const node: Record<string, unknown> = { ...sourceNodeFixture() }; delete node[key]; expect(validSourceNodePage(sourceNodePageFixture({ nodes: [node] }))).toBe(false) }
  expect(validSourceNodePage(sourceNodePageFixture({ nodes: [sourceNodeFixture({ public_id: 0 })] }))).toBe(false)
  expect(validSourceNodePage(sourceNodePageFixture({ nodes: [sourceNodeFixture({ adopted: true, metadata_revision: 3 })] }))).toBe(true)
})
test('catalog adoption sends the current version and the shared metadata revision', async () => {
  const current = snapshot(), node = sourceNodeFixture() as SourceNode, sent: unknown[] = []
  const adopted = await adoptSourceNode(current, source(), node, true, async (path: string, method: string, body?: unknown) => { sent.push([path, method, body]); return { ...node, adopted: true, metadata_revision: 1 } })
  expect(adopted.adopted).toBe(true)
  expect(sent).toEqual([[`${sourceRoot}/1/nodes/${node.id}`, 'PATCH', { adopted: true, settings_revision: 1, identity_epoch: 1, node_version_id: node.version_id, metadata_revision: 0 }]])
  await expect(adoptSourceNode(current, source(), { ...node, selectable: false }, true, async () => node)).rejects.toThrow('不可加入')
  await expect(adoptSourceNode(current, source(), node, true, async () => ({ ...node, adopted: false }))).rejects.toThrow('尚未确认')
  await expect(adoptSourceNode({ ...current, fresh: false }, source(), node, true, async () => node)).rejects.toThrow()
  // Removing from the catalog stays possible on an archived source; adopting does not.
  const archived = { ...source(), archived: true }, view = { data: [archived], fresh: true, error: '' }
  await expect(adoptSourceNode(view, archived, { ...node, adopted: true, selectable: false }, false, async () => ({ ...node, adopted: false }))).resolves.toMatchObject({ adopted: false })
  await expect(adoptSourceNode(view, archived, node, true, async () => node)).rejects.toThrow('归档')
  expect(sent).toHaveLength(1)
})
test('preview import commits only selectable nodes with the chosen name, interval and request id', () => {
  const preview = sourcePreviewFixture({ expires_at: 2000 }) as OrderedPreview
  expect(validOrderedPreview(preview)).toBe(true)
  expect(validOrderedPreview({ ...preview, nodes: [preview.nodes[0], { ...preview.nodes[1], selectable: true }] })).toBe(false)
  expect(validOrderedPreview({ ...preview, input_config: {} })).toBe(false)
  expect(validOrderedPreview({ ...preview, supported_count: 2 })).toBe(false)
  const draft = { ...emptySourceDraft(), name: '来源' }
  expect(previewCommit(preview, draft, ['node-0'], id, 1_000_000)).toEqual({ request_id: id, name: '来源', selected: ['node-0'], refresh_interval_secs: 86400, auto_refresh: true })
  expect(previewCommit(preview, { ...draft, kind: 'inline' }, ['node-0'], id, 1_000_000)).toMatchObject({ refresh_interval_secs: null, auto_refresh: false })
  for (const selected of [['node-1'], [], ['node-0', 'node-0']]) expect(() => previewCommit(preview, draft, selected, id, 1_000_000)).toThrow('可导入')
  expect(() => previewCommit(preview, draft, ['node-0'], id, 2_000_000)).toThrow('过期')
  expect(() => previewCommit(preview, { ...draft, name: 'https://x.example' }, ['node-0'], id, 1_000_000)).toThrow('来源名称')
  expect(orderedPreviewError(preview, ['node-0'], 1_000_000)).toBe('')
  expect(previewInput({ ...draft, url: 'https://subscription.example.com/s', userAgent: 'A/1' })).toEqual({ input: { kind: 'url', url: 'https://subscription.example.com/s', auth_headers: {} }, user_agent: 'A/1' })
  expect(previewInput({ ...draft, kind: 'inline', content: 'trojan://x@exit.example.com:443', userAgent: 'ignored' })).toEqual({ input: { kind: 'inline', content: 'trojan://x@exit.example.com:443' }, user_agent: null })
})
