import { expect, test } from 'bun:test'
import { artifactComponent, artifactGroups } from '../src/governance/system/artifactGroups'
import { resolveRoute } from '../src/app/routes'
import type { Artifact } from '../src/types'

const artifact = (name: string, version: string, arch: string): Artifact => ({ name, version, arch, sha256: 'a'.repeat(64), bytes: 1024 })

test('one component contains every architecture and version without mutating the API snapshot', () => {
  const input = [artifact('sing-box', '1.14.2', 'arm64'), artifact('sing-box', '1.14.2', 'amd64'), artifact('sing-box', '1.13.0', 'amd64'), artifact('nodequality', 'snapshot-r12', 'arm64')]
  const before = structuredClone(input)
  const runtime = artifactGroups(input).find(group => group.id === 'runtime')!.components[0]
  expect(runtime.name).toBe('sing-box')
  expect(runtime.architectures).toEqual(['amd64', 'arm64'])
  expect(runtime.versions.map(item => item.version)).toEqual(['1.14.2', '1.13.0'])
  expect(runtime.versions[0].packages.map(item => item.arch)).toEqual(['amd64', 'arm64'])
  expect(runtime.versions[1].packages.map(item => item.arch)).toEqual(['amd64'])
  expect(input).toEqual(before)
})

test('known components are grouped by what they are and stay listed without versions', () => {
  const groups = artifactGroups([])
  expect(groups.map(group => [group.id, group.components.map(component => component.name)])).toEqual([
    ['agent', ['agent']], ['runtime', ['sing-box']], ['diagnostics', ['ipquality', 'tcpquality', 'nodequality']],
  ])
  for (const component of groups.flatMap(group => group.components)) {
    expect(component.versions).toEqual([])
    expect(component.architectures).toEqual([])
  }
})

test('unknown components keep their own name and are not labelled as a runtime', () => {
  const groups = artifactGroups([artifact('agent', '0.3.0', 'amd64'), artifact('agent', '0.3.0', 'arm64'), artifact('custom-component', '2', 'amd64'), artifact('custom-component', '2', 'arm64')])
  expect(groups.find(group => group.id === 'agent')!.components[0].architectures).toEqual(['amd64', 'arm64'])
  const other = groups.find(group => group.id === 'other')!
  expect(other.components.map(component => [component.name, component.title, component.architectures])).toEqual([['custom-component', 'custom-component', ['amd64', 'arm64']]])
  expect(groups.find(group => group.id === 'runtime')!.components.some(component => component.name === 'custom-component')).toBeFalse()
})

test('the platform/version matrix does not invent cross-platform version availability', () => {
  const item = artifactComponent('tcpquality', 'TCP', [artifact('tcpquality', 'source-r1', 'linux-amd64'), artifact('tcpquality', 'source-r2', 'linux-arm64')])
  expect(item.versions.map(version => [version.version, version.packages.map(item => item.arch)])).toEqual([
    ['source-r2', ['linux-arm64']], ['source-r1', ['linux-amd64']],
  ])
})

test('the plugin centre addresses open their new homes for one release', () => {
  expect(resolveRoute('/settings/artifacts')).toEqual({ page: 'artifacts' })
  expect(resolveRoute('/settings/plugins')).toEqual({ page: 'artifacts' })
  expect(resolveRoute('/settings/plugins/servers')).toEqual({ page: 'nodes', view: 'runtimes' })
  expect(resolveRoute('/servers/42/plugins')).toEqual({ page: 'server', serverId: 42, section: 'services' })
  expect(resolveRoute('/servers/42/services')).toEqual({ page: 'server', serverId: 42, section: 'services' })
  for (const path of ['/settings/plugins/import-release', '/settings/plugins/1', '/settings/artifacts/1']) expect(resolveRoute(path)).toEqual({ page: 'not-found' })
})
