import { expect, test } from 'bun:test'
import { prepareChainBatch, submitChainBatch } from '../src/plugins/singbox/Chains'
import type { ChainBatchDraft, ChainDraftHop, ProxyWriteSnapshot } from '../src/plugins/singbox/Chains'
import { moveChainHop } from '../src/plugins/singbox/ChainPathEditor'
import { expandChainExits } from '../src/plugins/singbox/ChainExitExpansion'
import { chainMutationError, prepareChainMutation, submitChainMutation } from '../src/plugins/singbox/chainRequests'
import type { ChainMutation } from '../src/plugins/singbox/chainRequests'
import { filterProxyResources, validProxyResource, validatedSnapshot } from '../src/plugins/singbox/groupTypes'
import type { ProxyResource, PublicHop } from '../src/plugins/singbox/groupTypes'
import { nodeRoute } from '../src/plugins/singbox/nodeRoute'
import { validSubscriptionSource } from '../src/plugins/singbox/orderedSourceTypes'
import { orderedResourceFixture, pathFixtureUuid, proxyResourceFixtures } from './proxy-resource-fixtures.mjs'
import { sourceNodeFixture, sourceNodePageFixture, sourceUuid, subscriptionSourceFixture } from './subscription-source-fixtures.mjs'

const requestId = sourceUuid(900), nextId = sourceUuid(901)
function snapshot(): ProxyWriteSnapshot {
  const nodes = [1, 2, 3].map(id => ({ id, name: `节点 ${id}`, server_id: id, protocol: 'vless-reality', enabled: true, port: 20000 + id, public_host: `proxy${id}.example.com`, sni: 'www.example.com', public_key: 'TEST_ONLY', short_id: 'abcd' }))
  const servers = [1, 2, 3].map(id => ({ id, name: `服务器 ${id}`, enabled: true, online: false, read_only: false }))
  return { nodes: { data: nodes, fresh: true, error: '' }, servers: { data: servers, fresh: true, error: '' }, resources: { data: proxyResourceFixtures(nodes, servers), fresh: true, error: '' }, sources: { data: [subscriptionSourceFixture()], fresh: true, error: '' }, sourceNodes: { 1: { data: sourceNodePageFixture(), fresh: true, error: '' } } }
}
function external(mode: 'follow_node' | 'pinned' = 'follow_node'): ChainDraftHop { return { kind: 'subscription', source_id: 1, node: sourceNodeFixture(), update_mode: mode } }
function draft(): ChainBatchDraft { return { mode: 'new', server_id: '1', public_host: 'entry.example.com', sni: 'www.example.com', entry_node_id: '', exit_node_id: '', hops: [external(), { kind: 'managed', node_id: '2' }], rows: [{ name: '混合甲', port: '' }, { name: '混合乙', port: '24443', hops: [{ kind: 'managed', node_id: '3' }, external('pinned'), { kind: 'managed', node_id: '2' }] }] } }
function publicHops(current = snapshot()): PublicHop[] { return [{ kind: 'subscription', position: 1, source_id: 1, source_name: '示例订阅', identity_epoch: 1, external_node_id: sourceUuid(301), node_version_id: sourceUuid(401), source_revision_id: sourceUuid(101), update_mode: 'pinned', name: '外部示例节点', protocol: 'trojan', server: 'exit.example.com', server_port: 443, sni: 'exit.example.com', transport: 'tcp', capabilities: { tcp: true, udp: true }, source_archived: false, node_present: true, update_error: null }, { kind: 'managed', position: 2, node_id: 2, endpoint_version_id: pathFixtureUuid(2), endpoint: current.resources.data![1].entry }] }
function ordered(current = snapshot()): ProxyResource { return orderedResourceFixture({ id: 10, name: '混合资源', entry: current.resources.data![0].entry, hops: publicHops(current) }) as ProxyResource }

