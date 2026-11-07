export type NodeTransport =
  | { type: 'tcp' }
  | { type: 'ws'; path: string; host: string | null; max_early_data: number; early_data_header_name: string }
  | { type: 'httpupgrade'; path: string; host: string | null }
  | { type: 'grpc'; service_name: string }

export type NodeSettings = {
  realm?: { server_url: string; realm_id: string; server_token: string; client_token: string; stun_servers: string[]; ip_version: number } | null;
  public_addresses?: { name: string; server: string; port: number; sni?: string; host?: string }[];
  listen: string; public_port: number | null; tcp_fast_open: boolean; tls_alpn: string[];
  disable_tcp_keep_alive?: boolean; tcp_keep_alive_seconds?: number | null; tcp_keep_alive_interval_seconds?: number | null;
  tls_min_version?: string | null; tls_max_version?: string | null; tls_handshake_timeout_seconds?: number | null;
  transport?: NodeTransport;
  reality: { handshake_server: string | null; handshake_port: number; fingerprint: string; max_time_difference_seconds?: number | null; flow?: 'vision' | 'none' };
  hysteria2: { up_mbps: number | null; down_mbps: number | null; ignore_client_bandwidth: boolean; obfs_enabled: boolean; bbr_profile?: string; masquerade?: { status_code: number; content_type: string; content: string } | null };
  tuic: { congestion_control: string; auth_timeout_seconds: number | null; heartbeat_seconds: number | null; zero_rtt_handshake: boolean; udp_relay_mode?: string };
  anytls: { idle_session_check_seconds: number | null; idle_session_timeout_seconds: number | null; min_idle_session: number | null; padding_scheme?: string[] };
  snell?: { mode: string; reuse: boolean };
  shadowsocks?: { udp_over_tcp: boolean; multiplex: { enabled: boolean; padding: boolean; protocol: string; max_connections: number | null; min_streams: number | null; max_streams: number | null } };
}
const numeric = (form: FormData, key: string) => { const value = String(form.get(key) ?? '').trim(); return value ? Number(value) : null }
const checked = (form: FormData, key: string) => form.get(key) === 'on'
const text = (form: FormData, key: string, fallback = '') => String(form.get(key) ?? fallback).trim()

