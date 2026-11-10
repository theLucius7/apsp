import { expect, test } from 'bun:test'
import { protocolRequest } from '../src/traffic/ProtocolFields'
import { nodeSettingsRequest } from '../src/traffic/NodeSettingsFields'

function form(values: Record<string, string>) {
  const form = new FormData()
  for (const [key, value] of Object.entries(values)) form.set(key, value)
  return form
}

test('editing manual TLS without a replacement preserves stored secrets', () => {
  expect(protocolRequest(form({ protocol: 'anytls', tls_mode: 'manual', certificate: '', key: '' })))
    .toEqual({ type: 'anytls', tls: { mode: 'manual' } })
})

test('automatic certificates send only contact and challenge settings', () => {
  const request = protocolRequest(form({ protocol: 'tuic', tls_mode: 'acme', email: ' admin@example.com ', challenge: 'tls-alpn-01', key: 'TEST_ONLY stale form value' }))
  expect(request).toEqual({ type: 'tuic', tls: { mode: 'acme', email: 'admin@example.com', challenge: 'tls-alpn-01' } })
})

test('switching to a protocol without TLS does not submit certificate material', () => {
  expect(protocolRequest(form({ protocol: 'snell-v6', certificate: 'TEST_ONLY stale certificate', key: 'TEST_ONLY stale key' })))
    .toEqual({ type: 'snell-v6' })
})

test('incomplete manual replacement is sent for validation rather than silently discarded', () => {
  expect(protocolRequest(form({ protocol: 'hysteria2', tls_mode: 'manual', key: 'TEST_ONLY incomplete key' })))
    .toEqual({ type: 'hysteria2', tls: { mode: 'manual', certificate: '', key: 'TEST_ONLY incomplete key' } })
})

test('node settings preserve obfuscation secrets and clear a public port explicitly', () => {
  const request = nodeSettingsRequest(form({ protocol:'hysteria2', listen:'::', public_port:'', obfs_enabled:'on', obfs_password:'', up_mbps:'80', down_mbps:'40' }))
  expect(request.public_port).toBeNull()
  expect(request).toHaveProperty('hysteria2', { up_mbps:80, down_mbps:40, ignore_client_bandwidth:false, obfs_enabled:true, bbr_profile:'standard', masquerade:null })
  expect(request).not.toHaveProperty('hysteria2.obfs_password')
})

test('protocol changes cannot submit another protocol group', () => {
  const request = nodeSettingsRequest(form({ protocol:'tuic', listen:'0.0.0.0', public_port:'443', congestion_control:'bbr', heartbeat_seconds:'10', obfs_enabled:'on', obfs_password:'TEST_ONLY stale field', handshake_port:'8443' }))
  expect(request.public_port).toBe(443)
  expect(request).not.toHaveProperty('hysteria2')
  expect(request).not.toHaveProperty('reality')
  expect(request).toHaveProperty('tuic.heartbeat_seconds', 10)
})

test('transport selection disables Vision and drops inactive transport fields', () => {
  const request = nodeSettingsRequest(form({ protocol:'vless-reality', transport_type:'grpc', reality_flow:'vision', service_name:'example-service', transport_path:'/unused', max_early_data:'2048' }))
  expect(request).toHaveProperty('transport', {type:'grpc',service_name:'example-service'})
  expect(request).toHaveProperty('reality.flow','none')
  expect(nodeSettingsRequest(form({protocol:'vless-reality'}))).toHaveProperty('reality.flow','vision')
})

test('disabled TCP and bandwidth fields cannot leak stale draft values', () => {
  const tcp = nodeSettingsRequest(form({protocol:'snell-v6',disable_tcp_keep_alive:'on',tcp_keep_alive_seconds:'30',tcp_keep_alive_interval_seconds:'10'}))
  expect(tcp.tcp_keep_alive_seconds).toBeNull()
  expect(tcp.tcp_keep_alive_interval_seconds).toBeNull()
  const quic = nodeSettingsRequest(form({protocol:'hysteria2',tcp_fast_open:'on',tcp_keep_alive_seconds:'30',tls_handshake_timeout_seconds:'20',ignore_client_bandwidth:'on',up_mbps:'10',down_mbps:'20'}))
  expect(quic.tcp_fast_open).toBe(false)
  expect(quic.tls_handshake_timeout_seconds).toBeNull()
  expect(quic).toHaveProperty('hysteria2.up_mbps',null)
  expect(quic).toHaveProperty('hysteria2.down_mbps',null)
})

test('optional values clear explicitly and protocol options survive request building', () => {
  const anytls = nodeSettingsRequest(form({protocol:'anytls',tls_min_version:'1.2',tls_max_version:'1.3',padding_scheme:'stop=2\n0=30-30\n1=100-400\n'}))
  expect(anytls.tls_min_version).toBe('1.2')
  expect(anytls.tls_handshake_timeout_seconds).toBeNull()
  expect(anytls).toHaveProperty('anytls.padding_scheme',['stop=2','0=30-30','1=100-400'])
  expect(nodeSettingsRequest(form({protocol:'snell-v6',snell_mode:'unshaped',snell_reuse:'on'}))).toHaveProperty('snell',{mode:'unshaped',reuse:true})
  expect(nodeSettingsRequest(form({protocol:'tuic',udp_relay_mode:'udp-over-stream'}))).toHaveProperty('tuic.udp_relay_mode','udp-over-stream')
})

test('multiplex limits remain exclusive and disabling returns compatible defaults', () => {
  const request = nodeSettingsRequest(form({protocol:'shadowsocks2022',multiplex_enabled:'on',multiplex_limit:'streams',multiplex_protocol:'smux',max_connections:'4',min_streams:'4',max_streams:'16',multiplex_padding:'on',udp_over_tcp:'on'}))
  expect(request).toHaveProperty('shadowsocks',{udp_over_tcp:true,multiplex:{enabled:true,padding:true,protocol:'smux',max_connections:null,min_streams:null,max_streams:16}})
  const disabled = nodeSettingsRequest(form({protocol:'shadowsocks2022',multiplex_protocol:'smux',multiplex_padding:'on',max_streams:'16'}))
  expect(disabled).toHaveProperty('shadowsocks.multiplex',{enabled:false,padding:false,protocol:'h2mux',max_connections:null,min_streams:null,max_streams:null})
})

test('HY2 response fields respect fixed runtime and HTTP body limitations', () => {
  expect(nodeSettingsRequest(form({protocol:'hysteria2',masquerade_enabled:'on',masquerade_status_code:'404',masquerade_content_type:'application/json',masquerade_content:'TEST_ONLY body'}))).toHaveProperty('hysteria2.masquerade',{status_code:404,content_type:'',content:'TEST_ONLY body'})
  expect(nodeSettingsRequest(form({protocol:'hysteria2',masquerade_enabled:'on',masquerade_status_code:'204',masquerade_content:'TEST_ONLY stale body'}))).toHaveProperty('hysteria2.masquerade',{status_code:204,content_type:'',content:''})
})


test('HTTPUpgrade drops inactive WS and gRPC settings while preserving the selected endpoint', () => {
  const request = nodeSettingsRequest(form({ protocol: 'vless-reality', transport_type: 'httpupgrade', transport_path: '/upgrade', transport_host: 'proxy.example.com', reality_flow: 'vision', max_early_data: '2048', early_data_header_name: 'TEST_ONLY inactive', service_name: 'TEST_ONLY inactive' }))
  expect(request).toHaveProperty('transport', { type: 'httpupgrade', path: '/upgrade', host: 'proxy.example.com' })
  expect(request).toHaveProperty('reality.flow', 'none')
})
