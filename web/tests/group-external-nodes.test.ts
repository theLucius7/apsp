import { describe, expect, test } from 'bun:test'
import { externalSelectionError } from '../src/plugins/singbox/GroupExternalNodes'
import { externalReference } from '../src/plugins/singbox/externalAccessTypes'
import type { ExternalEntry } from '../src/plugins/singbox/externalAccessTypes'

const node: ExternalEntry = { external_node_id: 1, source_id: 2, identity_epoch: 1, node_version_id: 3, update_mode: 'follow_node', metadata_revision: 0, name: '原节点', source_name: '原来源', protocol: 'tuic', server: 'provider.example.com', port: 443, available: true, reason: null, current_version_id: 3, resolved_version_id: 3, source_last_error: null }
const gone: ExternalEntry = { ...node, available: false, reason: 'node_missing', resolved_version_id: null }

describe('external nodes in a policy group', () => {
  test('an adopted node on its current version can be added', () => {
    expect(externalSelectionError([externalReference(node)], [], [node])).toBe('')
  })
  test('a node the group already holds stays even while unavailable', () => {
    expect(externalSelectionError([externalReference(gone)], [gone], [gone])).toBe('')
  })
  test('an unavailable node cannot be added', () => {
    expect(externalSelectionError([externalReference(gone)], [], [gone])).not.toBe('')
  })
  test('a changed identity or metadata revision needs a new choice', () => {
    expect(externalSelectionError([{ ...externalReference(node), identity_epoch: 2 }], [], [node])).not.toBe('')
    expect(externalSelectionError([{ ...externalReference(node), metadata_revision: 1 }], [], [node])).not.toBe('')
    expect(externalSelectionError([externalReference(node)], [], [])).not.toBe('')
  })
  test('pinning keeps the current version', () => {
    expect(externalSelectionError([{ ...externalReference(node), update_mode: 'pinned' }], [], [node])).toBe('')
  })
})