export function nodeSettingsRequest(form: FormData) {
  const protocol = text(form, 'protocol'), tcp = !['hysteria2', 'tuic'].includes(protocol)
  const tls = ['hysteria2', 'tuic', 'anytls', 'naive', 'vless-tls','trojan','vmess','http','mixed'].includes(protocol), disableKeepAlive = tcp && checked(form, 'disable_tcp_keep_alive')
  const settings = {
    public_addresses: JSON.parse(text(form, 'public_addresses', '[]') || '[]'),
    listen: text(form, 'listen', '::'), public_port: numeric(form, 'public_port'),
    tcp_fast_open: tcp && checked(form, 'tcp_fast_open'), disable_tcp_keep_alive: disableKeepAlive,
    tcp_keep_alive_seconds: tcp && !disableKeepAlive ? numeric(form, 'tcp_keep_alive_seconds') : null,
    tcp_keep_alive_interval_seconds: tcp && !disableKeepAlive ? numeric(form, 'tcp_keep_alive_interval_seconds') : null,
    tls_alpn: tls ? text(form, 'tls_alpn').split(',').map(value => value.trim()).filter(Boolean) : [],
    tls_min_version: tls ? text(form, 'tls_min_version') || null : null,
    tls_max_version: tls ? text(form, 'tls_max_version') || null : null,
    tls_handshake_timeout_seconds: tcp && (tls || protocol === 'vless-reality') ? numeric(form, 'tls_handshake_timeout_seconds') : null,
  }
  if (['vless-reality','vless-tls','vless-tunnel','trojan','vmess'].includes(protocol)) {
    const transportType = text(form, 'transport_type', 'tcp')
    const transport: NodeTransport = transportType === 'ws' ? { type: 'ws', path: text(form, 'transport_path', '/') || '/', host: text(form, 'transport_host') || null, max_early_data: numeric(form, 'max_early_data') ?? 0, early_data_header_name: (numeric(form, 'max_early_data') ?? 0) > 0 ? text(form, 'early_data_header_name') : '' }
      : transportType === 'httpupgrade' ? { type: 'httpupgrade', path: text(form, 'transport_path', '/') || '/', host: text(form, 'transport_host') || null }
      : transportType === 'grpc' ? { type: 'grpc', service_name: text(form, 'service_name') } : { type: 'tcp' }
    return { ...settings, transport, ...(protocol === 'vless-reality' ? { reality: { handshake_server: text(form, 'handshake_server') || null, handshake_port: numeric(form, 'handshake_port') ?? 443, fingerprint: text(form, 'fingerprint', 'chrome'), max_time_difference_seconds: numeric(form, 'max_time_difference_seconds'), flow: transport.type === 'tcp' ? text(form, 'reality_flow', 'vision') : 'none' } } : {}) }
  }
  if (protocol === 'hysteria2') {
    const bbr = checked(form, 'ignore_client_bandwidth')
    return { ...settings, realm: JSON.parse(text(form, 'realm', 'null') || 'null'), hysteria2: { up_mbps: bbr ? null : numeric(form, 'up_mbps'), down_mbps: bbr ? null : numeric(form, 'down_mbps'), ignore_client_bandwidth: bbr, obfs_enabled: checked(form, 'obfs_enabled'), ...(form.get('obfs_password') ? { obfs_password: String(form.get('obfs_password')) } : {}), bbr_profile: text(form, 'bbr_profile', 'standard'), masquerade: checked(form, 'masquerade_enabled') ? { status_code: numeric(form, 'masquerade_status_code') ?? 200, content_type: (numeric(form, 'masquerade_status_code') ?? 200) === 200 ? text(form, 'masquerade_content_type', 'text/plain') : '', content: [204, 304].includes(numeric(form, 'masquerade_status_code') ?? 200) ? '' : String(form.get('masquerade_content') ?? '') } : null } }
  }
  if (protocol === 'tuic') return { ...settings, tuic: { congestion_control: text(form, 'congestion_control', 'cubic'), auth_timeout_seconds: numeric(form, 'auth_timeout_seconds'), heartbeat_seconds: numeric(form, 'heartbeat_seconds'), zero_rtt_handshake: checked(form, 'zero_rtt_handshake'), udp_relay_mode: text(form, 'udp_relay_mode', 'native') } }
  if (protocol === 'anytls') return { ...settings, anytls: { idle_session_check_seconds: numeric(form, 'idle_session_check_seconds'), idle_session_timeout_seconds: numeric(form, 'idle_session_timeout_seconds'), min_idle_session: numeric(form, 'min_idle_session'), padding_scheme: text(form, 'padding_scheme').split('\n').map(value => value.trim()).filter(Boolean) } }
  if (protocol === 'snell-v6') return { ...settings, snell: { mode: text(form, 'snell_mode', 'default'), reuse: checked(form, 'snell_reuse') } }
  if (protocol === 'shadowsocks2022') {
    const enabled = checked(form, 'multiplex_enabled')
    return { ...settings, shadowsocks: { udp_over_tcp: checked(form, 'udp_over_tcp'), multiplex: { enabled, padding: enabled && checked(form, 'multiplex_padding'), protocol: enabled ? text(form, 'multiplex_protocol', 'h2mux') : 'h2mux', max_connections: enabled && text(form, 'multiplex_limit', 'connections') === 'connections' ? numeric(form, 'max_connections') : null, min_streams: enabled && text(form, 'multiplex_limit', 'connections') === 'connections' ? numeric(form, 'min_streams') : null, max_streams: enabled && text(form, 'multiplex_limit', 'connections') === 'streams' ? numeric(form, 'max_streams') : null } } }
  }
  return settings
}
