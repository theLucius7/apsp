import { expect, test } from 'bun:test'
import { resolveRoute } from '../src/app/routes'
import { currentNavigation, homePath, navigation, sectionTabs, sidebarEntries } from '../src/app/navigation'
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

test('the subscription board sits beside the server board and its cards open one proxy user', () => {
  expect(resolveRoute('/subscriptions')).toEqual({ page: 'subscription-board' })
  expect(currentNavigation(resolveRoute('/subscriptions'))).toMatchObject({ label: '订阅看板', group: '总览', parent: '/plugins/sing-box' })
  expect(resolveRoute('/plugins/sing-box/users')).toEqual({ page: 'proxy-users' })
  expect(resolveRoute('/plugins/sing-box/commerce')).toEqual({ page: 'proxy-users' })
  expect(resolveRoute('/plugins/sing-box/users/42')).toEqual({ page: 'proxy-users', userId: 42 })
  expect(currentNavigation(resolveRoute('/plugins/sing-box/users/42'))?.path).toBe('/plugins/sing-box/users')
  for (const path of ['/plugins/sing-box/users/0', '/plugins/sing-box/users/01', '/plugins/sing-box/users/9007199254740992', '/plugins/sing-box/users/x']) expect(resolveRoute(path)).toEqual({ page: 'not-found' })
})

test('china reachability is a view of the latency entry, not another sidebar entry', () => {
  expect(resolveRoute('/latency/reachability')).toEqual({ page: 'latency-reachability' })
  expect(currentNavigation(resolveRoute('/latency/reachability'))?.path).toBe('/latency')
  expect(resolveRoute('/latency')).toEqual({ page: 'latency' })
  expect(sectionTabs(currentNavigation(resolveRoute('/latency/reachability')), navigation).map(item => item.label)).toEqual(['网络检测', '延迟检测'])
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
  expect([...new Set(navigation.map(item => item.group))]).toEqual(['总览', '代理服务', '服务器', '监控与设置', '实验功能'])
  // Every destination stays; pages about the same thing are tabs of one sidebar entry (ADR 0102).
  expect(navigation).toHaveLength(22)
  const everyday = sidebarEntries(navigation.filter(item => !item.tier))
  expect(everyday.map(entry => entry.root.navLabel ?? entry.root.label)).toEqual(['概览', '费用与续费', '代理节点', '代理用户', '策略与套餐', '服务器', '域名解析', '阿里云 CDT', '告警', '线路检测', '设置'])
  expect(navigation.filter(item => item.group === '代理服务').map(item => item.label)).toEqual(['代理节点', '代理用户', '策略与套餐'])
  expect(sectionTabs(currentNavigation(resolveRoute('/dashboard')), navigation).map(item => item.label)).toEqual(['概览', '统计报表', '服务器看板', '订阅看板'])
  expect(sectionTabs(currentNavigation(resolveRoute('/subscriptions')), navigation).map(item => item.label)).toEqual(['概览', '统计报表', '服务器看板', '订阅看板'])
  expect(sectionTabs(currentNavigation(resolveRoute('/latency')), navigation).map(item => item.label)).toEqual(['网络检测', '延迟检测'])
  expect(sectionTabs(currentNavigation(resolveRoute('/plugins/sing-box/nodes')), navigation)).toEqual([])
  // Someone who may only open some tabs of an entry goes to the first of those.
  const limited = navigation.filter(item => item.page === 'control-center' || item.page === 'security')
  expect(sidebarEntries(limited)).toHaveLength(1)
  expect(sidebarEntries(limited)[0].href).toBe('/system/control-center')
  expect(sidebarEntries(limited)[0].root.navLabel).toBe('设置')
  expect(navigation.filter(item => item.tier === 'experimental').map(item => item.page)).toEqual(['operations', 'network-configuration'])
  expect(navigation[0]).toMatchObject({ page: 'singbox-overview', label: '概览', path: homePath })
  // A group never repeats its own name as a page.
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
  expect(currentNavigation(resolveRoute('/system/backup'))?.group).toBe('监控与设置')
  expect(resolveRoute('/system/control-center')).toEqual({ page: 'control-center' })
  expect(currentNavigation(resolveRoute('/system/control-center'))?.group).toBe('监控与设置')
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
  expect(currentNavigation(resolveRoute('/monitoring/settings'))?.group).toBe('监控与设置')
  expect(currentNavigation(resolveRoute('/system/settings'))?.label).toBe('面板设置')
  expect(currentNavigation(resolveRoute('/system/notifications'))?.group).toBe('监控与设置')
  expect(currentNavigation(resolveRoute('/plugins/ddns'))?.group).toBe('服务器')
  expect(currentNavigation(resolveRoute('/plugins/alicloud'))?.group).toBe('服务器')
  expect(currentNavigation(resolveRoute('/plugins/catalog'))?.group).toBe('监控与设置')
  expect(currentNavigation(resolveRoute('/network-workbench'))?.group).toBe('监控与设置')
  for (const path of ['/servers/42/overview', '/servers/42/assets/extra', '/monitoring/settings/extra']) expect(resolveRoute(path)).toEqual({ page: 'not-found' })
})

test('member and expense destinations preserve public and administrator boundaries', () => {
  expect(resolveRoute('/plugins/sing-box/member')).toEqual({ page: 'member-login' })
  expect(resolveRoute('/plugins/sing-box/commerce')).toEqual({ page: 'proxy-users' })
  expect(resolveRoute('/plugins/expenses')).toEqual({ page: 'expenses' })
  expect(currentNavigation(resolveRoute('/plugins/expenses'))?.group).toBe('总览')
})

test('frozen entries stay hidden until the panel shows them', () => {
  const entries = [{ key: 'core' }, { key: 'experimental', tier: 'experimental' as const }, { key: 'frozen', tier: 'frozen' as const }]
  expect(visibleByTier(entries, false).map(entry => entry.key)).toEqual(['core', 'experimental'])
  expect(visibleByTier(entries, true).map(entry => entry.key)).toEqual(['core', 'experimental', 'frozen'])
})