test('ordered batch preserves middle subscription and independent per-row full paths without credentials', async () => {
  const pending = prepareChainBatch(draft(), snapshot(), undefined, () => requestId), writes: unknown[] = []
  await submitChainBatch(pending, snapshot(), async body => { writes.push(body) })
  expect((writes[0] as { items: { hops: unknown[] }[] }).items.map(item => item.hops)).toEqual([
    [{ kind: 'subscription', source_id: 1, external_node_id: sourceUuid(301), node_version_id: sourceUuid(401), update_mode: 'follow_node' }, { kind: 'managed', node_id: 2 }],
    [{ kind: 'managed', node_id: 3 }, { kind: 'subscription', source_id: 1, external_node_id: sourceUuid(301), node_version_id: sourceUuid(401), update_mode: 'pinned' }, { kind: 'managed', node_id: 2 }],
  ])
  expect(pending.serialized).not.toContain('password'); expect(pending.serialized).not.toContain('server_port'); expect(pending.serialized).not.toContain('url')
})
test('move and explicit exit expansion preserve each actual order and do not build a Cartesian product', () => {
  const value = draft(), hops = value.hops!
  expect(moveChainHop(hops, 0, 1)).toEqual([hops[1], hops[0]])
  expect(moveChainHop(hops, -1, 1)).toBe(hops)
  const expanded = expandChainExits(value, [{ kind: 'managed', node_id: '2' }, { kind: 'managed', node_id: '3' }])
  expect(expanded.rows).toHaveLength(2); expect(expanded.rows.map(row => row.hops)).toEqual([[hops[0], { kind: 'managed', node_id: '2' }], [hops[0], { kind: 'managed', node_id: '3' }]])
  expect(expanded.rows.map(row => row.port)).toEqual(['', '']); expect(expanded.rows[0].hops).not.toBe(expanded.rows[1].hops)
  expect(() => expandChainExits({ ...value, mode: 'existing' }, [hops[0], hops[1]])).toThrow('已有入口')
})
test('new selection refuses missing, ambiguous, archived, new epoch, historical and fresh-version drift without automatic replacement', () => {
  for (const reason of ['missing', 'ambiguous', 'archive', 'epoch', 'history', 'version', 'pending', 'failure'] as const) {
    const current = snapshot(), page = current.sourceNodes![1], node = page.data!.nodes[0]
    if (reason === 'missing') { node.present_in_latest = false; node.selectable = false }
    if (reason === 'ambiguous') { node.identity_state = 'ambiguous'; node.selectable = false }
    if (reason === 'archive') current.sources!.data![0].archived = true
    if (reason === 'epoch') current.sources!.data![0].identity_epoch = 2
    if (reason === 'history') node.selectable = false
    if (reason === 'version') node.version_id = sourceUuid(405)
    if (reason === 'pending') page.fresh = false
    if (reason === 'failure') page.error = '超时，旧预览保留'
    const value = draft(), original = JSON.stringify(value)
    expect(() => prepareChainBatch(value, current, undefined, () => requestId)).toThrow(); expect(JSON.stringify(value)).toBe(original)
  }
})
test('path limits, repeat logical identities and repeated managed servers refuse before POST', () => {
  for (const hops of [[], Array.from({ length: 9 }, () => external()), [external(), external()], [{ kind: 'managed', node_id: '1' }], [{ kind: 'managed', node_id: '2' }, { kind: 'managed', node_id: '2' }]] as ChainDraftHop[][]) {
    const value = draft(); value.hops = hops; value.rows = [value.rows[0]]
    expect(() => prepareChainBatch(value, snapshot(), undefined, () => requestId)).toThrow()
  }
})
test('exact batch replay requires the original source identity and selected version to remain eligible', async () => {
  const value = draft(), pending = prepareChainBatch(value, snapshot(), undefined, () => requestId), bodies: string[] = []
  await expect(submitChainBatch(pending, snapshot(), async body => { bodies.push(JSON.stringify(body)); throw new Error('response lost') })).rejects.toThrow()
  for (const change of ['deleted', 'archived', 'epoch', 'node', 'version', 'pending'] as const) {
    const current = snapshot()
    if (change === 'deleted') current.sources!.data = []
    if (change === 'archived') current.sources!.data![0].archived = true
    if (change === 'epoch') current.sources!.data![0].identity_epoch = 2
    if (change === 'node') current.sourceNodes![1].data!.nodes = []
    if (change === 'version') current.sourceNodes![1].data!.nodes[0].version_id = sourceUuid(405)
    if (change === 'pending') current.sourceNodes![1].fresh = false
    expect(() => prepareChainBatch(value, current, pending, () => { throw new Error('new key forbidden') })).toThrow()
    await expect(submitChainBatch(pending, current, async body => { bodies.push(JSON.stringify(body)) })).rejects.toThrow()
    const changed = structuredClone(value); changed.rows[0].name = '修改名称'
    expect(() => prepareChainBatch(changed, current, pending, () => nextId)).toThrow()
    expect(pending.source_draft).toBe(JSON.stringify(value)); expect(pending.serialized).toBe(bodies[0])
  }
  expect(bodies).toHaveLength(1)
  const replay = prepareChainBatch(value, snapshot(), pending, () => { throw new Error('new key forbidden') })
  await submitChainBatch(replay, snapshot(), async body => { bodies.push(JSON.stringify(body)) })
  expect(bodies).toEqual([pending.serialized, pending.serialized])
})
test('ordered public projection exposes every managed position and independent frozen applied vector, rejecting secrets and missing topology', () => {
  const current = snapshot(), resource = ordered(current)
  expect(validProxyResource(resource)).toBe(true)
  expect(filterProxyResources([resource], 'chains', 2).map(value => value.id)).toEqual([10])
  expect(filterProxyResources([resource], 'chains', 2, 'exit')).toHaveLength(1)
  expect(filterProxyResources([resource], 'chains', 2, 'middle')).toEqual([])
  const applied = structuredClone(resource.hops); const external = applied[0]; if (external.kind === 'subscription') external.node_version_id = sourceUuid(400)
  resource.path_state!.desired_generation = 2; resource.path_state!.candidate_generation = 2; resource.path_state!.generations.forEach(view => { view.generation = 2 })
  resource.path_state!.applied_generation = 1; resource.path_state!.generations.push({ state: 'applied', generation: 1, hops: applied })
  expect(validProxyResource(resource)).toBe(true); expect(resource.hops).not.toEqual(applied)
  resource.path_state!.dependencies = Array.from({ length: 40 }, (_, index) => ({ server_id: 1, role: 'entry' as const, hop_position: null, generation: 1, stage: `recorded_stage_${index}`, required_revision: 1, applied_revision: 1, bundle_sha256: null, state: 'pending' as const, observed_at: null }))
  expect(validProxyResource(resource)).toBe(true)
  current.resources.data![1].chain_refs = [{ id: 10, name: resource.name, role: 'exit', hop_position: 2, generation: 1, state: 'applied' }, { id: 10, name: resource.name, role: 'exit', hop_position: 2, generation: 2, state: 'candidate' }]
  expect(validProxyResource(current.resources.data![1])).toBe(true)
  expect(validProxyResource({ ...current.resources.data![1], chain_refs: [{ id: 10, name: resource.name, role: 'exit' }] })).toBe(false)
  for (const malformed of [{ ...resource, hops: [] }, { ...resource, raw_config: 'TEST_ONLY' }, { ...resource, entry: { ...resource.entry, password: 'TEST_ONLY' } }, { ...resource, hops: [{ ...resource.hops[0], password: 'TEST_ONLY' }, resource.hops[1]] }, { ...resource, path_state: { ...resource.path_state, probe: { stage: 'candidate', state: 'verified' } } }]) {
    const view = validatedSnapshot({ data: malformed, fresh: true, error: '' }, validProxyResource, resource)
    expect(view.data).toBe(resource); expect(view.fresh).toBe(false)
  }
})
test('typed detail routes and strict role filters preserve legacy list bookmarks', () => {
  for (const base of ['/nodes']) {
    const route = (rest: string) => nodeRoute(`${base}${rest}`)
    expect(route('/chain/10')).toEqual({ kind: 'all', chains: false, resource: { kind: 'chain', id: 10 } })
    expect(route('?kind=chains&server=3&role=middle')).toEqual({ kind: 'chains', chains: true, serverId: 3, serverRole: 'middle' })
    for (const rest of ['/chain/01', '/chain/1?kind=chains', '?role=other', '?role=any&role=entry']) expect(route(rest)).toBeNull()
  }
})
test('source dependencies are explicit current/candidate/recovery references, not arbitrary JSON or credentials', () => {
  const source = subscriptionSourceFixture({ dependencies: [{ chain_id: 10, chain_name: '混合资源', generation: 1, state: 'candidate', hop_position: 1, external_node_id: sourceUuid(301), node_version_id: sourceUuid(401), identity_epoch: 1 }] })
  expect(validSubscriptionSource(source)).toBe(true)
  expect(validSubscriptionSource({ ...source, dependencies: [{ ...source.dependencies[0], password: 'TEST_ONLY' }] })).toBe(false)
})
test('public edit and pinned version update use CAS and retained exact receipts after a lost response', async () => {
  const current = snapshot(), resource = ordered(current); current.resources.data!.push(resource)
  for (const command of [{ kind: 'chain', id: 10, settings_revision: 1, operation: 'edit', fields: { name: '新名称' } }, { kind: 'chain', id: 10, settings_revision: 1, operation: 'versions', fields: { generation: 1, versions: [{ hop_position: 1, node_version_id: sourceUuid(405) }] } }] as ChainMutation[]) {
    resource.settings_revision = 1
    const pending = prepareChainMutation(command, current.resources, undefined, () => requestId), bodies: string[] = []
    await expect(submitChainMutation(pending, current.resources, async (_path, _method, body) => { bodies.push(JSON.stringify(body)); throw new Error('lost') })).rejects.toThrow()
    resource.settings_revision = 2
    const retry = prepareChainMutation(command, current.resources, pending, () => { throw new Error('new key forbidden') })
    await submitChainMutation(retry, current.resources, async (_path, _method, body) => { bodies.push(JSON.stringify(body)); return { request_id: requestId, kind: 'chain', id: 10, settings_revision: 2, generation: 1 } })
    expect(bodies).toEqual([pending.serialized, pending.serialized]); expect(chainMutationError(current.resources, command)).toContain('设置已变化')
  }
})
test('mutation malformed receipts keep pending replay, and unsafe fields never reach writer', async () => {
  const current = snapshot(); current.resources.data!.push(ordered(current))
  for (const fields of [{ password: 'TEST_ONLY' }, { hops: [] }, { request_id: nextId }, { port: 18085 }]) expect(() => prepareChainMutation({ kind: 'chain', id: 10, settings_revision: 1, operation: 'edit', fields }, current.resources, undefined, () => requestId)).toThrow()
  const command: ChainMutation = { kind: 'chain', id: 10, settings_revision: 1, operation: 'versions', fields: { generation: 1, versions: [{ hop_position: 1, node_version_id: sourceUuid(405) }] } }
  const pending = prepareChainMutation(command, current.resources, undefined, () => requestId)
  await expect(submitChainMutation(pending, current.resources, async () => ({ request_id: requestId, kind: 'chain', id: 10 }))).rejects.toThrow('收据尚未确认')
  expect(pending.attempted).toBe(true)
  current.resources.fresh = false
  await expect(submitChainMutation(pending, current.resources, async () => { throw new Error('writer forbidden') })).rejects.toThrow('正在刷新')
})


