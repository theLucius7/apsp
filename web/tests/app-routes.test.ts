import { expect, test } from 'bun:test'
import { resolveRoute } from '../src/app/routes'
import { currentNavigation, homePath, navigation } from '../src/app/navigation'
import { visibleByTier } from '../src/app/tiers'
import { nodeHash } from '../src/plugins/singbox/nodeRoute'

test('dashboard aliases keep the public read-only route after shell extraction', () => {
  for (const path of ['/', '/dashboard', '/overview']) expect(resolveRoute(path)).toEqual({ page: 'dashboard' })
  for (const path of ['/dashboard/42', '/overview/42']) expect(resolveRoute(path)).toEqual({ page: 'dashboard', serverId: 42 })
  expect(resolveRoute('/servers')).toEqual({ page: 'servers' })
  expect(currentNavigation(resolveRoute('/'))?.path).toBe('/dashboard')
})

test('old device DDNS, catalog alias and unified node selection retain their destinations', () => {
  expect(resolveRoute('/servers/42/ddns')).toEqual({ page: 'server', serverId: 42, section: 'ddns' })
  expect(currentNavigation(resolveRoute('/servers/42/ddns'))?.path).toBe('/plugins/ddns')
  expect(currentNavigation(resolveRoute('/servers/42/node-quality'))?.path).toBe('/servers')
  expect(resolveRoute('/artifacts')).toEqual({ page: 'catalog' })
  expect(currentNavigation(resolveRoute('/artifacts'))?.path).toBe('/plugins/catalog')
  expect(resolveRoute('/plugins/sing-box/nodes?server=42')).toEqual({ page: 'nodes', serverId: 42, chains: false })
  expect(resolveRoute('/plugins/sing-box/nodes?kind=chains')).toEqual({ page: 'nodes', chains: true })
  expect(resolveRoute('/plugins/sing-box/nodes/chain/42')).toEqual({ page: 'nodes', selected: { kind: 'chain', id: 42 } })
  expect(currentNavigation(resolveRoute('/plugins/alicloud'))?.path).toBe('/plugins/alicloud')
})

test('unsafe resource IDs and ambiguous filtered paths never fall through into management routes', () => {
  for (const path of ['/servers/0', '/servers/9007199254740992/plugins', '/dashboard/9007199254740992',
    '/plugins/sing-box/nodes/direct/9007199254740992', '/plugins/sing-box/nodes?role=unknown',
    '/plugins/sing-box/nodes?server=42&server=43', 'constructor', '__proto__']) {
    expect(resolveRoute(path)).toEqual({ page: 'not-found' })
    expect(currentNavigation(resolveRoute(path))).toBeUndefined()
  }
})

test('combined node filters and old chain entry use the same page without replacing drafts', () => {
  expect(resolveRoute('/plugins/sing-box/nodes?server=42&kind=chains&role=middle')).toEqual({ page: 'nodes', serverId: 42, chains: true, serverRole: 'middle' })
  expect(resolveRoute('/plugins/sing-box/nodes?server=42&kind=direct')).toEqual({ page: 'nodes', serverId: 42, chains: false, kind: 'direct' })
  expect(resolveRoute('/plugins/sing-box/chains')).toEqual({ page: 'nodes', chains: true })
  expect(resolveRoute('/plugins/sing-box')).toEqual({ page: 'singbox-overview' })
  for (const query of ['server=42&role=exit&role=entry', 'server=01', 'kind=chains&kind=direct', 'server=42&kind=chains&other=1']) expect(resolveRoute(`/plugins/sing-box/nodes?${query}`)).toEqual({ page: 'not-found' })
})

test('node page sections are addressable and malformed sections are refused', () => {
  expect(resolveRoute('/plugins/sing-box/nodes?view=sources')).toEqual({ page: 'nodes', chains: false, view: 'sources' })
  expect(resolveRoute('/plugins/sing-box/nodes?server=42&view=chains')).toEqual({ page: 'nodes', serverId: 42, chains: false, view: 'chains' })
  expect(resolveRoute('/plugins/sing-box/nodes?kind=chains&view=catalog')).toEqual({ page: 'nodes', chains: true, view: 'catalog' })
  for (const query of ['view=unknown', 'view=chains&view=sources', 'view=']) expect(resolveRoute(`/plugins/sing-box/nodes?${query}`)).toEqual({ page: 'not-found' })
  expect(nodeHash({ view: 'catalog' })).toBe('#/plugins/sing-box/nodes')
  expect(nodeHash({ server: '2', view: 'chains' })).toBe('#/plugins/sing-box/nodes?server=2&view=chains')
  expect(nodeHash({ kind: 'chains', view: 'chains' })).toBe('#/plugins/sing-box/nodes?kind=chains')
  expect(nodeHash({ kind: 'chains', view: 'catalog' })).toBe('#/plugins/sing-box/nodes?kind=chains&view=catalog')
  expect(nodeHash({ server: '3', role: 'middle', view: 'chains' })).toBe('#/plugins/sing-box/nodes?server=3&role=middle&view=chains')
  for (const view of ['catalog', 'chains', 'sources'] as const) expect(resolveRoute(nodeHash({ server: '7', view }).slice(1))).toMatchObject({ page: 'nodes', serverId: 7 })
})

