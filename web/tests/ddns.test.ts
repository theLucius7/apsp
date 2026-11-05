import { describe, expect, test } from 'bun:test'
import { ddnsMessage, ddnsWrite, matchZone, ruleState, zoneName } from '../src/plugins/ddns/types'
import type { DdnsConfig, DdnsRule } from '../src/plugins/ddns/types'

describe('DDNS credentials and status', () => {
  const config: DdnsConfig = { name: '测试', server_id: 1, zone_id: '00000000000000000000000000000001', record_name: 'node.example.com', record_type: 'A', ttl: 300, proxied: false, interval_secs: 300, enabled: true, adopt_existing: false }
  test('ordinary edits omit saved credentials and preserve a revision', () => {
    expect(ddnsWrite(config, ' ', 3)).toEqual({ config, revision: 3 })
    const replacement = ddnsWrite({ ...config, proxied: true }, ' TEST_ONLY_TOKEN_VALUE ', 4)
    expect(replacement.api_token).toBe('TEST_ONLY_TOKEN_VALUE')
    expect(replacement.config.ttl).toBe(1)
    expect(config.ttl).toBe(300)
  })
  test('unknown provider text is never displayed as a raw message', () => {
    for (const code of ['TEST_ONLY_TOKEN_VALUE', '__proto__', 'constructor', 'toString']) expect(ddnsMessage(code)).toBe('状态未知，请稍后刷新')
    expect(ddnsMessage('no_public_ip')).toContain('公网地址')
    expect(ddnsMessage('server_offline')).toContain('保留')
    expect(ddnsMessage('rate_limited')).toContain('重试')
  })
})

test('cloud provider credentials are sent only as a pair and stale Cloudflare tokens are omitted', () => {
  const config: DdnsConfig = { provider: 'tencent', line: '0', name: '测试', server_id: 1, zone_id: 'example.com', record_name: 'node.example.com', record_type: 'A', ttl: 600, proxied: false, interval_secs: 300, enabled: false, adopt_existing: false }
  const write = ddnsWrite(config, 'TEST_ONLY_OLD_CF_TOKEN', 2, ' TEST_ONLY_ID ', ' TEST_ONLY_SECRET ')
  expect(write).toEqual({ config, revision: 2, access_key_id: 'TEST_ONLY_ID', access_key_secret: 'TEST_ONLY_SECRET' })
  expect(ddnsWrite(config, '', 3)).toEqual({ config, revision: 3 })
  expect(ddnsMessage('submitted')).toContain('等待')
})

test('manual source edits preserve the selected address and expose paused rollback status', () => {
  const config: DdnsConfig = { provider: 'cloudflare', name: '手工来源', server_id: 1, zone_id: '00000000000000000000000000000001', record_name: 'node.example.com', record_type: 'AAAA', ttl: 300, proxied: false, interval_secs: 300, enabled: false, adopt_existing: false, address_source: 'manual', manual_ip: '2001:db8::1' }
  expect(ddnsWrite(config, '', 5)).toEqual({ config, revision: 5 })
  expect(ddnsMessage('rolled_back')).toContain('暂停')
  expect(ddnsMessage('remote_changed')).toContain('拒绝覆盖')
})

test('credential center references prevent accidentally sending draft plaintext secrets', () => {
  const config: DdnsConfig = { provider: 'cloudflare', name: '引用凭据', server_id: 1, zone_id: '00000000000000000000000000000001', record_name: 'node.example.com', record_type: 'A', ttl: 300, proxied: false, interval_secs: 300, enabled: false, adopt_existing: false, credential_id: '00000000-0000-0000-0000-000000000001' }
  const write = ddnsWrite(config, 'TEST_ONLY_TOKEN_VALUE', 3, 'TEST_ONLY_KEY_ID', 'TEST_ONLY_KEY_SECRET')
  expect(write).toEqual({ config, revision: 3 })
  expect(ddnsMessage('credential_unavailable')).toContain('凭据中心')
})

test('a domain finds the saved account and the most specific zone that manage it', () => {
  const account = (id: string, provider: 'cloudflare' | 'tencent', zones: Record<string, string> | string[], enabled = true) => ({ id, config: { provider, zone_ids: Array.isArray(zones) ? zones : Object.keys(zones), zone_names: Array.isArray(zones) ? undefined : zones, enabled } })
  const cloudflare = account('cf', 'cloudflare', { ['1'.repeat(32)]: 'example.com', ['2'.repeat(32)]: 'hk.example.com' })
  const tencent = account('tc', 'tencent', ['example.net'])
  const accounts = [cloudflare, tencent, account('off', 'tencent', ['example.org'], false)]
  expect(matchZone(accounts, 'node.example.com')).toEqual({ account: cloudflare, zoneId: '1'.repeat(32), domain: 'example.com' })
  expect(matchZone(accounts, ' A.HK.Example.com. ')?.zoneId).toBe('2'.repeat(32))
  expect(matchZone(accounts, '*.example.net')?.account.id).toBe('tc')
  expect(matchZone(accounts, 'example.net')?.domain).toBe('example.net')
  expect(matchZone(accounts, 'notexample.com')).toBeNull()
  expect(matchZone(accounts, 'node.example.org')).toBeNull()
  expect(matchZone(accounts, '')).toBeNull()
  expect(zoneName('huawei', '3'.repeat(32))).toBeNull()
})

test('rule badges stay short and keep the explanation separate', () => {
  const rule = { id: 'r', config: { name: 'n', server_id: 1, zone_id: 'z', record_name: 'node.example.com', record_type: 'A', ttl: 1, proxied: false, interval_secs: 300, enabled: true, adopt_existing: false }, revision: 1, token_configured: true, busy: false, server_name: 's', candidate_ip: null, ip_status: 'server_offline', ip_received_at: null, last_ip: '198.51.100.1', last_success_at: 5, attempted_at: null, next_run_at: 0, status: 'updated', error_code: null, failures: 0 } as DdnsRule
  expect(ruleState(rule, true)).toEqual({ label: '服务器离线', tone: 'warm', detail: '服务器离线，保留现有解析' })
  expect(ruleState({ ...rule, ip_status: 'ready' }, true)).toEqual({ label: '已更新', tone: 'good', detail: '' })
  expect(ruleState({ ...rule, ip_status: 'ready', status: 'error', error_code: 'record_not_owned' }, true).detail).toContain('接管')
  expect(ruleState({ ...rule, config: { ...rule.config, enabled: false } }, true).label).toBe('已暂停')
  expect(ruleState(rule, false).label).toBe('状态未知')
  expect(ruleState({ ...rule, ip_status: 'ready', status: 'TEST_ONLY_unknown' }, true).label).toBe('状态未知')
})
