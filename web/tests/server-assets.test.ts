import { expect, test } from 'bun:test'
import { assetDraft, assetPayload, defaultAssets, expiryState, parseTraffic, trafficSize } from '../src/resources/server-assets'
import { filterServers } from '../src/observability/display/data'
import type { Server } from '../src/types'

test('large traffic limits survive editing exactly and decimal units are explicit', () => {
  for (const value of ['0', '1', '9007199254740993', '18446744073709551615', '1099511627776', '1610612736']) {
    expect(assetPayload(assetDraft({ ...defaultAssets, traffic_limit: value })).traffic_limit).toBe(value)
  }
  expect(parseTraffic('1.5', 'GB')).toBe('1500000000')
  expect(parseTraffic('1.5', 'GiB')).toBe('1610612736')
  expect(() => parseTraffic('0.1', 'B')).toThrow()
  expect(() => parseTraffic('18446744073709551616', 'B')).toThrow()
  expect(() => parseTraffic('1e2', 'GB')).toThrow()
  expect(trafficSize('0')).toBe('0 B')
  expect(trafficSize(null)).toBe('—')
})

test('asset edits retain free versus unknown cost, clear optional fields and use UTC dates', () => {
  const draft = assetDraft({ ...defaultAssets, price: '0.00', expires_at: 1709164800 })
  expect(draft.expiry).toBe('2024-02-29')
  expect(assetPayload(draft).price).toBe('0.00')
  expect(assetPayload({ ...draft, price: '', expiry: '' }).expires_at).toBeNull()
  expect(assetPayload({ ...draft, price: '' }).price).toBeNull()
  expect(assetPayload({ ...draft, tags: '线路, 主力，线路' }).tags).toEqual(['线路', '主力'])
  expect(expiryState({ ...defaultAssets, expires_at: 1709164800 }, 1709164800000).label).toBe('已到期')
  expect(expiryState(defaultAssets).label).toBe('未设置到期')
})

test('server search includes configured region, group and tags', () => {
  const server = { name: '测试', static_info: {}, online: true, asset_settings: { ...defaultAssets, region: 'JP', group_name: '主力', tags: ['线路:BGP'] } } as Server
  for (const query of ['jp', '主力', 'bgp']) expect(filterServers([server], query, 'all')).toEqual([server])
  expect(filterServers([server], '不存在', 'all')).toEqual([])
})
