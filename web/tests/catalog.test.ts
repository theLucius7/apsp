import { expect, test } from 'bun:test'
import { catalogItem, isCatalogPath, pluginCatalog, pluginDefinitions, pluginServerPath } from '../src/plugins/catalog'
import type { Artifact } from '../src/types'

const artifact = (name: string, version: string, arch: string): Artifact => ({ name, version, arch, sha256: 'a'.repeat(64), bytes: 1024 })

test('one product contains every architecture and version without mutating the API snapshot', () => {
  const input = [artifact('sing-box', '1.14.2', 'arm64'), artifact('sing-box', '1.14.2', 'amd64'), artifact('sing-box', '1.13.0', 'amd64'), artifact('nodequality', 'snapshot-r12', 'arm64')]
  const before = structuredClone(input)
  const catalog = pluginCatalog(input)
  expect(catalog.plugins).toHaveLength(6)
  const plugin = catalog.plugins.find(item => item.id === 'sing-box')!
  expect(plugin.architectures).toEqual(['amd64', 'arm64'])
  expect(plugin.versions.map(item => item.version)).toEqual(['1.14.2', '1.13.0'])
  expect(plugin.versions[0].packages.map(item => item.arch)).toEqual(['amd64', 'arm64'])
  expect(plugin.versions[1].packages.map(item => item.arch)).toEqual(['amd64'])
  expect(input).toEqual(before)
})

test('catalog descriptions remain available before any signed download packages exist', () => {
  const catalog = pluginCatalog([])
  expect(catalog.plugins.map(item => item.id)).toEqual(['alicloud', 'ddns', 'sing-box', 'ipquality', 'tcpquality', 'nodequality'])
  for (const plugin of catalog.plugins) {
    expect(plugin.description.length).toBeGreaterThan(10)
    expect(plugin.usage.length).toBeGreaterThan(10)
    expect(plugin.versions).toEqual([])
    expect(plugin.architectures).toEqual([])
  }
})

test('Agent is a base component, unknown packages are not mislabeled as proxy plugins', () => {
  const catalog = pluginCatalog([artifact('agent', '0.3.0', 'amd64'), artifact('agent', '0.3.0', 'arm64'), artifact('custom-component', '2', 'amd64'), artifact('custom-component', '2', 'arm64')])
  expect(catalog.plugins.some(item => item.id === 'agent' || item.id === 'custom-component')).toBeFalse()
  expect(catalog.agent.versions).toHaveLength(1)
  expect(catalog.agent.architectures).toEqual(['amd64', 'arm64'])
  expect(catalog.others).toHaveLength(1)
  expect(catalog.others[0].id).toBe('custom-component')
  expect(catalog.others[0].architectures).toEqual(['amd64', 'arm64'])
})

test('the platform/version matrix does not invent cross-platform version availability', () => {
  const item = catalogItem('tcpquality', [artifact('tcpquality', 'source-r1', 'linux-amd64'), artifact('tcpquality', 'source-r2', 'linux-arm64')])
  expect(item.versions.map(version => [version.version, version.packages.map(item => item.arch)])).toEqual([
    ['source-r2', ['linux-arm64']], ['source-r1', ['linux-amd64']],
  ])
})

test('cloud account plugins have a global destination and device plugins require a server identity', () => {
  // The three line diagnostics open the merged 线路诊断 tab of the server.
  expect(pluginDefinitions.map(plugin => pluginServerPath(plugin, 42))).toEqual([null, '/servers/42/ddns', '/servers/42/plugins', '/servers/42/ip-info', '/servers/42/ip-info', '/servers/42/ip-info'])
  expect(pluginDefinitions.filter(plugin => plugin.group === 'diagnostics').map(plugin => plugin.id)).toEqual(['ipquality', 'tcpquality', 'nodequality'])
  expect(pluginDefinitions.find(plugin => plugin.id === 'alicloud')?.panelPath).toBe('/plugins/alicloud')
  for (const plugin of pluginDefinitions) {
    for (const invalid of [0, -1, 1.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) expect(pluginServerPath(plugin, invalid)).toBeNull()
  }
})

test('legacy artifact bookmarks show the catalog without matching unrelated routes', () => {
  for (const path of ['/plugins/catalog', '/artifacts']) expect(isCatalogPath(path)).toBeTrue()
  for (const path of ['/artifacts/import-release', '/plugins/catalog/1', '/system/plugins', '/servers']) expect(isCatalogPath(path)).toBeFalse()
})
