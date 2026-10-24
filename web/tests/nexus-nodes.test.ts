import { expect, test } from 'bun:test'
import { filterNexusNodes, nexusEndpoint, nexusProtocol, validNexusNodes } from '../src/plugins/singbox/nexusNodeTypes'
import type { NexusNode } from '../src/plugins/singbox/nexusNodeTypes'

const node: NexusNode = { id: 1, source_id: 'fixture', legacy_node_id: 31, legacy_machine_id: 7, legacy_host_id: 1,
  server_id: 20, server_name: '第一台主机', machine_name: '旧实例', machine_active: true, name: '测试入口', protocol: 'vless',
  public_host: 'node.example.invalid', public_port: '443', listen_port: '8443', rate: '0', enabled: true, visible: false,
  sort_order: 0, tags: ['归档'], parent_id: null, groups: [{ id: 1, name: '测试组' }], routes: [],
  protocol_summary: { tls: 2, short_id: '0123456789abcdef' }, custom_route_count: 1, custom_outbound_count: 1,
  timed_rate_enabled: false, timed_rate_count: 0, certificate_saved: false, configuration_saved: true,
  state: 'pending_agent', exported_at: 1, imported_at: 2 }

test('imported identities remain separate from original node and host ids', () => {
  expect(validNexusNodes([node])).toBe(true)
  expect(validNexusNodes([node, { ...node, id: 2 }])).toBe(false)
  expect(validNexusNodes([node, { ...node, id: 2, source_id: 'second-source' }])).toBe(true)
  expect(validNexusNodes([node, { ...node, legacy_node_id: 32 }])).toBe(false)
  expect(validNexusNodes([{ ...node, state: 'active' }])).toBe(false)
  expect(validNexusNodes([{ ...node, protocol_summary: { unexpected: { secret: 'fixture' } } }])).toBe(false)
})

test('filtering includes source labels, original ids and unregistered hosts', () => {
  const other = { ...node, id: 2, legacy_node_id: 32, protocol: 'hysteria', server_id: 21 }
  expect(filterNexusNodes([node, other], '测试组', '', '')).toHaveLength(2)
  expect(filterNexusNodes([node, other], '归档', 'vless', '20')).toEqual([node])
  expect(filterNexusNodes([node, other], '31', '', '')).toEqual([node])
  expect(filterNexusNodes([node, other], '', 'vless', '21')).toEqual([])
})

test('port ranges, IPv6 and nonstandard short ids remain visible without rewriting', () => {
  expect(nexusEndpoint({ ...node, public_host: '2001:db8::1', public_port: '21000-21020' })).toBe('[2001:db8::1]:21000-21020')
  expect(nexusProtocol(node)).toBe('VLESS · Reality')
  expect(nexusProtocol({ ...node, protocol: 'hysteria', protocol_summary: { version: 2 } })).toBe('Hysteria2')
  expect(node.protocol_summary.short_id).toBe('0123456789abcdef')
})