test('navigation groups follow administrator tasks and keep every destination', () => {
  expect([...new Set(navigation.map(item => item.group))]).toEqual(['总览', '代理服务', '服务器', '监控告警', '系统', '实验功能'])
  expect(navigation).toHaveLength(22)
  expect(navigation.filter(item => item.group === '代理服务').map(item => item.label)).toEqual(['代理节点', '代理用户', '策略与套餐', '成员运营'])
  // Every group keeps two to four entries; only the last group holds experimental features.
  for (const group of new Set(navigation.map(item => item.group))) {
    const size = navigation.filter(item => item.group === group).length
    expect(size).toBeGreaterThanOrEqual(2)
    expect(size).toBeLessThanOrEqual(4)
  }
  expect(navigation.filter(item => item.tier === 'experimental').map(item => item.page)).toEqual(['operations', 'network-configuration'])
  expect(navigation[0]).toMatchObject({ page: 'singbox-overview', label: '概览', path: homePath })
  // A group never repeats its own name as an entry, so the breadcrumb is never "服务器 / 服务器".
  for (const item of navigation) expect(item.label).not.toBe(item.group)
  expect(new Set(navigation.map(item => item.path)).size).toBe(navigation.length)
  expect(currentNavigation(resolveRoute('/plugins/sing-box/nodes?view=sources'))?.label).toBe('代理节点')
  expect(currentNavigation(resolveRoute('/plugins/sing-box/nodes?view=sources'))?.group).toBe('代理服务')
  expect(currentNavigation(resolveRoute('/plugins/sing-box'))?.group).toBe('总览')
})

test('platform management pages preserve selected server context alongside the node views', () => {
  for (const section of ['network-workbench', 'network-configuration', 'operations'] as const) {
    expect(resolveRoute(`/servers/42/${section}`)).toEqual({ page: 'server', serverId: 42, section })
    expect(currentNavigation(resolveRoute(`/servers/42/${section}`))?.path).toBe(`/${section}`)
    expect(resolveRoute(`/${section}`)).toEqual({ page: section })
  }
  // The operations tools moved into server detail; their old addresses stay with the server list.
  expect(resolveRoute('/servers/42/fleet')).toEqual({ page: 'server', serverId: 42, section: 'fleet' })
  expect(currentNavigation(resolveRoute('/servers/42/fleet'))?.path).toBe('/servers')
  expect(resolveRoute('/fleet')).toEqual({ page: 'fleet' })
  expect(currentNavigation(resolveRoute('/fleet'))?.path).toBe('/servers')
  expect(resolveRoute('/servers/batch')).toEqual({ page: 'batch-enrollment' })
  expect(currentNavigation(resolveRoute('/servers/batch'))?.path).toBe('/servers')
  expect(resolveRoute('/system/backup')).toEqual({ page: 'backup' })
  expect(currentNavigation(resolveRoute('/system/backup'))?.group).toBe('系统')
  expect(resolveRoute('/system/control-center')).toEqual({ page: 'control-center' })
  expect(currentNavigation(resolveRoute('/system/control-center'))?.group).toBe('系统')
  for (const path of ['/servers/0/fleet', '/servers/9007199254740992/network-workbench', '/servers/42/operations/extra']) {
    expect(resolveRoute(path)).toEqual({ page: 'not-found' })
  }
})

test('server sections, plugin centre views and split settings resolve to one sidebar entry each', () => {
  for (const section of ['assets', 'agent', 'plugins', 'ip-info', 'node-quality', 'tcp-quality'] as const) {
    expect(resolveRoute(`/servers/42/${section}`)).toEqual({ page: 'server', serverId: 42, section })
    expect(currentNavigation(resolveRoute(`/servers/42/${section}`))?.path).toBe('/servers')
  }
  expect(resolveRoute('/system/plugins')).toEqual({ page: 'plugins' })
  expect(currentNavigation(resolveRoute('/system/plugins'))?.label).toBe('插件中心')
  expect(currentNavigation(resolveRoute('/plugins/catalog'))?.label).toBe('插件中心')
  expect(resolveRoute('/monitoring/settings')).toEqual({ page: 'alert-settings' })
  expect(currentNavigation(resolveRoute('/monitoring/settings'))?.group).toBe('监控告警')
  expect(currentNavigation(resolveRoute('/system/settings'))?.label).toBe('面板设置')
  expect(currentNavigation(resolveRoute('/system/notifications'))?.group).toBe('监控告警')
  expect(currentNavigation(resolveRoute('/plugins/ddns'))?.group).toBe('服务器')
  expect(currentNavigation(resolveRoute('/plugins/alicloud'))?.group).toBe('服务器')
  expect(currentNavigation(resolveRoute('/plugins/catalog'))?.group).toBe('服务器')
  expect(currentNavigation(resolveRoute('/network-workbench'))?.group).toBe('监控告警')
  for (const path of ['/servers/42/overview', '/servers/42/assets/extra', '/monitoring/settings/extra']) expect(resolveRoute(path)).toEqual({ page: 'not-found' })
})

test('member and expense destinations preserve public and administrator boundaries', () => {
  expect(resolveRoute('/plugins/sing-box/member')).toEqual({ page: 'member-login' })
  expect(resolveRoute('/plugins/sing-box/commerce')).toEqual({ page: 'commerce' })
  expect(resolveRoute('/plugins/expenses')).toEqual({ page: 'expenses' })
  expect(currentNavigation(resolveRoute('/plugins/expenses'))?.group).toBe('总览')
})

test('frozen entries stay hidden until the panel shows them', () => {
  const entries = [{ key: 'core' }, { key: 'experimental', tier: 'experimental' as const }, { key: 'frozen', tier: 'frozen' as const }]
  expect(visibleByTier(entries, false).map(entry => entry.key)).toEqual(['core', 'experimental'])
  expect(visibleByTier(entries, true).map(entry => entry.key)).toEqual(['core', 'experimental', 'frozen'])
})