test('public and version mutation retries keep the original request while current dependencies are unavailable', async () => {
  const current = snapshot(), target = ordered(current); current.resources.data!.push(target)
  const command: ChainMutation = { kind: 'chain', id: target.id, settings_revision: 1, operation: 'edit', fields: { name: '保留名称' } }
  const pending = prepareChainMutation(command, current.resources, undefined, () => requestId), bodies: string[] = []
  await expect(submitChainMutation(pending, current.resources, async (_path, _method, body) => { bodies.push(JSON.stringify(body)); throw new Error('lost') })).rejects.toThrow('lost')
  target.settings_revision = 2
  const original = structuredClone(target)
  for (const change of ['entry', 'managed', 'source', 'missing', 'availability'] as const) {
    Object.assign(target, structuredClone(original))
    if (change === 'entry') target.entry.plugin_enabled = false
    if (change === 'managed') { const hop = target.hops.find(hop => hop.kind === 'managed')!; if (hop.kind === 'managed') hop.endpoint.node_deleted = true }
    if (change === 'source') { const hop = target.hops.find(hop => hop.kind === 'subscription')!; if (hop.kind === 'subscription') hop.source_archived = true }
    if (change === 'missing') { const hop = target.hops.find(hop => hop.kind === 'subscription')!; if (hop.kind === 'subscription') hop.node_present = false }
    if (change === 'availability') target.available = false
    expect(() => prepareChainMutation(command, current.resources, pending, () => nextId)).toThrow()
    await expect(submitChainMutation(pending, current.resources, async (_path, _method, body) => { bodies.push(JSON.stringify(body)); return {} })).rejects.toThrow()
    expect(pending.serialized).toBe(bodies[0])
  }
  expect(bodies).toHaveLength(1)
})
