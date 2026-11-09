import { describe, expect, test } from 'bun:test'
import { catalogKey, catalogMutationError, filterCatalog, parsedTags, renamed, tagsError, validCatalog } from '../src/plugins/singbox/catalog'
import type { CatalogNode, CatalogFilter } from '../src/plugins/singbox/catalog'
import { sourceReason } from '../src/plugins/singbox/sourceTypes'

const node = (fields: Partial<CatalogNode>): CatalogNode => ({ id: 1, kind: 'direct', name: '香港入口', original_name: '香港入口', public_host: 'node.example.com', protocol: 'vless-reality', enabled: true, available: true, server_id: 1, server_name: '测试服务器', port: 443, role: 'direct', tcp: true, udp: true, stage: 'direct', reference_count: 0, sort_order: 0, revision: '1'.repeat(64), tags: [], note: '', ...fields }) as CatalogNode
const empty: CatalogFilter = { search: '', kind: '', protocol: '', tag: '', status: '', source: '', server: '', role: '' }
const nodes = [node({}), node({ kind: 'external', name: '东京', original_name: '提供方旧名', source_id: 3, server_id: null, tags: ['常用'], note: '待续费', protocol: 'tuic' }), node({ id: 2, enabled: false, available: false }), node({ id: 3, available: false })]
describe('node catalog', () => {
  test('resource identities never collide across kinds', () => expect(new Set(nodes.map(catalogKey)).size).toBe(4))
  test('search retains origin, notes and tags after local rename', () => {
    for (const search of ['提供方旧名', '待续费', '常用']) expect(filterCatalog(nodes, { ...empty, search }).map(catalogKey)).toEqual(['external:1'])
  })
  test('combines source/protocol/tag and server filters without inventing external servers', () => {
    expect(filterCatalog(nodes, { ...empty, protocol: 'tuic', tag: '常用', source: '3' }).length).toBe(1)
    expect(filterCatalog(nodes, { ...empty, server: '1', kind: 'external' })).toEqual([])
  })
  test('disabled and unavailable remain separate', () => {
    expect(filterCatalog(nodes, { ...empty, status: 'disabled' }).map(catalogKey)).toEqual(['direct:2'])
    expect(filterCatalog(nodes, { ...empty, status: 'unavailable' }).map(catalogKey)).toEqual(['direct:3'])
  })
  test('rename replaces literal strings, not regular expressions or replacement templates', () => {
    expect(renamed('a.*a.*', 'replace', '.*', '$&')).toBe('a$&a$&')
    expect(renamed('node', 'prefix', 'HK ')).toBe('HK node')
    expect(renamed('node', 'suffix', ' 02')).toBe('node 02')
    expect(renamed('node', 'replace', '', 'x')).toBe('node')
  })
  test('tag normalization deduplicates and enforces UTF-8 byte limits', () => {
    expect(parsedTags(' 香港, 香港，常用\n 测试 ')).toEqual(['香港', '常用', '测试'])
    expect(tagsError(['中'.repeat(21)])).toBe('')
    expect(tagsError(['中'.repeat(22)])).not.toBe('')
    expect(tagsError(Array.from({ length: 17 }, (_, index) => String(index)))).not.toBe('')
  })
})

describe('catalog current mutation eligibility', () => {
  const original = node({})
  test('accepts current exact hash and preserves legitimate zero metadata/order', () => {
    expect(validCatalog([original])).toBe(true)
    expect(catalogMutationError([original], [original], '1')).toBe('')
  })
  test('missing/empty/wrong or unsafe revision fails closed even when both sides match', () => {
    for (const revision of [undefined, '', 'managed-1', '1'.repeat(63), 0, Number.MAX_SAFE_INTEGER + 1]) {
      const corrupt = { ...original, revision } as unknown as CatalogNode
      expect(validCatalog([corrupt])).toBe(false)
      expect(catalogMutationError([corrupt], [corrupt], '')).not.toBe('')
    }
  })
  test('disappeared/current changed/filter changed rows retain the draft and reject mutation', () => {
    expect(catalogMutationError([], [original], '')).not.toBe('')
    expect(catalogMutationError([{ ...original, revision: '2'.repeat(64) }], [original], '')).not.toBe('')
    expect(catalogMutationError([original], [original], '2')).not.toBe('')
    expect(catalogMutationError(undefined, [original], '')).not.toBe('')
  })
  test('a managed node is in scope only on the server it listens on; the catalog has no chain rows', () => {
    const hop = node({ id: 4, role: 'managed_hop', server_id: 2 })
    expect(filterCatalog([hop], { ...empty, server: '2' })).toEqual([hop])
    expect(filterCatalog([hop], { ...empty, server: '3' })).toEqual([])
    expect(validCatalog([{ ...node({}), kind: 'chain', role: 'chain_entry' } as unknown as CatalogNode])).toBe(false)
  })
  test('unavailable external nodes explain the catalog reason without echoing unknown codes', () => {
    for (const reason of ['source_deleted', 'source_archived', 'source_replaced', 'node_missing', 'ambiguous_node_identity', 'unsupported_configuration']) expect(sourceReason(reason)).not.toBe('')
    expect(sourceReason('unknown-fixture-secret')).not.toContain('fixture-secret')
    expect(sourceReason(null)).toBe('')
  })
  test('external missing endpoint is readable without inventing a managed server or protocol', () => {
    const external = node({ kind: 'external', role: 'external', server_id: null, server_name: null, protocol: null, public_host: null, port: null, source_id: 1, source_name: '旧来源', version_id: null, identity_epoch: 1, available: false })
    expect(validCatalog([external])).toBe(true)
    expect(filterCatalog([external], { ...empty, server: '1' })).toEqual([])
    expect(filterCatalog([external], { ...empty, search: '旧来源' })).toEqual([])
  })
})
